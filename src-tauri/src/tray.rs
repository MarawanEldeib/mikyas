//! Tray icon: colour by usage, tooltip "5h 22% · 7d 61%", and the menu. Left click and the
//! Show/Hide item share the show/hide hotkey's path (`visibility`), and the item's label follows
//! `UiState.hidden_reason`. The menu handler also serves the widget's right-click menu
//! (`context_menu`): Tauri passes it every menu event.

use std::sync::{Arc, Mutex};

use sovawatch_core::engine::types::Snapshot;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::settings::{TrayNumber, ViewMode};
use crate::state::{HiddenReason, Shared};
pub use crate::tray_icon::Level;
use crate::tray_icon::{Style, tray_values};
use crate::visibility::{self, Event};

pub const TRAY_ID: &str = "main";

const ICON_GREEN: &[u8] = include_bytes!("../icons/tray-green.png");
const ICON_ORANGE: &[u8] = include_bytes!("../icons/tray-orange.png");
const ICON_RED: &[u8] = include_bytes!("../icons/tray-red.png");
const ICON_GREY: &[u8] = include_bytes!("../icons/tray-grey.png");

/// Handles to menu items whose state mirrors the app.
pub struct TrayItems {
    show_hide: MenuItem<Wry>,
    pin: CheckMenuItem<Wry>,
    click_through: CheckMenuItem<Wry>,
    compact: MenuItem<Wry>,
    autostart: CheckMenuItem<Wry>,
    /// What the icon was last drawn with (the number's mode and style).
    drawn: Mutex<Option<(TrayNumber, Style)>>,
}

/// Highest usage among fresh five-hour / weekly windows → colour (statusline thresholds:
/// green < 40, orange 40–69, red ≥ 70). No fresh data → grey.
pub fn level(snapshot: &Snapshot) -> Level {
    tray_values(snapshot)
        .filter(|v| !v.stale)
        .map(|v| v.pct)
        .max()
        .map_or(Level::Grey, |pct| Level::for_pct(f32::from(pct)))
}

/// "5h 22% · 7d 61%" (stale values marked with a trailing "?").
pub fn tooltip(snapshot: &Snapshot) -> String {
    let parts: Vec<String> = tray_values(snapshot)
        .map(|v| format!("{} {}%{}", v.kind.short(), v.pct, if v.stale { "?" } else { "" }))
        .collect();
    if parts.is_empty() { "SovaWatch — no data yet".into() } else { parts.join(" · ") }
}

fn icon(level: Level) -> Option<Image<'static>> {
    let bytes = match level {
        Level::Green => ICON_GREEN,
        Level::Orange => ICON_ORANGE,
        Level::Red => ICON_RED,
        Level::Grey => ICON_GREY,
    };
    Image::from_bytes(bytes).ok()
}

pub fn create(app: &AppHandle, shared: &Shared) -> tauri::Result<()> {
    let settings = shared.settings().clone();
    let ui = shared.ui().clone();
    let show_hide = MenuItem::with_id(app, "show", show_hide_label(ui.hidden_reason), true, None::<&str>)?;
    let pin = CheckMenuItem::with_id(app, "pin", "Pin on top", true, ui.pinned, None::<&str>)?;
    let click_through =
        CheckMenuItem::with_id(app, "click_through", "Click-through", true, ui.click_through, None::<&str>)?;
    let compact = MenuItem::with_id(app, "compact", compact_label(ui.view), true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start with Windows",
        true,
        settings.start_with_windows,
        None::<&str>,
    )?;
    let disconnect = MenuItem::with_id(app, "disconnect", "Disconnect Claude Code", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);

    let menu = Menu::new(app)?;
    menu.append_items(&[&show_hide, &pin, &click_through, &compact, &settings_item, &sep()?, &autostart, &disconnect])?;
    #[cfg(debug_assertions)]
    {
        use tauri::menu::Submenu;
        let simulate = Submenu::with_id_and_items(
            app,
            "simulate",
            "Simulate",
            true,
            &[
                &MenuItem::with_id(app, "sim_80", "80% alert", true, None::<&str>)?,
                &MenuItem::with_id(app, "sim_95", "95% alert", true, None::<&str>)?,
                &MenuItem::with_id(app, "sim_reset", "Reset alert", true, None::<&str>)?,
            ],
        )?;
        menu.append_items(&[&sep()?, &simulate])?;
    }
    menu.append_items(&[&sep()?, &quit])?;

    app.manage(TrayItems { show_hide, pin, click_through, compact, autostart, drawn: Mutex::new(None) });

    let snapshot = crate::state::lock(&shared.snapshot).clone();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip(&snapshot))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                let app = tray.app_handle();
                let shared = app.state::<Arc<Shared>>().inner().clone();
                visibility::apply(app, &shared, Event::UserToggle);
            }
        });
    if let Some(img) = tray_image(app, &snapshot, settings.tray_number) {
        builder = builder.icon(img);
    }
    builder.build(app)?;
    Ok(())
}

