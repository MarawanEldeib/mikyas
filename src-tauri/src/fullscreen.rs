//! Auto-hide while a fullscreen app (a video, a course, a presentation) owns the foreground on the widget's monitor
//! (`settings.auto_hide_fullscreen`); the widget returns, without taking the focus, when it ends.
//!
//! A small thread samples the foreground window every 1.5 s while the setting is on (every 30 s
//! otherwise, or at once when the setting changes). **Safety:**
//! detection uses window queries only — `GetForegroundWindow`, `GetWindowRect`,
//! `MonitorFromWindow` / `GetMonitorInfoW`, `GetClassNameW` and `SHQueryUserNotificationState`. It
//! never opens another process (`OpenProcess`), reads another process's memory, injects code or
//! installs hooks.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::state::{HiddenReason, Shared, lock};
use crate::visibility::{self, Event};

/// Sampling period while auto-hide is on.
pub const POLL_ON: Duration = Duration::from_millis(1500);
/// Idle period while it is off (a settings change wakes the thread at once).
pub const POLL_OFF: Duration = Duration::from_secs(30);
/// Consecutive agreeing samples before the widget hides or returns, so Alt+Tab or Task View
/// passing through the foreground does not make it flicker.
pub const STABLE_SAMPLES: u8 = 2;
/// How far (px) a fullscreen window's edges may stray from its monitor's. A maximized window's
/// frame overhangs every edge by its thickness (8–9 px at 100–125 %), so it never matches.
pub const EDGE_SLACK: i32 = 1;

/// Screen rectangle in physical pixels (`right`/`bottom` exclusive, like Win32 `RECT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    /// Every edge of `self` is within [`EDGE_SLACK`] px of the same edge of `other`.
    pub fn matches(&self, other: &Rect) -> bool {
        [
            self.left - other.left,
            self.top - other.top,
            self.right - other.right,
            self.bottom - other.bottom,
        ]
        .iter()
        .all(|d| d.abs() <= EDGE_SLACK)
    }
}

/// `SHQueryUserNotificationState`, reduced to the states that matter here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quns {
    /// `QUNS_BUSY`: the shell considers a fullscreen app to be running.
    Busy,
    /// `QUNS_RUNNING_D3D_FULL_SCREEN`: an exclusive-fullscreen Direct3D app.
    D3dFullScreen,
    /// `QUNS_PRESENTATION_MODE`: Windows presentation settings are on.
    PresentationMode,
    /// Anything else, or the query failed.
    Other,
}

impl Quns {
    pub fn from_raw(state: i32) -> Self {
        match state {
            2 => Self::Busy,
            3 => Self::D3dFullScreen,
            4 => Self::PresentationMode,
            _ => Self::Other,
        }
    }
}

/// What the window queries report about the foreground window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    pub class: String,
    pub rect: Rect,
    /// Full bounds of the monitor the window is on (`rcMonitor`, taskbar included).
    pub monitor: Rect,
    /// That monitor's handle, compared with the widget's.
    pub monitor_id: isize,
    /// The widget itself is in front.
    pub is_ours: bool,
}

/// Desktop, taskbar and task-switcher windows: they cover a monitor without being a fullscreen app.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "ForegroundStaging",
];

/// Window class names are case-insensitive.
pub fn is_shell_class(class: &str) -> bool {
    SHELL_CLASSES.iter().any(|c| c.eq_ignore_ascii_case(class))
}

/// Does a fullscreen app own the foreground on the widget's monitor (`widget_monitor`)?
///
/// Exclusive Direct3D and presentation mode say so outright. Otherwise the window must match its
/// monitor (within [`EDGE_SLACK`]), even when the shell reports busy: a maximized window stops
/// at the taskbar, and under an auto-hidden taskbar its frame overhangs every edge (the shell may
/// then call it busy). A fullscreen window, even one that also reports maximized, does not.
pub fn is_fullscreen(fg: Option<&Foreground>, quns: Quns, widget_monitor: Option<isize>) -> bool {
    let Some(fg) = fg else {
        return quns == Quns::PresentationMode;
    };
    if fg.is_ours {
        return false;
    }
    if quns == Quns::PresentationMode {
        return true;
    }
    if is_shell_class(&fg.class) || widget_monitor.is_some_and(|m| m != fg.monitor_id) {
        return false;
    }
    quns == Quns::D3dFullScreen || fg.rect.matches(&fg.monitor)
}

