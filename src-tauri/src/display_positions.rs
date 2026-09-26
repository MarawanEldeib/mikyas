//! Per-display widget position: one remembered position per monitor setup.
//!
//! TODO(stream shell): implement.
//! - Active while `settings.per_display_position` is on; the window-state plugin's position stays
//!   the fallback for a setup seen for the first time.
//! - Signature of a setup = the sorted monitor rects plus scale factors.
//! - Stored in `<data_root>/positions.json`: signature → position (a docked widget stores its
//!   edge position). Saved when the user moves the widget.
//! - Restored on startup and when the monitor setup changes (polled every 3 s with window
//!   queries only).

use std::sync::Arc;

use tauri::AppHandle;

use crate::state::Shared;

/// Starts the monitor-setup poll. TODO(stream shell).
pub fn start(app: &AppHandle, shared: Arc<Shared>) -> std::io::Result<()> {
    let _ = (app, shared);
    Ok(())
}
