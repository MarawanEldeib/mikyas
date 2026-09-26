//! Global hotkeys: `settings.hotkey` (default Ctrl+Alt+U) toggles click-through and
//! `settings.toggle_hotkey` (default Ctrl+Alt+H, "" = none) shows / hides the widget. A failed
//! registration (bad accelerator, or another app owns it) is reported separately in
//! `UiState.hotkey_error` / `UiState.toggle_hotkey_error`.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::state::{Shared, lock};
use crate::visibility::{self, Event};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ClickThrough,
    ShowHide,
}

/// The shortcuts currently registered, so the handler knows which one fired.
#[derive(Debug, Default)]
struct Registered {
    click_through: Option<Shortcut>,
    show_hide: Option<Shortcut>,
}

static REGISTERED: Mutex<Registered> = Mutex::new(Registered {
    click_through: None,
    show_hide: None,
});

impl Registered {
    fn action_for(&self, pressed: &Shortcut) -> Option<Action> {
        if self.click_through.as_ref() == Some(pressed) {
            Some(Action::ClickThrough)
        } else if self.show_hide.as_ref() == Some(pressed) {
            Some(Action::ShowHide)
        } else {
            None
        }
    }
}

/// Plugin handler: fires on key press only (the plugin reports press and release).
pub fn handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let action = lock(&REGISTERED).action_for(shortcut);
    let shared = app.state::<Arc<Shared>>().inner().clone();
    match action {
        Some(Action::ClickThrough) => crate::commands::apply_click_through_toggle(app, &shared),
        Some(Action::ShowHide) => visibility::apply(app, &shared, Event::UserToggle),
        None => {}
    }
}

/// Parses an accelerator; `Ok(None)` for "" (no shortcut).
pub fn parse(accel: &str) -> Result<Option<Shortcut>, String> {
    let accel = accel.trim();
    if accel.is_empty() {
        return Ok(None);
    }
    accel.parse::<Shortcut>().map(Some).map_err(|e| {
        // The parser's message ends with a "please report" link; keep the first clause.
        let detail = e.to_string();
        let detail = detail.split(", if you").next().unwrap_or_default().to_owned();
        format!("\"{accel}\" is not a valid shortcut ({detail})")
    })
}

/// The show/hide shortcut may not repeat the click-through one (the second registration would
/// fail with a confusing "already registered").
pub fn duplicate_error(click_through: Option<&Shortcut>, show_hide: Option<&Shortcut>, accel: &str) -> Option<String> {
    (show_hide.is_some() && show_hide == click_through)
        .then(|| format!("{} is already the click-through shortcut", accel.trim()))
}

/// Registers the click-through shortcut `accel` (just saved in the settings) together with the
/// saved show/hide shortcut; updates both errors and emits `ui-state`.
pub fn register(app: &AppHandle, shared: &Shared, accel: &str) {
    let toggle = shared.settings().toggle_hotkey.clone();
    register_both(app, shared, accel, &toggle);
}

/// Registers both saved shortcuts (startup, or the show/hide shortcut changed).
pub fn register_all(app: &AppHandle, shared: &Shared) {
    let (click, toggle) = {
        let s = shared.settings();
        (s.hotkey.clone(), s.toggle_hotkey.clone())
    };
    register_both(app, shared, &click, &toggle);
}

fn register_both(app: &AppHandle, shared: &Shared, click_accel: &str, toggle_accel: &str) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut registered = Registered::default();
    let try_register = |accel: &str, shortcut: Shortcut| {
        gs.register(shortcut)
            .map(|()| shortcut)
            .map_err(|e| format!("{} could not be registered (already used by another app?): {e}", accel.trim()))
    };

    let click = parse(click_accel);
    let click_shortcut = click.as_ref().ok().copied().flatten();
    let click_error = match click {
        Ok(Some(s)) => try_register(click_accel, s).map(|s| registered.click_through = Some(s)).err(),
        Ok(None) => None,
        Err(e) => Some(e),
    };
    let toggle_error = match parse(toggle_accel) {
        Ok(Some(s)) => duplicate_error(click_shortcut.as_ref(), Some(&s), toggle_accel)
            .or_else(|| try_register(toggle_accel, s).map(|s| registered.show_hide = Some(s)).err()),
        Ok(None) => None,
        Err(e) => Some(e),
    };
    *lock(&REGISTERED) = registered;
    {
        let mut ui = shared.ui();
        ui.hotkey_error = click_error;
        ui.toggle_hotkey_error = toggle_error;
    }
    crate::window::emit_ui(app, shared);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(accel: &str) -> Shortcut {
        parse(accel).unwrap().unwrap()
    }

    #[test]
    fn parses_and_reports_bad_accelerators() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("   ").unwrap(), None);
        assert_eq!(parse("Ctrl+Alt+H").unwrap(), Some(sc("Control+Alt+KeyH")));
        let err = parse("Ctrl+Alt+Nope").unwrap_err();
        assert!(err.starts_with("\"Ctrl+Alt+Nope\" is not a valid shortcut ("), "{err}");
        assert!(!err.contains("if you"), "{err}");
    }

    #[test]
    fn the_handler_tells_the_shortcuts_apart() {
        let reg = Registered {
            click_through: Some(sc("Ctrl+Alt+U")),
            show_hide: Some(sc("Ctrl+Alt+H")),
        };
        assert_eq!(reg.action_for(&sc("Ctrl+Alt+U")), Some(Action::ClickThrough));
        assert_eq!(reg.action_for(&sc("Control+Alt+H")), Some(Action::ShowHide));
        assert_eq!(reg.action_for(&sc("Ctrl+Alt+J")), None);
        let none = Registered::default();
        assert_eq!(none.action_for(&sc("Ctrl+Alt+U")), None);
    }

    #[test]
    fn show_hide_may_not_repeat_click_through() {
        let u = sc("Ctrl+Alt+U");
        assert_eq!(
            duplicate_error(Some(&u), Some(&sc("Control+Alt+U")), "Control+Alt+U").as_deref(),
            Some("Control+Alt+U is already the click-through shortcut")
        );
        assert_eq!(duplicate_error(Some(&u), Some(&sc("Ctrl+Alt+H")), "Ctrl+Alt+H"), None);
        assert_eq!(duplicate_error(None, Some(&u), "Ctrl+Alt+U"), None);
        assert_eq!(duplicate_error(None, None, ""), None);
    }
}