/// Turns raw samples into state changes once [`STABLE_SAMPLES`] of them agree.
#[derive(Debug, Default)]
pub struct Debounce {
    state: bool,
    pending: u8,
}

impl Debounce {
    /// `Some(new_state)` when the debounced state flips.
    pub fn push(&mut self, sample: bool) -> Option<bool> {
        if sample == self.state {
            self.pending = 0;
            return None;
        }
        self.pending += 1;
        if self.pending < STABLE_SAMPLES {
            return None;
        }
        self.state = sample;
        self.pending = 0;
        Some(sample)
    }

    pub fn state(&self) -> bool {
        self.state
    }
}

/// Auto-hide applies only while the setting is on and click-through ("ghost") is off: ghost mode
/// is the explicit "keep it over this fullscreen video or app" choice.
pub fn auto_hide_active(setting: bool, click_through: bool) -> bool {
    setting && !click_through
}

/// What one tick of the thread should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    Nothing,
    Emit(Event),
}

/// One tick: `enabled` is the setting now, `was_enabled` its value on the previous tick and
/// `sample` the fullscreen test (only taken while enabled). `hidden_by_fullscreen_and_visible`
/// asks whether the widget reappeared although auto-hide hid it.
pub fn tick(
    debounce: &mut Debounce,
    enabled: bool,
    was_enabled: bool,
    sample: impl FnOnce() -> bool,
    hidden_by_fullscreen_and_visible: impl FnOnce() -> bool,
) -> Tick {
    if !enabled {
        *debounce = Debounce::default();
        return if was_enabled { Tick::Emit(Event::AutoHideOff) } else { Tick::Nothing };
    }
    match debounce.push(sample()) {
        Some(true) => Tick::Emit(Event::FullscreenStarted),
        Some(false) => Tick::Emit(Event::FullscreenEnded),
        None if debounce.state() && hidden_by_fullscreen_and_visible() => Tick::Emit(Event::FullscreenOngoing),
        None => Tick::Nothing,
    }
}

// ---------------------------------------------------------------------------------------------
// Thread

struct Waker(Mutex<Sender<()>>);

/// Starts the sampling thread.
pub fn start(app: &AppHandle, shared: Arc<Shared>) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    app.manage(Waker(Mutex::new(tx)));
    let app = app.clone();
    std::thread::Builder::new()
        .name("cuw-fullscreen".into())
        .spawn(move || run(&app, &shared, &rx))?;
    Ok(())
}

/// Re-evaluates at once (the setting changed).
pub fn wake(app: &AppHandle) {
    if let Some(waker) = app.try_state::<Waker>() {
        let _ = lock(&waker.0).send(());
    }
}

