//! Connection watchdog: notices when Claude Code's status line stops running the widget's
//! capture after Connect had wrapped it (another tool or the user rewrote `settings.json`).
//!
//! TODO(stream shell): implement.
//! - Runs while `settings.connection_watchdog` is on. Checks when Claude Code's `settings.json`
//!   changes (watched read-only, like the other watches) and every 5 min.
//! - Lost = `wrap.json` says we connected, and `connect::status` is no longer `Connected`. Then
//!   `UiState.connection_lost = true` (emit `ui-state`) and one [`crate::notify::Alert::ConnectionLost`]
//!   toast per change.
//! - The card's banner offers Reconnect (the normal connect, no preview; clears the flag) and
//!   Dismiss ([`dismiss_connection_warning`]), which records the dismissed change in
//!   `<data_root>/watchdog.json` so the same change never warns again.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::state::Shared;

/// Starts the watchdog. TODO(stream shell): watch + 5-min timer.
pub fn start(app: &AppHandle, shared: Arc<Shared>) -> std::io::Result<()> {
    let _ = (app, shared);
    Ok(())
}

/// The banner's Dismiss. TODO(stream shell): record the dismissed change in `watchdog.json`.
#[tauri::command]
pub fn dismiss_connection_warning(app: AppHandle, shared: State<'_, Arc<Shared>>) {
    shared.ui().connection_lost = false;
    crate::window::emit_ui(&app, &shared);
}
