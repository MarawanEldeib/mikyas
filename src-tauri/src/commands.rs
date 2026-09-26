//! Tauri commands (names and camelCase args exactly as listed in `src/lib/types.ts`) and the
//! shared actions the tray and hotkey reuse.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cuw_core::engine::types::Snapshot;
use cuw_core::time::now_ms;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

use crate::connect::{self, ConnectEnv, ConnectPreview, ConnectionStatus};
use crate::pipeline::Msg;
use crate::settings::{self, Settings, ViewMode};
use crate::state::{Shared, UiState, lock};

type Shr<'a> = State<'a, Arc<Shared>>;

#[tauri::command]
pub fn get_snapshot(shared: Shr<'_>) -> Snapshot {
    lock(&shared.snapshot).clone()
}

#[tauri::command]
pub fn get_settings(shared: Shr<'_>) -> Settings {
    shared.settings().clone()
}

#[tauri::command]
pub fn update_settings(app: AppHandle, shared: Shr<'_>, patch: serde_json::Value) -> Result<Settings, String> {
    apply_settings_patch(&app, &shared, &patch)
}

#[tauri::command]
pub fn get_ui_state(shared: Shr<'_>) -> UiState {
    shared.ui().clone()
}

#[tauri::command]
pub fn set_view(app: AppHandle, shared: Shr<'_>, view: ViewMode) {
    apply_view(&app, &shared, view);
}

#[tauri::command]
pub fn set_pinned(app: AppHandle, shared: Shr<'_>, pinned: bool) {
    apply_pinned(&app, &shared, pinned);
}

#[tauri::command]
pub fn toggle_click_through(app: AppHandle, shared: Shr<'_>) {
    apply_click_through_toggle(&app, &shared);
}

#[tauri::command]
pub async fn connection_status(shared: Shr<'_>) -> Result<ConnectionStatus, String> {
    Ok(connect::status(&shared.paths))
}

/// Async so the shell detection and self-test never block the UI thread.
#[tauri::command]
pub async fn connect_claude_code(shared: Shr<'_>, dry_run: bool) -> Result<ConnectPreview, String> {
    let _guard = lock(&shared.connect_lock);
    let env = ConnectEnv::detect(shared.paths.clone());
    if dry_run {
        connect::preview(&env, now_ms())
    } else {
        connect::connect(&env, now_ms())
    }
}

#[tauri::command]
pub async fn disconnect_claude_code(shared: Shr<'_>) -> Result<ConnectionStatus, String> {
    let _guard = lock(&shared.connect_lock);
    connect::disconnect(&shared.paths, now_ms())
}

#[tauri::command]
pub fn open_data_folder(shared: Shr<'_>) -> Result<(), String> {
    let dir = shared.paths.data_root().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn quit_app(app: AppHandle, shared: Shr<'_>) {
    quit(&app, &shared);
}

// ---- shared actions ----

pub fn apply_view(app: &AppHandle, shared: &Shared, view: ViewMode) {
    let from = shared.ui().view;
    crate::window::set_view(app, from, view);
    shared.ui().view = view;
    if matches!(view, ViewMode::Pill | ViewMode::Card) {
        let mut s = shared.settings();
        if s.view != view {
            s.view = view;
            save(shared, &s);
        }
    }
    crate::window::emit_ui(app, shared);
}

pub fn apply_pinned(app: &AppHandle, shared: &Shared, pinned: bool) {
    crate::window::set_pinned(app, pinned);
    shared.ui().pinned = pinned;
    {
        let mut s = shared.settings();
        s.pinned = pinned;
        save(shared, &s);
    }
    crate::window::emit_ui(app, shared);
}

/// Ghost mode is never persisted.
pub fn apply_click_through_toggle(app: &AppHandle, shared: &Shared) {
    let on = !shared.ui().click_through;
    let effect = shared.settings().effect;
    crate::window::set_click_through(app, on, effect);
    shared.ui().click_through = on;
    crate::window::emit_ui(app, shared);
}

/// Validates, persists and applies a settings patch.
pub fn apply_settings_patch(app: &AppHandle, shared: &Shared, patch: &serde_json::Value) -> Result<Settings, String> {
    let (old, new) = {
        let mut s = shared.settings();
        let old = s.clone();
        let new = settings::apply_patch(&old, patch)?;
        *s = new.clone();
        (old, new)
    };
    if new.start_with_windows != old.start_with_windows {
        let autolaunch = app.autolaunch();
        let result = if new.start_with_windows { autolaunch.enable() } else { autolaunch.disable() };
        if let Err(e) = result {
            shared.settings().start_with_windows = old.start_with_windows;
            return Err(format!("could not change Start with Windows: {e}"));
        }
    }
    save(shared, &shared.settings());
    if new.pinned != old.pinned {
        crate::window::set_pinned(app, new.pinned);
        shared.ui().pinned = new.pinned;
    }
    if new.effect != old.effect {
        let ghost = shared.ui().click_through;
        crate::window::set_click_through(app, ghost, new.effect);
    }
    if new.hotkey != old.hotkey {
        crate::hotkey::register(app, shared, &new.hotkey);
    }
    // [stream B] appearance hooks (ui_scale, card_rows, dock) go here.

    // [stream C] system hooks (toggle_hotkey, auto_hide_fullscreen, check_updates) go here.

    let view_patched = patch.get("view").is_some() && new.view != shared.ui().view;
    if view_patched {
        apply_view(app, shared, new.view);
    }
    if new.thresholds != old.thresholds
        || new.notify_reset != old.notify_reset
        || new.stale_min != old.stale_min
        || new.ctx_overrides != old.ctx_overrides
        || new.show_project != old.show_project
        || new.ctx_alerts != old.ctx_alerts
        || new.ctx_thresholds != old.ctx_thresholds
    {
        shared.send(Msg::SettingsChanged);
    }
    crate::window::emit_ui(app, shared);
    Ok(shared.settings().clone())
}

fn save(shared: &Shared, settings: &Settings) {
    if let Err(e) = settings::save(&shared.paths.settings_file(), settings) {
        crate::pipeline::log(&format!("settings.json write failed: {e}"));
    }
}

pub fn quit(app: &AppHandle, shared: &Shared) {
    shared.quitting.store(true, Ordering::SeqCst);
    let _ = app.save_window_state(StateFlags::POSITION);
    shared.send(Msg::Shutdown);
    app.exit(0);
}
