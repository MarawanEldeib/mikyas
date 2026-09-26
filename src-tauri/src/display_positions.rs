//! Per-display widget position: one remembered position per monitor setup.
//!
//! - Active while `settings.per_display_position` is on; the window-state plugin's position stays
//!   the fallback for a setup seen for the first time.
//! - Signature of a setup = the sorted monitor rects plus scale factors.
//! - Stored in `<data_root>/positions.json`: signature → the window's visible rect (without the
//!   invisible frame, so a widget flush against an edge comes back flush). Saved when it
//!   moved (seen by the poll) while it shows the pill or the card, undocked or as the dock strip
//!   (so a docked widget stores its edge position); never while slid out or with a panel open.
//! - Restored on startup and when the monitor setup changes (polled every 3 s with window
//!   queries only; a changed setup must hold for one more poll, as Windows moves windows around
//!   while monitors come and go). The corner nearest the screen edges is kept, so a pill
//!   restores where a card was saved; a strip then re-snaps flush to its edge. The poll runs on
//!   its own thread; the restore itself runs on the main thread.
//! - Switching the setting wakes the poll at once (`wake`, from the settings hook).
//! - The same poll redraws the tray number when the scale or taskbar theme changed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use cuw_core::time::{Ms, now_ms};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow};

use crate::dock::{self, DockState};
use crate::settings::ViewMode;
use crate::state::{Shared, load_json, save_json};
use crate::window::{Rect, anchored_position_with, area_for, nearest_anchor, visible_rect, work_areas};

/// Poll period while the setting is on.
pub const POLL_ON: Duration = Duration::from_secs(3);
/// Poll period while it is off (only the tray style is refreshed then).
pub const POLL_OFF: Duration = Duration::from_secs(30);
/// Monitor setups remembered (least recently used dropped first).
pub const KEEP_SETUPS: usize = 16;
const FILE_NAME: &str = "positions.json";

/// A monitor as the signature sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorInfo {
    /// Full monitor rect (physical px, taskbar included).
    pub rect: Rect,
    pub scale: f64,
}

/// "x,y,wxh@scale%" per monitor, sorted, joined by ";".
pub fn signature(monitors: &[MonitorInfo]) -> String {
    let mut parts: Vec<String> = monitors
        .iter()
        .map(|m| {
            let (x, y, w, h) = m.rect;
            format!("{x},{y},{w}x{h}@{}", (m.scale * 100.0).round() as i64)
        })
        .collect();
    parts.sort();
    parts.join(";")
}

/// One remembered position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// Visible window rect (x, y, w, h), physical px.
    pub rect: Rect,
    pub used_ms: Ms,
}

/// `<data_root>/positions.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PositionsFile {
    pub setups: BTreeMap<String, Saved>,
}

impl PositionsFile {
    /// Stores the rect for a setup, keeping the [`KEEP_SETUPS`] most recently used. Use stamps
    /// only go forward, so a clock set back never makes the setup just saved the oldest.
    pub fn remember(&mut self, signature: &str, rect: Rect, now: Ms) {
        let latest = self.setups.values().map(|s| s.used_ms.saturating_add(1)).max();
        let used_ms = latest.map_or(now, |l| now.max(l));
        self.setups.insert(signature.to_owned(), Saved { rect, used_ms });
        while self.setups.len() > KEEP_SETUPS {
            let oldest = self.setups.iter().min_by_key(|(_, s)| s.used_ms).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.setups.remove(&k),
                None => break,
            };
        }
    }
}

/// Where a window of `size` goes for a saved rect: on the work area the rect is on, with the
/// corner nearest the screen edges kept. `None` when that area is gone.
pub fn restore_position(saved: Rect, (w, h): (i32, i32), areas: &[Rect]) -> Option<(i32, i32)> {
    let area = area_for(saved, areas)?;
    Some(anchored_position_with(saved, w, h, area, nearest_anchor(saved, area)))
}

