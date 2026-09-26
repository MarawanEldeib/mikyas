//! Global hotkey (default Ctrl+Alt+U) that toggles click-through. A failed registration (bad
//! accelerator, or another app owns it) is reported in `UiState.hotkey_error`.

use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::state::Shared;

/// Plugin handler: fires on key press only (the plugin reports press and release).
pub fn handler(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let shared = app.state::<Arc<Shared>>().inner().clone();
    crate::commands::apply_click_through_toggle(app, &shared);
}

/// Replaces any registered hotkey with `accel`; updates `hotkey_error` and emits `ui-state`.
pub fn register(app: &AppHandle, shared: &Shared, accel: &str) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let error = if accel.trim().is_empty() {
        None
    } else {
        match accel.parse::<Shortcut>() {
            Err(e) => {
                // The parser's message ends with a "please report" link; keep the first clause.
                let detail = e.to_string();
                let detail = detail.split(", if you").next().unwrap_or_default().to_owned();
                Some(format!("\"{accel}\" is not a valid shortcut ({detail})"))
            }
            Ok(shortcut) => gs
                .register(shortcut)
                .err()
                .map(|e| format!("{accel} could not be registered (already used by another app?): {e}")),
        }
    };
    shared.ui().hotkey_error = error;
    crate::window::emit_ui(app, shared);
}
