//! Opt-in update checker: the app's ONLY network access, off by default (`check_updates`).
//! Queries the GitHub Releases API of the project repository at most once a day.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::state::{Shared, UpdateInfo};

/// Checks now (explicit user action; allowed even when the daily check is off).
/// TODO(stream C): implement.
#[tauri::command]
pub async fn check_updates_now(app: AppHandle, shared: State<'_, Arc<Shared>>) -> Result<Option<UpdateInfo>, String> {
    let _ = (app, shared);
    Err("update check not implemented yet".into())
}

/// Opens a release page in the default browser. Only `https://github.com/<repo>/releases…` URLs.
/// TODO(stream C): implement with an allowlist.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let _ = url;
    Err("not implemented yet".into())
}