/// Whether the current position is one to remember.
pub fn savable(dock: DockState, view: ViewMode) -> bool {
    dock != DockState::SlidOut && dock::collapsible(view)
}

/// What a poll does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Put the window where it was for this setup.
    Restore,
    /// Remember this rect for the current setup.
    Save(Rect),
    Wait,
}

/// The poll's memory of the setup and of what it saved.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    signature: Option<String>,
    /// A restore is due (a new setup, or startup).
    pending: bool,
    /// Polls the new setup must still hold before the restore.
    settle: u8,
    saved: Option<Rect>,
}

impl Tracker {
    /// Takes the current setup and position as they are (the setting was just switched on).
    pub fn adopt(&mut self, signature: &str) {
        *self = Self { signature: Some(signature.to_owned()), ..Self::default() };
    }

    pub fn step(&mut self, signature: &str, rect: Rect, savable: bool) -> Step {
        if self.signature.as_deref() != Some(signature) {
            // Startup restores at once; a change waits one poll for the setup to settle.
            let settle = u8::from(self.signature.is_some());
            *self = Self { signature: Some(signature.to_owned()), pending: true, settle, saved: None };
        }
        if self.pending {
            if self.settle > 0 {
                self.settle -= 1;
                return Step::Wait;
            }
            if !savable {
                return Step::Wait;
            }
            self.pending = false;
            return Step::Restore;
        }
        if savable && self.saved != Some(rect) {
            self.saved = Some(rect);
            return Step::Save(rect);
        }
        Step::Wait
    }
}

struct Poller {
    path: PathBuf,
    file: PositionsFile,
    tracker: Tracker,
    was_enabled: bool,
}

fn monitors(window: &WebviewWindow) -> Vec<MonitorInfo> {
    window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            MonitorInfo { rect: (p.x, p.y, s.width as i32, s.height as i32), scale: m.scale_factor() }
        })
        .collect()
}

impl Poller {
    fn poll(&mut self, app: &AppHandle, shared: &Shared) {
        let enabled = shared.settings().per_display_position;
        let was_enabled = std::mem::replace(&mut self.was_enabled, enabled);
        if !enabled {
            return;
        }
        // Window queries wait for the main thread, so no lock may be held across them.
        let Some(window) = crate::window::get(app) else { return };
        let monitors = monitors(&window);
        let Some((rect, _)) = visible_rect(&window).filter(|_| !monitors.is_empty()) else { return };
        let signature = signature(&monitors);
        if !was_enabled {
            self.tracker.adopt(&signature);
        }
        let view = shared.ui().view;
        match self.tracker.step(&signature, rect, savable(DockState::now(shared), view)) {
            Step::Restore => {
                let saved = self.file.setups.get(&signature).map(|s| s.rect);
                let handle = app.clone();
                // Placing the window queries it, which waits for the main thread, and the dock
                // takes its placement lock there too; run it on the main thread (at once when
                // already on it) so the two can never wait on each other.
                let _ = app.run_on_main_thread(move || {
                    if let Some(window) = crate::window::get(&handle) {
                        restore(&window, saved);
                    }
                });
            }
            Step::Save(rect) => {
                self.file.remember(&signature, rect, now_ms());
                if let Err(e) = save_json(&self.path, &self.file) {
                    crate::pipeline::log(&format!("positions.json write failed: {e}"));
                }
            }
            Step::Wait => {}
        }
    }
}

/// Puts the window where it was for this setup (`saved`, a visible rect). Main thread only.
fn restore(window: &WebviewWindow, saved: Option<Rect>) {
    if let (Some(saved), Some((current, (dx, dy)))) = (saved, visible_rect(window)) {
        if let Some((x, y)) = restore_position(saved, (current.2, current.3), &work_areas(window)) {
            let _ = window.set_position(PhysicalPosition::new(x - dx, y - dy));
        }
    }
    // A first-seen setup keeps the window where it is (Windows moved it, or the window-state
    // plugin restored it); either way it must be on-screen, and a strip flush to its edge.
    crate::window::ensure_on_screen(window);
}

