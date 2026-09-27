//! Tauri commands (names and camelCase args exactly as listed in `src/lib/types.ts`) and the
//! shared actions the tray and hotkey reuse.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mikyas_core::engine::types::Snapshot;
use mikyas_core::time::now_ms;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

use crate::connect::{self, ConnectEnv, ConnectPreview, ConnectionStatus};
use crate::pipeline::Msg;
use crate::settings::{self, Settings, ViewMode};
use crate::state::{Shared, UiState, lock};
use crate::visibility::Event;

type Shr<'a> = State<'a, Arc<Shared>>;

#[tauri::command]
pub fn get_snapshot(shared: Shr<'_>) -> Snapshot {
    Snapshot::clone(&lock(&shared.snapshot))
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

/// Runs file and process work on a blocking worker, never on an async-runtime thread.
pub async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn connection_status(shared: Shr<'_>) -> Result<ConnectionStatus, String> {
    let paths = shared.paths.clone();
    blocking(move || Ok(connect::status(&paths))).await
}

/// The shell detection and self-test run on a blocking worker (they can take seconds).
#[tauri::command]
pub async fn connect_claude_code(shared: Shr<'_>, dry_run: bool) -> Result<ConnectPreview, String> {
    let shared = shared.inner().clone();
    blocking(move || {
        let _guard = lock(&shared.connect_lock);
        let env = ConnectEnv::detect(shared.paths.clone());
        if dry_run { connect::preview(&env, now_ms()) } else { connect::connect(&env, now_ms()) }
    })
    .await
}

#[tauri::command]
pub async fn disconnect_claude_code(shared: Shr<'_>) -> Result<ConnectionStatus, String> {
    let shared = shared.inner().clone();
    blocking(move || {
        let _guard = lock(&shared.connect_lock);
        connect::disconnect(&shared.paths, now_ms())
    })
    .await
}

#[tauri::command]
pub fn open_data_folder(shared: Shr<'_>) -> Result<(), String> {
    let dir = shared.paths.data_root().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// File name of the bundled third-party license notices (`bundle.resources` in tauri.conf.json).
pub const NOTICES_FILE: &str = "THIRD_PARTY_NOTICES.md";

/// Opens the bundled third-party license notices with the system's viewer for `.md` files,
/// through Windows' own explorer.exe by absolute path (the helper `open_url` uses). Nothing is
/// downloaded; only the file inside the app's resource folder can be opened.
#[tauri::command]
pub fn open_third_party_notices(app: AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let resources = app.path().resource_dir().map_err(|e| e.to_string())?;
    let windows = crate::updates::windows_dir().ok_or("The Windows folder was not found")?;
    let (exe, file) = notices_command(&resources, &windows)?;
    std::process::Command::new(exe).arg(file).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// The program and argument that open the notices: `<Windows>\explorer.exe <resources>\NOTICES_FILE`.
/// A missing file is an error rather than an explorer window on some other path.
pub fn notices_command(
    resources: &std::path::Path,
    windows: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let file = resources.join(NOTICES_FILE);
    if !file.is_file() {
        return Err(format!("{NOTICES_FILE} is missing from the app folder; reinstall to restore it"));
    }
    Ok((crate::updates::explorer_path(windows), file))
}

#[tauri::command]
pub fn quit_app(app: AppHandle, shared: Shr<'_>) {
    quit(&app, &shared);
}

/// The widget's × button (with `close_action` "hide").
#[tauri::command]
pub fn hide_widget(app: AppHandle, shared: Shr<'_>) {
    hide_from_widget(&app, &shared);
}

// ---- shared actions ----

pub fn apply_view(app: &AppHandle, shared: &Shared, view: ViewMode) {
    let from = shared.ui().view;
    crate::window::set_view(app, from, view);
    shared.ui().view = view;
    if matches!(view, ViewMode::Pill | ViewMode::Card) {
        let changed = {
            let mut s = shared.settings();
            (s.view != view).then(|| {
                s.view = view;
                s.clone()
            })
        };
        if let Some(s) = changed {
            save(shared, &s);
        }
    }
    crate::window::emit_ui(app, shared);
}

pub fn apply_pinned(app: &AppHandle, shared: &Shared, pinned: bool) {
    crate::window::set_pinned(app, pinned);
    shared.ui().pinned = pinned;
    let s = {
        let mut s = shared.settings();
        s.pinned = pinned;
        s.clone()
    };
    save(shared, &s);
    crate::window::emit_ui(app, shared);
}

/// Hides the widget from its own × or right-click menu, on the tray's and the hotkey's path
/// (`HiddenReason::User`); the first time, a toast says how to bring it back.
pub fn hide_from_widget(app: &AppHandle, shared: &Shared) {
    // The pointer is over a slid-out pill or card, so it would never slide back in on its own
    // and would come back slid out: back into the strip first (forced, like the card's "–").
    crate::dock::set_expanded(app, shared, false, true);
    crate::visibility::apply(app, shared, Event::UserHide);
    // A show/hide shortcut that failed to register can't bring it back.
    let hotkey_works = shared.ui().toggle_hotkey_error.is_none();
    let s = {
        let mut s = shared.settings();
        if !crate::visibility::hide_hint_due(Event::UserHide, s.hide_hint_shown) {
            return;
        }
        s.hide_hint_shown = true;
        s.clone()
    };
    save(shared, &s);
    let (title, body) = crate::toast::hide_hint_text(if hotkey_works { &s.toggle_hotkey } else { "" });
    crate::toast::show(app, &title, &body);
}

/// Ghost mode is never persisted.
pub fn apply_click_through_toggle(app: &AppHandle, shared: &Shared) {
    let on = !shared.ui().click_through;
    let effect = shared.settings().effect;
    crate::window::set_click_through(app, on, effect);
    shared.ui().click_through = on;
    crate::window::emit_ui(app, shared);
    // Ghost mode pauses fullscreen auto-hide; re-evaluate now rather than at the next poll.
    crate::fullscreen::wake(app);
}

/// Validates the patch and runs `side_effect(old, new)` (e.g. the autostart registration), and
/// only when both succeed replaces the settings: a patch applies completely or not at all.
fn commit_patch(
    shared: &Shared,
    patch: &serde_json::Value,
    side_effect: impl FnOnce(&Settings, &Settings) -> Result<(), String>,
) -> Result<(Settings, Settings), String> {
    let mut s = shared.settings();
    let old = s.clone();
    let new = settings::apply_patch(&old, patch)?;
    side_effect(&old, &new)?;
    *s = new.clone();
    Ok((old, new))
}

/// Validates, persists and applies a settings patch.
pub fn apply_settings_patch(app: &AppHandle, shared: &Shared, patch: &serde_json::Value) -> Result<Settings, String> {
    let (old, new) = commit_patch(shared, patch, |old, new| {
        if new.start_with_windows == old.start_with_windows {
            return Ok(());
        }
        let autolaunch = app.autolaunch();
        let result = if new.start_with_windows { autolaunch.enable() } else { autolaunch.disable() };
        result.map_err(|e| format!("could not change Start with Windows: {e}"))
    })?;
    save(shared, &new);
    if new.pinned != old.pinned {
        crate::window::set_pinned(app, new.pinned);
        shared.ui().pinned = new.pinned;
    }
    if new.effect != old.effect {
        let ghost = shared.ui().click_through;
        crate::window::set_click_through(app, ghost, new.effect);
    }
    if new.hotkey != old.hotkey {
        crate::hotkey::register_edited(app, shared, crate::hotkey::Action::ClickThrough);
    }
    // Appearance (ui_scale, card_rows, dock, ...) is applied by the window module.
    crate::window::on_settings_changed(app, shared, &old, &new);

    // A changed `hotkey` above already re-registered both shortcuts.
    if new.toggle_hotkey != old.toggle_hotkey && new.hotkey == old.hotkey {
        crate::hotkey::register_edited(app, shared, crate::hotkey::Action::ShowHide);
    }
    if new.auto_hide_fullscreen != old.auto_hide_fullscreen {
        crate::fullscreen::wake(app);
    }
    if new.check_updates != old.check_updates {
        crate::updates::wake(app);
    }

    let view_patched = patch.get("view").is_some() && new.view != shared.ui().view;
    if view_patched {
        apply_view(app, shared, new.view);
    }
    // The pipeline reads many settings (snapshot inputs and every alert switch); a tick is
    // cheap, so any change re-runs it.
    if new != old {
        shared.send(Msg::SettingsChanged);
    }
    crate::window::emit_ui(app, shared);
    Ok(shared.settings().clone())
}

/// Callers pass a copy, so the settings lock is not held during the write.
fn save(shared: &Shared, settings: &Settings) {
    if let Err(e) = settings::save(&shared.paths.settings_file(), settings) {
        crate::diag::log(&format!("settings.json write failed: {e}"));
    }
}

pub fn quit(app: &AppHandle, shared: &Shared) {
    shared.quitting.store(true, Ordering::SeqCst);
    crate::dock::collapse_before_save(app, shared);
    let _ = app.save_window_state(StateFlags::POSITION);
    shared.send(Msg::Shutdown);
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Dirty, PipelineState};
    use mikyas_core::paths::Paths;

    fn shared() -> (tempfile::TempDir, Shared) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_roots(tmp.path().join(".claude"), vec![], tmp.path().join("data"));
        let out = PipelineState::new(paths.clone()).tick(1_000, &Settings::default(), &Dirty::default());
        let shared = Shared::new(paths, Settings::default(), Snapshot::clone(&out.snapshot));
        (tmp, shared)
    }

    #[test]
    fn notices_open_only_the_bundled_file_through_explorer() {
        let tmp = tempfile::tempdir().unwrap();
        let windows = std::path::Path::new(r"C:\Windows");
        assert!(notices_command(tmp.path(), windows).unwrap_err().contains(NOTICES_FILE), "missing file");
        std::fs::write(tmp.path().join(NOTICES_FILE), "# notices\n").unwrap();
        let (exe, file) = notices_command(tmp.path(), windows).unwrap();
        assert_eq!(exe, windows.join("explorer.exe"));
        assert_eq!(file, tmp.path().join(NOTICES_FILE));
    }

    /// The installer ships the notices next to the app, where `resource_dir()` finds them, and the
    /// file exists at the repository root.
    #[test]
    fn notices_are_bundled_as_a_resource() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["bundle"]["resources"]["../THIRD_PARTY_NOTICES.md"], NOTICES_FILE);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let text = std::fs::read_to_string(root.join(NOTICES_FILE)).unwrap();
        assert!(text.contains("MPL-2.0") && text.contains("svelte"), "Rust crates and the npm runtime are covered");
    }

    #[test]
    fn a_failed_side_effect_leaves_the_settings_untouched() {
        let (_t, shared) = shared();
        let patch = serde_json::json!({"start_with_windows": true, "opacity": 0.5});
        let err = commit_patch(&shared, &patch, |_, _| Err("registry refused".into())).unwrap_err();
        assert_eq!(err, "registry refused");
        assert_eq!(*shared.settings(), Settings::default(), "nothing of the patch applied");

        let (old, new) = commit_patch(&shared, &patch, |_, _| Ok(())).unwrap();
        assert_eq!(old, Settings::default());
        assert!(new.start_with_windows && new.opacity == 0.5);
        assert_eq!(*shared.settings(), new);
        assert!(commit_patch(&shared, &serde_json::json!({"pinned": "yes"}), |_, _| Ok(())).is_err());
        assert_eq!(*shared.settings(), new);
    }
}