pub fn compact_label(view: ViewMode) -> &'static str {
    if view == ViewMode::Pill { "Expanded" } else { "Compact" }
}

fn show_hide_label(reason: HiddenReason) -> &'static str {
    if reason == HiddenReason::None { "Hide widget" } else { "Show widget" }
}

/// Tray and right-click menu items (the ids "hide" and "history" are the right-click menu's).
fn on_menu(app: &AppHandle, event: MenuEvent) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    match event.id().as_ref() {
        "show" => visibility::apply(app, &shared, Event::UserToggle),
        "hide" => crate::commands::hide_from_widget(app, &shared),
        "pin" => {
            let pinned = !shared.ui().pinned;
            crate::commands::apply_pinned(app, &shared, pinned);
        }
        "click_through" => crate::commands::apply_click_through_toggle(app, &shared),
        "compact" => {
            let view = if shared.ui().view == ViewMode::Pill { ViewMode::Card } else { ViewMode::Pill };
            visibility::apply(app, &shared, Event::UserShow);
            crate::commands::apply_view(app, &shared, view);
        }
        id @ ("settings" | "history") => {
            let view = if id == "settings" { ViewMode::Settings } else { ViewMode::History };
            visibility::apply(app, &shared, Event::UserShow);
            crate::commands::apply_view(app, &shared, view);
        }
        "autostart" => {
            let on = !shared.settings().start_with_windows;
            let patch = serde_json::json!({ "start_with_windows": on });
            if let Err(e) = crate::commands::apply_settings_patch(app, &shared, &patch) {
                crate::pipeline::log(&e);
            }
            sync_checks(app, &shared);
        }
        "disconnect" => {
            let paths = shared.paths.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                let shared = app.state::<Arc<Shared>>().inner().clone();
                let _guard = crate::state::lock(&shared.connect_lock);
                let result = crate::connect::disconnect(&paths, sovawatch_core::time::now_ms());
                let body = match result {
                    Ok(_) => "Claude Code's statusline was restored.".to_owned(),
                    Err(e) => format!("Disconnect failed: {e}"),
                };
                crate::notify::show(&app, "SovaWatch", &body);
            });
        }
        "quit" => crate::commands::quit(app, &shared),
        #[cfg(debug_assertions)]
        id @ ("sim_80" | "sim_95" | "sim_reset") => crate::notify::simulate(app, id),
        _ => {}
    }
}

/// The number icon (`settings.tray_number`), or the coloured dot when it is off or there is no data.
fn tray_image(app: &AppHandle, snapshot: &Snapshot, mode: TrayNumber) -> Option<Image<'static>> {
    let style = Style::current(app);
    if let Some(items) = app.try_state::<TrayItems>() {
        *crate::state::lock(&items.drawn) = Some((mode, style));
    }
    crate::tray_icon::number_icon(snapshot, mode, style).or_else(|| icon(level(snapshot)))
}

fn mode(app: &AppHandle) -> TrayNumber {
    app.try_state::<Arc<Shared>>().map_or(TrayNumber::Off, |shared| shared.settings().tray_number)
}

/// Refreshes the tooltip and icon from a new snapshot.
pub fn update(app: &AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_tooltip(Some(tooltip(snapshot)));
    let _ = tray.set_icon(tray_image(app, snapshot, mode(app)));
}

/// Redraws the icon when the number's setting, the display scale or the taskbar theme changed
/// since it was drawn (called from the settings hook and the display poll).
pub fn refresh_style(app: &AppHandle) {
    let now = (mode(app), Style::current(app));
    let unchanged = app.try_state::<TrayItems>().is_some_and(|items| *crate::state::lock(&items.drawn) == Some(now));
    if unchanged {
        return;
    }
    let Some(shared) = app.try_state::<Arc<Shared>>() else { return };
    let snapshot = crate::state::lock(&shared.snapshot).clone();
    update(app, &snapshot);
}

