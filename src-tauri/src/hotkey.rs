//! Global hotkeys: `settings.hotkey` (default Ctrl+Alt+U) toggles click-through and
//! `settings.toggle_hotkey` (default Ctrl+Alt+H, "" = none) shows / hides the widget. A failed
//! registration (bad accelerator, another app owns it, or the other shortcut already is it) is
//! reported separately in `UiState.hotkey_error` / `UiState.toggle_hotkey_error`.

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

/// Managed state holding the [`Registered`] shortcuts.
struct RegisteredState(Mutex<Registered>);

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
    let Some(registered) = app.try_state::<RegisteredState>() else { return };
    let action = lock(&registered.0).action_for(shortcut);
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

/// The two shortcuts may not be the same (the second registration would fail with a confusing
/// "already registered"). The one the user just changed (`edited`) is refused, so the error
/// shows under the field they edited, and the other keeps working; at startup (`None`) the
/// show/hide one gives way. Returns the (click-through, show/hide) errors; `accels` are the
/// settings' (click-through, show/hide) strings.
pub fn duplicate_errors(
    click_through: Option<&Shortcut>,
    show_hide: Option<&Shortcut>,
    edited: Option<Action>,
    (click_accel, toggle_accel): (&str, &str),
) -> (Option<String>, Option<String>) {
    if show_hide.is_none() || show_hide != click_through {
        return (None, None);
    }
    match edited {
        Some(Action::ClickThrough) => (Some(format!("{} is already the show/hide shortcut", click_accel.trim())), None),
        Some(Action::ShowHide) | None => {
            (None, Some(format!("{} is already the click-through shortcut", toggle_accel.trim())))
        }
    }
}

/// Registers both saved shortcuts after the user changed `edited` in the settings.
pub fn register_edited(app: &AppHandle, shared: &Shared, edited: Action) {
    register_both(app, shared, Some(edited));
}

/// Registers both saved shortcuts (startup).
pub fn register_all(app: &AppHandle, shared: &Shared) {
    register_both(app, shared, None);
}

/// Updates both errors and emits `ui-state`.
fn register_both(app: &AppHandle, shared: &Shared, edited: Option<Action>) {
    let (click_accel, toggle_accel) = {
        let s = shared.settings();
        (s.hotkey.clone(), s.toggle_hotkey.clone())
    };
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut registered = Registered::default();
    let try_register = |accel: &str, shortcut: Shortcut| {
        gs.register(shortcut)
            .map(|()| shortcut)
            .map_err(|e| format!("{} could not be registered (already used by another app?): {e}", accel.trim()))
    };

    let click = parse(&click_accel);
    let toggle = parse(&toggle_accel);
    let (click_duplicate, toggle_duplicate) = duplicate_errors(
        click.as_ref().ok().and_then(Option::as_ref),
        toggle.as_ref().ok().and_then(Option::as_ref),
        edited,
        (&click_accel, &toggle_accel),
    );
    let click_error = match click {
        Ok(Some(s)) => {
            click_duplicate.or_else(|| try_register(&click_accel, s).map(|s| registered.click_through = Some(s)).err())
        }
        Ok(None) => None,
        Err(e) => Some(e),
    };
    let toggle_error = match toggle {
        Ok(Some(s)) => {
            toggle_duplicate.or_else(|| try_register(&toggle_accel, s).map(|s| registered.show_hide = Some(s)).err())
        }
        Ok(None) => None,
        Err(e) => Some(e),
    };
    match app.try_state::<RegisteredState>() {
        Some(state) => *lock(&state.0) = registered,
        None => {
            app.manage(RegisteredState(Mutex::new(registered)));
        }
    }
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
        let reg = Registered { click_through: Some(sc("Ctrl+Alt+U")), show_hide: Some(sc("Ctrl+Alt+H")) };
        assert_eq!(reg.action_for(&sc("Ctrl+Alt+U")), Some(Action::ClickThrough));
        assert_eq!(reg.action_for(&sc("Control+Alt+H")), Some(Action::ShowHide));
        assert_eq!(reg.action_for(&sc("Ctrl+Alt+J")), None);
        let none = Registered::default();
        assert_eq!(none.action_for(&sc("Ctrl+Alt+U")), None);
    }

    #[test]
    fn a_clash_is_reported_under_the_field_just_edited() {
        let h = sc("Ctrl+Alt+H");
        let accels = ("Control+Alt+H", " Ctrl+Alt+H ");
        // The click-through shortcut was changed to the show/hide one: refused under its own field.
        assert_eq!(
            duplicate_errors(Some(&h), Some(&h), Some(Action::ClickThrough), accels),
            (Some("Control+Alt+H is already the show/hide shortcut".to_owned()), None)
        );
        // The show/hide shortcut was changed to the click-through one.
        let show_hide_refused = (None, Some("Ctrl+Alt+H is already the click-through shortcut".to_owned()));
        assert_eq!(duplicate_errors(Some(&h), Some(&h), Some(Action::ShowHide), accels), show_hide_refused);
        // At startup (a hand-edited settings file) the show/hide shortcut gives way.
        assert_eq!(duplicate_errors(Some(&h), Some(&h), None, accels), show_hide_refused);
        // No clash.
        let u = sc("Ctrl+Alt+U");
        assert_eq!(duplicate_errors(Some(&u), Some(&h), Some(Action::ClickThrough), accels), (None, None));
        assert_eq!(duplicate_errors(None, Some(&h), Some(Action::ShowHide), accels), (None, None));
        assert_eq!(duplicate_errors(None, None, Some(Action::ClickThrough), ("", "")), (None, None));
    }
}