/// Wakes the poll (managed; the setting was switched).
struct Waker(Sender<()>);

/// Polls at once instead of at the end of the current period.
pub fn wake(app: &AppHandle) {
    if let Some(waker) = app.try_state::<Waker>() {
        let _ = waker.0.send(());
    }
}

/// Restores the position for the current setup (before the window is first shown) and starts
/// the poll.
pub fn start(app: &AppHandle, shared: Arc<Shared>) -> std::io::Result<()> {
    let mut poller = Poller {
        path: shared.paths.data_root().join(FILE_NAME),
        file: PositionsFile::default(),
        tracker: Tracker::default(),
        was_enabled: true,
    };
    poller.file = load_json(&poller.path);
    poller.poll(app, &shared);
    let (tx, rx) = mpsc::channel();
    app.manage(Waker(tx));
    let app = app.clone();
    std::thread::Builder::new().name("cuw-displays".into()).spawn(move || run(&app, &shared, poller, &rx))?;
    Ok(())
}

fn run(app: &AppHandle, shared: &Shared, mut poller: Poller, rx: &Receiver<()>) {
    loop {
        let period = if shared.settings().per_display_position { POLL_ON } else { POLL_OFF };
        if let Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(period) {
            break;
        }
        if shared.quitting.load(Ordering::SeqCst) {
            break;
        }
        poller.poll(app, shared);
        crate::tray::refresh_style(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: MonitorInfo = MonitorInfo { rect: (0, 0, 1920, 1080), scale: 1.25 };
    const DESK: MonitorInfo = MonitorInfo { rect: (1920, -200, 2560, 1440), scale: 1.0 };

    #[test]
    fn signature_is_order_free_and_scale_aware() {
        assert_eq!(signature(&[LAPTOP]), "0,0,1920x1080@125");
        assert_eq!(signature(&[LAPTOP, DESK]), signature(&[DESK, LAPTOP]));
        assert_eq!(signature(&[DESK, LAPTOP]), "0,0,1920x1080@125;1920,-200,2560x1440@100");
        let scaled = MonitorInfo { scale: 1.5, ..LAPTOP };
        assert_ne!(signature(&[scaled]), signature(&[LAPTOP]));
        let moved = MonitorInfo { rect: (-2560, 0, 2560, 1440), ..DESK };
        assert_ne!(signature(&[LAPTOP, moved]), signature(&[LAPTOP, DESK]), "arrangement counts");
    }

    #[test]
    fn remembers_per_setup_and_drops_the_least_recently_used() {
        let mut f = PositionsFile::default();
        f.remember("a", (1, 2, 3, 4), 10);
        f.remember("a", (5, 6, 7, 8), 11);
        assert_eq!(f.setups["a"], Saved { rect: (5, 6, 7, 8), used_ms: 11 });
        for i in 0..KEEP_SETUPS {
            f.remember(&format!("s{i}"), (0, 0, 1, 1), 100 + i as Ms);
        }
        assert_eq!(f.setups.len(), KEEP_SETUPS);
        assert!(!f.setups.contains_key("a"), "oldest dropped");
        assert!(f.setups.contains_key("s0"));
    }

    #[test]
    fn a_clock_set_back_never_drops_the_setup_just_saved() {
        let mut f = PositionsFile::default();
        for i in 0..KEEP_SETUPS {
            f.remember(&format!("s{i}"), (0, 0, 1, 1), 1_000 + i as Ms);
        }
        // The clock went back a day: the new setup is still the most recently used.
        f.remember("new", (1, 2, 3, 4), 1_000 - 86_400_000);
        assert_eq!(f.setups.len(), KEEP_SETUPS);
        assert!(f.setups.contains_key("new"), "the setup just saved survives");
        assert!(!f.setups.contains_key("s0"), "the oldest is dropped instead");
        f.remember("s1", (0, 0, 1, 1), 5);
        f.remember("newer", (0, 0, 1, 1), 6);
        assert!(f.setups.contains_key("s1") && !f.setups.contains_key("s2"), "order still follows use");
    }

    #[test]
    fn positions_file_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(FILE_NAME);
        assert_eq!(load_json::<PositionsFile>(&p), PositionsFile::default());
        let mut f = PositionsFile::default();
        f.remember(&signature(&[LAPTOP, DESK]), (2000, 100, 334, 239), 5);
        save_json(&p, &f).unwrap();
        assert_eq!(load_json::<PositionsFile>(&p), f);
    }

    #[test]
    fn restore_keeps_the_nearest_corner_on_the_saved_monitor() {
        let laptop = (0, 0, 1920, 1040);
        let desk = (1920, -200, 2560, 1400);
        // A card saved at the desk monitor's bottom right comes back as a pill in that corner.
        let card = (4140, 950, 320, 232);
        assert_eq!(restore_position(card, (240, 72), &[laptop, desk]), Some((4220, 1110)));
        // Same size: exactly where it was.
        assert_eq!(restore_position(card, (320, 232), &[laptop, desk]), Some((4140, 950)));
        // Top-left stays top-left.
        assert_eq!(restore_position((10, 10, 320, 232), (240, 72), &[laptop]), Some((10, 10)));
        // The saved monitor is not there.
        assert_eq!(restore_position(card, (320, 232), &[laptop]), None);
    }

    #[test]
    fn only_settled_pill_or_card_positions_are_saved() {
        use DockState::*;
        assert!(savable(Undocked, ViewMode::Card));
        assert!(savable(Undocked, ViewMode::Pill));
        assert!(savable(Strip, ViewMode::Card), "the strip is the docked edge position");
        assert!(!savable(SlidOut, ViewMode::Card));
        for panel in [ViewMode::Settings, ViewMode::Sessions, ViewMode::History] {
            assert!(!savable(Undocked, panel));
        }
    }

    const A: Rect = (100, 100, 320, 232);
    const B: Rect = (900, 500, 320, 232);

    #[test]
    fn startup_restores_then_saves_moves() {
        let mut t = Tracker::default();
        assert_eq!(t.step("one", A, true), Step::Restore);
        assert_eq!(t.step("one", A, true), Step::Save(A), "records the restored place");
        assert_eq!(t.step("one", A, true), Step::Wait);
        assert_eq!(t.step("one", B, true), Step::Save(B));
        assert_eq!(t.step("one", B, false), Step::Wait);
        assert_eq!(t.step("one", A, false), Step::Wait, "slid out or a panel: not saved");
        assert_eq!(t.step("one", A, true), Step::Save(A));
    }

    #[test]
    fn a_setup_change_settles_restores_and_never_saves_under_the_old_setup() {
        let mut t = Tracker::default();
        t.step("one", A, true);
        t.step("one", A, true);
        // A monitor went away and Windows moved the window: nothing is saved for "one".
        assert_eq!(t.step("two", B, true), Step::Wait);
        assert_eq!(t.step("two", B, true), Step::Restore);
        assert_eq!(t.step("two", A, true), Step::Save(A));
        // Back again: the same dance for "one".
        assert_eq!(t.step("one", B, true), Step::Wait);
        assert_eq!(t.step("one", B, true), Step::Restore);
    }

    #[test]
    fn a_restore_waits_until_the_position_is_settled() {
        let mut t = Tracker::default();
        assert_eq!(t.step("one", A, false), Step::Wait, "slid out at startup");
        assert_eq!(t.step("one", A, false), Step::Wait);
        assert_eq!(t.step("one", A, true), Step::Restore);
    }

    #[test]
    fn switching_the_setting_on_adopts_the_current_place() {
        let mut t = Tracker::default();
        t.adopt("one");
        assert_eq!(t.step("one", A, true), Step::Save(A), "no restore");
    }
}
