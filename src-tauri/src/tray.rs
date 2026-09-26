//! Tray icon: colour by usage, tooltip "5h 22% · 7d 61%", and the menu.

use std::sync::Arc;

use cuw_core::engine::types::{Phase, Snapshot, WindowKind};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::settings::ViewMode;
use crate::state::Shared;

pub const TRAY_ID: &str = "main";

const ICON_GREEN: &[u8] = include_bytes!("../icons/tray-green.png");
const ICON_ORANGE: &[u8] = include_bytes!("../icons/tray-orange.png");
const ICON_RED: &[u8] = include_bytes!("../icons/tray-red.png");
const ICON_GREY: &[u8] = include_bytes!("../icons/tray-grey.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Green,
    Orange,
    Red,
    Grey,
}

/// Handles to menu items whose state mirrors the app.
pub struct TrayItems {
    pin: CheckMenuItem<Wry>,
    click_through: CheckMenuItem<Wry>,
    compact: MenuItem<Wry>,
    autostart: CheckMenuItem<Wry>,
}

/// Highest usage among fresh five-hour / weekly windows → colour (statusline thresholds:
/// green < 40, orange 40–69, red ≥ 70). No fresh data → grey.
pub fn level(snapshot: &Snapshot) -> Level {
    let max = snapshot
        .windows
        .iter()
        .filter(|w| matches!(w.state.kind, WindowKind::FiveHour | WindowKind::SevenDay))
        .filter(|w| !w.state.stale)
        .map(|w| if w.state.phase == Phase::ResetAwaitingData { 0.0 } else { w.state.pct })
        .fold(None, |m: Option<f32>, p| Some(m.map_or(p, |m| m.max(p))));
    match max {
        None => Level::Grey,
        Some(p) if p >= 70.0 => Level::Red,
        Some(p) if p >= 40.0 => Level::Orange,
        Some(_) => Level::Green,
    }
}

/// "5h 22% · 7d 61%" (stale values marked with a trailing "?").
pub fn tooltip(snapshot: &Snapshot) -> String {
    let parts: Vec<String> = snapshot
        .windows
        .iter()
        .filter(|w| matches!(w.state.kind, WindowKind::FiveHour | WindowKind::SevenDay))
        .map(|w| {
            let pct = if w.state.phase == Phase::ResetAwaitingData { 0.0 } else { w.state.pct };
            let stale = if w.state.stale { "?" } else { "" };
            format!("{} {:.0}%{stale}", w.state.kind.short(), pct)
        })
        .collect();
    if parts.is_empty() {
        "Claude Usage — no data yet".into()
    } else {
        parts.join(" · ")
    }
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
    let show_hide = MenuItem::with_id(app, "show", "Show / Hide", true, None::<&str>)?;
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

    let snapshot = crate::state::lock(&shared.snapshot).clone();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip(&snapshot))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::window::toggle_visible(tray.app_handle());
            }
        });
    if let Some(img) = icon(level(&snapshot)) {
        builder = builder.icon(img);
    }
    builder.build(app)?;
    app.manage(TrayItems {
        pin,
        click_through,
        compact,
        autostart,
    });
    Ok(())
}

fn compact_label(view: ViewMode) -> &'static str {
    if view == ViewMode::Pill { "Expanded" } else { "Compact" }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    match event.id().as_ref() {
        "show" => crate::window::toggle_visible(app),
        "pin" => {
            let pinned = !shared.ui().pinned;
            crate::commands::apply_pinned(app, &shared, pinned);
        }
        "click_through" => crate::commands::apply_click_through_toggle(app, &shared),
        "compact" => {
            let view = if shared.ui().view == ViewMode::Pill { ViewMode::Card } else { ViewMode::Pill };
            crate::window::show(app);
            crate::commands::apply_view(app, &shared, view);
        }
        "settings" => {
            crate::window::show(app);
            crate::commands::apply_view(app, &shared, ViewMode::Settings);
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
                let result = crate::connect::disconnect(&paths, cuw_core::time::now_ms());
                let body = match result {
                    Ok(_) => "Claude Code's statusline was restored.".to_owned(),
                    Err(e) => format!("Disconnect failed: {e}"),
                };
                crate::notify::show(&app, "Claude Usage Widget", &body);
            });
        }
        "quit" => crate::commands::quit(app, &shared),
        #[cfg(debug_assertions)]
        id @ ("sim_80" | "sim_95" | "sim_reset") => crate::notify::simulate(app, id),
        _ => {}
    }
}

/// Refreshes the tooltip and icon from a new snapshot.
pub fn update(app: &AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_tooltip(Some(tooltip(snapshot)));
    let _ = tray.set_icon(icon(level(snapshot)));
}

/// Mirrors pin / click-through / view / autostart into the menu.
pub fn sync_checks(app: &AppHandle, shared: &Shared) {
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let ui = shared.ui().clone();
    let autostart = shared.settings().start_with_windows;
    let _ = items.pin.set_checked(ui.pinned);
    let _ = items.click_through.set_checked(ui.click_through);
    let _ = items.compact.set_text(compact_label(ui.view));
    let _ = items.autostart.set_checked(autostart);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuw_core::engine::types::{DesktopHealth, ResetInfo, Source, SourceHealth, WindowState, WindowView};

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
        assert_eq!(tooltip(&snap(&[])), "Claude Usage — no data yet");
        assert_eq!(tooltip(&snap(&[(WindowKind::FiveHour, 5.0, true)])), "5h 5%?");
        for l in [Level::Green, Level::Orange, Level::Red, Level::Grey] {
            assert!(icon(l).is_some());
        }
    }
}