fn run(app: &AppHandle, shared: &Shared, rx: &Receiver<()>) {
    let mut debounce = Debounce::default();
    let mut was_enabled = false;
    loop {
        if shared.quitting.load(Ordering::SeqCst) {
            break;
        }
        let setting = shared.settings().auto_hide_fullscreen;
        let enabled = auto_hide_active(setting, shared.ui().click_through);
        let action = tick(
            &mut debounce,
            enabled,
            was_enabled,
            || sample(app),
            || {
                // Window queries wait for the main thread, so no lock may be held across them.
                let reason = shared.ui().hidden_reason;
                reason == HiddenReason::Fullscreen && widget_visible(app)
            },
        );
        if let Tick::Emit(event) = action {
            visibility::dispatch(app, event);
        }
        was_enabled = enabled;
        match rx.recv_timeout(if enabled { POLL_ON } else { POLL_OFF }) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn widget_visible(app: &AppHandle) -> bool {
    crate::window::get(app).is_some_and(|w| w.is_visible().unwrap_or(false))
}

fn sample(app: &AppHandle) -> bool {
    let own = crate::window::get(app).and_then(|w| crate::platform::window_id(&w));
    let foreground = crate::platform::foreground_window(own);
    let widget_monitor = own.and_then(crate::platform::monitor_id);
    is_fullscreen(foreground.as_ref(), crate::platform::notification_state(), widget_monitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn fg(class: &str, rect: Rect) -> Foreground {
        Foreground {
            class: class.into(),
            rect,
            monitor: MONITOR,
            monitor_id: 1,
            is_ours: false,
        }
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    #[test]
    fn matching_the_monitor() {
        assert!(MONITOR.matches(&MONITOR));
        assert!(rect(-1, 1, 1921, 1079).matches(&MONITOR), "within a pixel on every edge");
        assert!(!rect(-8, -8, 1928, 1088).matches(&MONITOR), "a maximized frame overhangs");
        assert!(!rect(-8, -8, 1928, 1040).matches(&MONITOR), "maximized above the taskbar");
        assert!(!rect(0, 0, 1920, 1078).matches(&MONITOR));
        assert!(!rect(2, 0, 1920, 1080).matches(&MONITOR));
    }

    #[test]
    fn shell_windows_are_excluded() {
        for class in ["Progman", "WorkerW", "Shell_TrayWnd", "shell_traywnd", "Shell_SecondaryTrayWnd"] {
            assert!(is_shell_class(class), "{class}");
            assert!(!is_fullscreen(Some(&fg(class, MONITOR)), Quns::Busy, Some(1)), "{class}");
        }
        assert!(!is_shell_class("UnrealWindow"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn borderless_and_exclusive_fullscreen() {
        let player = fg("MediaPlayerClassicW", MONITOR);
        assert!(is_fullscreen(Some(&player), Quns::Busy, Some(1)));
        assert!(is_fullscreen(Some(&player), Quns::Other, Some(1)), "exact monitor size");
        let exclusive = fg("D3DPresenterWindow", rect(0, 0, 1280, 720));
        assert!(is_fullscreen(Some(&exclusive), Quns::D3dFullScreen, Some(1)));
        // A window that overshoots the monitor by a pixel on every side still counts.
        assert!(is_fullscreen(Some(&fg("SDL_app", rect(-1, -1, 1921, 1081))), Quns::Busy, None));
    }

    #[test]
    fn maximized_windows_are_not_fullscreen() {
        let maximized = fg("Chrome_WidgetWin_1", rect(-8, -8, 1928, 1040));
        assert!(!is_fullscreen(Some(&maximized), Quns::Other, Some(1)));
        assert!(!is_fullscreen(Some(&maximized), Quns::Busy, Some(1)));
        // Auto-hidden taskbar: the frame overhangs every edge of the monitor. Even when the shell
        // calls that busy, the overhang tells it from a fullscreen window.
        for over in [rect(-8, -8, 1928, 1088), rect(-9, -9, 1929, 1089)] {
            let over = fg("Notepad", over);
            assert!(!is_fullscreen(Some(&over), Quns::Other, Some(1)));
            assert!(!is_fullscreen(Some(&over), Quns::Busy, Some(1)), "{:?}", over.rect);
        }
    }

    #[test]
    fn fullscreen_means_the_monitor_rect_within_a_pixel() {
        // A browser in fullscreen (also maximized: IsZoomed is true) or a borderless fullscreen player.
        assert!(is_fullscreen(Some(&fg("Chrome_WidgetWin_1", MONITOR)), Quns::Busy, Some(1)));
        assert!(is_fullscreen(Some(&fg("SDL_app", rect(1, 0, 1920, 1081))), Quns::Other, Some(1)));
        assert!(!is_fullscreen(Some(&fg("SDL_app", rect(-2, 0, 1920, 1080))), Quns::Busy, Some(1)));
        assert!(!is_fullscreen(Some(&fg("SDL_app", rect(0, 0, 1920, 1078))), Quns::Busy, Some(1)));
    }

    #[test]
    fn our_window_other_monitors_and_no_foreground() {
        let mut ours = fg("Tauri Window", MONITOR);
        ours.is_ours = true;
        assert!(!is_fullscreen(Some(&ours), Quns::Busy, Some(1)));
        assert!(!is_fullscreen(Some(&ours), Quns::PresentationMode, Some(1)));
        // A fullscreen app on the other monitor leaves the widget alone.
        assert!(!is_fullscreen(Some(&fg("UnrealWindow", MONITOR)), Quns::Busy, Some(2)));
        assert!(!is_fullscreen(Some(&fg("UnrealWindow", MONITOR)), Quns::D3dFullScreen, Some(2)));
        // Lock screen / no foreground window.
        assert!(!is_fullscreen(None, Quns::Busy, Some(1)));
        assert!(!is_fullscreen(None, Quns::D3dFullScreen, Some(1)));
        // Presenting hides it regardless of the window in front.
        assert!(is_fullscreen(None, Quns::PresentationMode, Some(1)));
        assert!(is_fullscreen(Some(&fg("Notepad", rect(10, 10, 500, 500))), Quns::PresentationMode, Some(2)));
    }

    #[test]
    fn quns_mapping() {
        assert_eq!(Quns::from_raw(2), Quns::Busy);
        assert_eq!(Quns::from_raw(3), Quns::D3dFullScreen);
        assert_eq!(Quns::from_raw(4), Quns::PresentationMode);
        for other in [0, 1, 5, 6, 7, -1] {
            assert_eq!(Quns::from_raw(other), Quns::Other);
        }
    }

    #[test]
    fn debounce_needs_agreeing_samples() {
        let mut d = Debounce::default();
        assert_eq!(d.push(true), None, "one sample is a blip");
        assert_eq!(d.push(false), None);
        assert_eq!(d.push(true), None);
        assert_eq!(d.push(true), Some(true));
        assert!(d.state());
        assert_eq!(d.push(true), None);
        assert_eq!(d.push(false), None);
        assert_eq!(d.push(false), Some(false));
        assert!(!d.state());
    }

    fn run_ticks(samples: &[(bool, bool)], reappeared: bool) -> Vec<Tick> {
        let mut d = Debounce::default();
        let mut was = false;
        samples
            .iter()
            .map(|&(enabled, fullscreen)| {
                let t = tick(&mut d, enabled, was, || fullscreen, || reappeared);
                was = enabled;
                t
            })
            .collect()
    }

    #[test]
    fn tick_transitions() {
        use Tick::{Emit, Nothing};
        let on = |fs| (true, fs);
        assert_eq!(
            run_ticks(&[on(false), on(true), on(true), on(true), on(false), on(false)], false),
            [Nothing, Nothing, Emit(Event::FullscreenStarted), Nothing, Nothing, Emit(Event::FullscreenEnded)]
        );
        // Something showed the widget while it was auto-hidden: hide it again.
        assert_eq!(
            run_ticks(&[on(true), on(true), on(true)], true),
            [Nothing, Emit(Event::FullscreenStarted), Emit(Event::FullscreenOngoing)]
        );
        // Switching the setting off restores the widget once and resets the state.
        assert_eq!(
            run_ticks(&[on(true), on(true), (false, true), (false, true), on(false)], false),
            [Nothing, Emit(Event::FullscreenStarted), Emit(Event::AutoHideOff), Nothing, Nothing]
        );
    }

    #[test]
    fn ghost_mode_pauses_auto_hide() {
        assert!(auto_hide_active(true, false));
        assert!(!auto_hide_active(true, true), "click-through keeps the widget over a fullscreen video");
        assert!(!auto_hide_active(false, false));
        // Turning ghost mode on over a fullscreen video brings an auto-hidden widget back.
        let mut d = Debounce::default();
        for _ in 0..3 {
            tick(&mut d, auto_hide_active(true, false), true, || true, || false);
        }
        let t = tick(&mut d, auto_hide_active(true, true), true, || true, || false);
        assert_eq!(t, Tick::Emit(Event::AutoHideOff));
        assert_eq!(
            crate::visibility::transition(HiddenReason::Fullscreen, false, Event::AutoHideOff).1,
            crate::visibility::Action::Show
        );
    }

    #[test]
    fn the_sample_is_not_taken_while_disabled() {
        let mut d = Debounce::default();
        let t = tick(&mut d, false, false, || panic!("sampled while off"), || false);
        assert_eq!(t, Tick::Nothing);
    }
}
