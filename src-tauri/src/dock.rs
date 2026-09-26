//! Edge-dock mode: the widget tucks into a thin strip on a screen edge and slides out on hover.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::state::Shared;

/// Called by the UI on pointer enter/leave of the docked widget.
/// TODO(stream B): implement (resize/position the window, update `UiState.dock_expanded`, emit ui-state).
#[tauri::command]
pub fn set_dock_expanded(app: AppHandle, shared: State<'_, Arc<Shared>>, expanded: bool) {
    let _ = (app, shared, expanded);
}