/// Mirrors visibility / pin / click-through / view / autostart into the menu.
pub fn sync_checks(app: &AppHandle, shared: &Shared) {
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let ui = shared.ui().clone();
    let autostart = shared.settings().start_with_windows;
    let _ = items.show_hide.set_text(show_hide_label(ui.hidden_reason));
    let _ = items.pin.set_checked(ui.pinned);
    let _ = items.click_through.set_checked(ui.click_through);
    let _ = items.compact.set_text(compact_label(ui.view));
    let _ = items.autostart.set_checked(autostart);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sovawatch_core::engine::types::{
        DesktopHealth, Phase, ResetInfo, Source, SourceHealth, WindowKind, WindowState, WindowView,
    };

    fn snap(values: &[(WindowKind, f32, bool)]) -> Snapshot {
        Snapshot {
            generated_ms: 0,
            windows: values
                .iter()
                .map(|(k, p, stale)| WindowView {
                    state: WindowState {
                        kind: k.clone(),
                        pct: *p,
                        reset: ResetInfo::Unknown,
                        source: Source::Desktop,
                        observed_at_ms: 0,
                        stale: *stale,
                        limit_reached: false,
                        phase: Phase::Active,
                    },
                    burn: None,
                    spark: vec![],
                    worked_since: false,
                })
                .collect(),
            session: None,
            sessions: vec![],
            health: SourceHealth {
                desktop: DesktopHealth::NotFound,
                cli_last_capture_ms: None,
                transcripts_last_activity_ms: None,
            },
            warnings: vec![],
        }
    }

    #[test]
    fn tooltip_and_level() {
        let s = snap(&[(WindowKind::FiveHour, 22.4, false), (WindowKind::SevenDay, 61.0, false)]);
        assert_eq!(tooltip(&s), "5h 22% · 7d 61%");
        assert_eq!(level(&s), Level::Orange);
        assert_eq!(level(&snap(&[(WindowKind::FiveHour, 70.0, false)])), Level::Red);
        assert_eq!(level(&snap(&[(WindowKind::FiveHour, 39.0, false)])), Level::Green);
        assert_eq!(level(&snap(&[(WindowKind::FiveHour, 90.0, true)])), Level::Grey);
        assert_eq!(level(&snap(&[])), Level::Grey);
        assert_eq!(tooltip(&snap(&[])), "SovaWatch — no data yet");
        assert_eq!(tooltip(&snap(&[(WindowKind::FiveHour, 5.0, true)])), "5h 5%?");
        assert_eq!(show_hide_label(HiddenReason::None), "Hide widget");
        assert_eq!(show_hide_label(HiddenReason::User), "Show widget");
        assert_eq!(show_hide_label(HiddenReason::Fullscreen), "Show widget");
        for l in [Level::Green, Level::Orange, Level::Red, Level::Grey] {
            assert!(icon(l).is_some());
        }
    }

    #[test]
    fn dot_tooltip_and_number_agree_on_a_reached_limit() {
        let mut s = snap(&[(WindowKind::FiveHour, 95.0, false), (WindowKind::SevenDay, 20.0, false)]);
        s.windows[0].state.limit_reached = true;
        s.windows[1].state.pct = 20.4;
        assert_eq!(level(&s), Level::Red);
        assert_eq!(tooltip(&s), "5h 100% · 7d 20%");
        let r = crate::tray_icon::reading(&s, TrayNumber::Worst).unwrap();
        assert_eq!((r.pct, r.level), (100, Level::Red));
        // Out-of-range values are clamped the same way everywhere.
        let over = snap(&[(WindowKind::FiveHour, 130.0, false)]);
        assert_eq!(tooltip(&over), "5h 100%");
        let under = snap(&[(WindowKind::FiveHour, -4.0, false)]);
        assert_eq!(tooltip(&under), "5h 0%");
        assert_eq!(level(&under), Level::Green);
        // The dot follows the number shown: 39.6 shows as 40, orange.
        assert_eq!(level(&snap(&[(WindowKind::FiveHour, 39.6, false)])), Level::Orange);
    }

    #[test]
    fn level_thresholds() {
        assert_eq!(Level::for_pct(0.0), Level::Green);
        assert_eq!(Level::for_pct(39.9), Level::Green);
        assert_eq!(Level::for_pct(40.0), Level::Orange);
        assert_eq!(Level::for_pct(69.9), Level::Orange);
        assert_eq!(Level::for_pct(70.0), Level::Red);
        assert_eq!(Level::for_pct(100.0), Level::Red);
    }
}
