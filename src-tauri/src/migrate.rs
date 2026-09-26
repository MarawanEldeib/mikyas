//! The move from Claude Usage Widget (the app's former name) to SovaWatch, run once on the first
//! start, before anything else reads the data folder.
//!
//! When `%LOCALAPPDATA%\SovaWatch` has no `settings.json` and no [`MARKER_FILE`] yet, and
//! `%LOCALAPPDATA%\ClaudeUsageWidget` exists:
//! 1. the widget's own files ([`COPIED_FILES`] and the captures) are copied over. Nothing already
//!    in the new folder is overwritten, and the old folder is not deleted;
//! 2. if Claude Code's status line runs the old helper (`ClaudeUsageWidget\bin\cuw-capture.exe`),
//!    the new helper is installed into `SovaWatch\bin` and ONLY the helper path in
//!    `statusLine.command` is replaced (`claude_settings::migrate_shim`: splice + verify), with a
//!    redacted backup and the same compare-and-swap as Connect. The old connection record comes
//!    along as `wrap.json` (pointing at the new helper), so Disconnect still restores the
//!    original command exactly. After a successful switch the old folder's `wrap.json` is renamed
//!    `wrap.json.migrated`: a still-running old app then no longer reports a lost connection, and
//!    the old uninstaller's `--disconnect` has no record to act on — it does not recognise the
//!    new helper anyway, so it leaves the status line alone (proved against the released v0 code
//!    in the tests below);
//! 3. [`MARKER_FILE`] records the move, so it runs, and is announced, only once.
//!
//! A failed switch leaves the status line on the old helper: SovaWatch still recognises that
//! command (status, Disconnect, Reconnect), and the notice asks the user to Disconnect and
//! Connect again (Settings shows that command as connected, so it offers only Disconnect).
//! Nothing here runs while `SOVA_DATA_DIR` overrides the data folder (tests, development).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sovawatch_core::claude_settings::{self, WrapRecord};
use sovawatch_core::cmdline::{self, LEGACY_SHIM_EXE_NAME};
use sovawatch_core::paths::Paths;
use sovawatch_core::time::Ms;

use crate::connect::{self, ConnectionStatus};
use crate::state::{save_json, write_atomic};

/// Written into the new data folder once the move ran.
pub const MARKER_FILE: &str = "migrated.json";
/// The old folder's connection record is renamed to this after the status line was switched.
pub const RETIRED_WRAP_FILE: &str = "wrap.json.migrated";
/// The old app's own files that are copied (when present and not yet in the new folder).
/// `wrap.json` is handled separately; backups, logs, the helper, the watchdog's dismissed
/// fingerprints and `update-check.json` (its release page is on the old repository address,
/// which `open_url` refuses) are left behind.
pub const COPIED_FILES: &[&str] = &["settings.json", "state.json", "history.jsonl", "alerts.json", "positions.json"];
const CAS_ATTEMPTS: u32 = 3;
/// A capture file is small; anything bigger in the old capture folder is not ours to copy.
const MAX_CAPTURE_BYTES: u64 = 1024 * 1024;

/// What happened to Claude Code's status line during the move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLineMove {
    /// It did not run the old helper: nothing to switch.
    NotConnected,
    /// It now runs SovaWatch's helper.
    Switched,
    /// It still runs the old helper.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to move (already moved, a fresh install, or a data-folder override).
    NotNeeded,
    Moved {
        status_line: StatusLineMove,
        /// "Start with Windows" was on in the old app.
        autostart: bool,
    },
}

#[derive(Serialize)]
struct Marker<'a> {
    from: &'a str,
    at_ms: Ms,
    status_line: &'a StatusLineMove,
}

/// Runs the move if it is due. `legacy_root`: the old data folder (`None` = never move);
/// `sidecar`: the helper shipped next to the app. Errors while copying are logged and skipped;
/// the move never stops the app from starting.
pub fn run(paths: &Paths, legacy_root: Option<&Path>, sidecar: Option<&Path>, now: Ms) -> Outcome {
    let Some(legacy_root) = legacy_root else { return Outcome::NotNeeded };
    let marker = paths.data_root().join(MARKER_FILE);
    if paths.settings_file().exists() || marker.exists() || !legacy_root.is_dir() {
        return Outcome::NotNeeded;
    }
    if let Err(e) = fs::create_dir_all(paths.data_root()) {
        crate::diag::log(&format!("move from Claude Usage Widget: cannot create the data folder: {e}"));
        return Outcome::NotNeeded;
    }
    for name in COPIED_FILES {
        copy_if_absent(&legacy_root.join(name), &paths.data_root().join(name));
    }
    copy_captures(&legacy_root.join("capture"), &paths.capture_dir());
    let autostart = old_autostart(&legacy_root.join("settings.json"));

    let status_line = switch_status_line(paths, legacy_root, sidecar, now);
    let record = Marker { from: LEGACY_NAME, at_ms: now, status_line: &status_line };
    if let Err(e) = save_json(&marker, &record) {
        crate::diag::log(&format!("move from Claude Usage Widget: cannot write {MARKER_FILE}: {e}"));
    }
    Outcome::Moved { status_line, autostart }
}

/// The app's former name.
const LEGACY_NAME: &str = "Claude Usage Widget";

/// The command form of the old installed helper, `<legacy_root>/bin/cuw-capture.exe`.
pub fn legacy_shim_command_path(legacy_root: &Path) -> String {
    cmdline::shim_path_for_command(&legacy_root.join("bin").join(LEGACY_SHIM_EXE_NAME))
}

fn switch_status_line(paths: &Paths, legacy_root: &Path, sidecar: Option<&Path>, now: Ms) -> StatusLineMove {
    let legacy_shim = legacy_shim_command_path(legacy_root);
    match connect::status(paths) {
        ConnectionStatus::Connected { shim_path, .. } if shim_path.eq_ignore_ascii_case(&legacy_shim) => {}
        _ => return StatusLineMove::NotConnected,
    }
    let new_shim_file = connect::installed_shim(paths);
    let new_shim = cmdline::shim_path_for_command(&new_shim_file);

    // The old record, pointing at the new helper, so Disconnect restores the original exactly.
    let old_wrap = legacy_root.join("wrap.json");
    if let Some(mut rec) = fs::read(&old_wrap).ok().and_then(|b| serde_json::from_slice::<WrapRecord>(&b).ok()) {
        if !paths.wrap_file().exists() {
            rec.shim_path = new_shim.clone();
            if let Err(e) = save_json(&paths.wrap_file(), &rec) {
                return StatusLineMove::Failed(format!("cannot write wrap.json: {e}"));
            }
        }
    }

    let Some(sidecar) = sidecar else {
        return StatusLineMove::Failed("the capture helper was not found next to the app".into());
    };
    if let Err(e) = connect::install_shim(sidecar, &new_shim_file) {
        return StatusLineMove::Failed(format!("cannot install the capture helper: {e}"));
    }
    match rewrite_settings(paths, &legacy_shim, &new_shim, now) {
        Ok(true) => {
            if old_wrap.exists() {
                if let Err(e) = fs::rename(&old_wrap, legacy_root.join(RETIRED_WRAP_FILE)) {
                    crate::diag::log(&format!("move from Claude Usage Widget: cannot retire the old wrap.json: {e}"));
                }
            }
            StatusLineMove::Switched
        }
        // The status line changed in between and no longer runs the old helper.
        Ok(false) => StatusLineMove::NotConnected,
        Err(e) => StatusLineMove::Failed(e),
    }
}

/// Replaces the helper path in `settings.json` (backup first, compare-and-swap like Connect).
/// `Ok(false)` when the file no longer runs the old helper.
fn rewrite_settings(paths: &Paths, legacy_shim: &str, new_shim: &str, now: Ms) -> Result<bool, String> {
    for attempt in 0..CAS_ATTEMPTS {
        let bytes = connect::read_settings(paths)?;
        let Some(next) = claude_settings::migrate_shim(&bytes, legacy_shim, new_shim).map_err(|e| e.to_string())?
        else {
            return Ok(false);
        };
        connect::backup(&paths.backups_dir(), &bytes, now).map_err(|e| format!("backup failed: {e}"))?;
        if connect::read_settings(paths)? != bytes {
            if attempt + 1 == CAS_ATTEMPTS {
                break;
            }
            continue;
        }
        write_atomic(&paths.claude_settings(), &next).map_err(|e| format!("cannot write settings.json: {e}"))?;
        return Ok(true);
    }
    Err("settings.json keeps changing; try again in a moment".into())
}

fn copy_if_absent(from: &Path, to: &Path) {
    if to.exists() || !from.is_file() {
        return;
    }
    let result = fs::read(from).and_then(|bytes| write_atomic(to, &bytes));
    if let Err(e) = result {
        crate::diag::log(&format!("move from Claude Usage Widget: cannot copy {}: {e}", display_name(from)));
    }
}

/// The capture files (`<session>.json`, the helper's logs); sub-folders and oversized files are
/// skipped.
fn copy_captures(from: &Path, to: &Path) {
    let Ok(entries) = fs::read_dir(from) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() && meta.len() <= MAX_CAPTURE_BYTES {
            copy_if_absent(&entry.path(), &to.join(entry.file_name()));
        }
    }
}

fn old_autostart(settings: &Path) -> bool {
    fs::read(settings)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("start_with_windows").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The one-time notification after a move (`None` when nothing moved).
pub fn notice(outcome: &Outcome) -> Option<(String, String)> {
    let Outcome::Moved { status_line, .. } = outcome else { return None };
    let title = "Moved from Claude Usage Widget".to_owned();
    let body = match status_line {
        StatusLineMove::Switched => {
            "Settings and history were copied, and Claude Code's status line now uses SovaWatch. \
             Uninstall Claude Usage Widget from Windows Settings (don't click Reconnect in it)."
        }
        StatusLineMove::NotConnected => {
            "Settings and history were copied. You can uninstall Claude Usage Widget from Windows Settings."
        }
        StatusLineMove::Failed(_) => {
            "Settings and history were copied, but Claude Code's status line still uses Claude Usage \
             Widget's helper. In SovaWatch, open Settings → Claude Code, click Disconnect, then Connect \
             again before uninstalling the old app."
        }
    };
    Some((title, body.to_owned()))
}

/// Where the old data folder is, for the real app (`None` under a data-folder override).
pub fn legacy_root() -> Option<PathBuf> {
    sovawatch_core::paths::detect_legacy_data_root()
}

#[cfg(test)]
#[allow(dead_code, clippy::all, clippy::pedantic)]
mod legacy_v0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::{ConnectEnv, Shell};
    use sovawatch_core::cmdline::{SHIM_EXE_NAME, ShellKind};

    const ORIGINAL: &str = "{\r\n  \"model\": \"opus\",\r\n  \"statusLine\": {\r\n    \"type\": \"command\",\r\n    \"command\": \"pwsh -NoProfile -File \\\"C:/Users/tester/.claude/statusline.ps1\\\" \\u00e9\",\r\n    \"padding\": 0\r\n  },\r\n  \"theme\": \"dark\"\r\n}\r\n";

    /// A temp home with Claude Code's settings, the old app's data folder (connected by the
    /// released v0 code) and SovaWatch's empty data folder.
    struct World {
        _tmp: tempfile::TempDir,
        paths: Paths,
        legacy: PathBuf,
        sidecar: PathBuf,
    }

    impl World {
        fn new(settings: Option<&str>) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let local = tmp.path().join("Local");
            let paths = Paths::with_roots(tmp.path().join(".claude"), vec![], local.join("SovaWatch"));
            if let Some(s) = settings {
                fs::create_dir_all(paths.claude_home()).unwrap();
                fs::write(paths.claude_settings(), s).unwrap();
            }
            let sidecar = tmp.path().join("app").join(SHIM_EXE_NAME);
            fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            fs::write(&sidecar, b"new helper").unwrap();
            let legacy = local.join("ClaudeUsageWidget");
            fs::create_dir_all(&legacy).unwrap();
            Self { _tmp: tmp, paths, legacy, sidecar }
        }

        /// The old app's data: settings, history, a capture, and (optionally) its Connect done
        /// with the released v0 code, exactly as Claude Usage Widget wrote them.
        fn old_app(&self, connected: bool) {
            fs::write(self.legacy.join("settings.json"), r#"{"schema_version":1,"start_with_windows":true}"#).unwrap();
            fs::write(self.legacy.join("history.jsonl"), "{\"t\":1}\n").unwrap();
            fs::create_dir_all(self.legacy.join("capture")).unwrap();
            fs::write(self.legacy.join("capture").join("00000000-0000-4000-8000-000000000001.json"), "{}").unwrap();
            if connected {
                let old_paths = self.old_paths();
                let shim = cmdline::shim_path_for_command(&self.legacy.join("bin").join(LEGACY_SHIM_EXE_NAME));
                fs::create_dir_all(self.legacy.join("bin")).unwrap();
                fs::write(self.legacy.join("bin").join(LEGACY_SHIM_EXE_NAME), b"old helper").unwrap();
                let bytes = fs::read(old_paths.claude_settings()).unwrap_or_default();
                let (next, rec) =
                    legacy_v0::claude_settings::connect(&bytes, &shim, legacy_v0::cmdline::ShellKind::Pwsh, 5).unwrap();
                fs::write(old_paths.claude_settings(), next).unwrap();
                fs::write(old_paths.wrap_file(), serde_json::to_vec(&rec).unwrap()).unwrap();
            }
        }

        /// What the old app (and its uninstaller) resolves: the same Claude home, the old folder.
        fn old_paths(&self) -> Paths {
            Paths::with_roots(self.paths.claude_home().to_path_buf(), vec![], self.legacy.clone())
        }

        fn settings(&self) -> String {
            fs::read_to_string(self.paths.claude_settings()).unwrap()
        }

        fn run(&self) -> Outcome {
            run(&self.paths, Some(&self.legacy), Some(&self.sidecar), 1_000)
        }
    }

    #[test]
    fn nothing_to_move_without_the_old_folder_or_under_an_override() {
        let w = World::new(Some(ORIGINAL));
        fs::remove_dir_all(&w.legacy).unwrap();
        assert_eq!(w.run(), Outcome::NotNeeded);
        assert_eq!(run(&w.paths, None, Some(&w.sidecar), 1), Outcome::NotNeeded);
        assert!(!w.paths.data_root().exists(), "nothing created");
        assert_eq!(w.settings(), ORIGINAL);
    }

    #[test]
    fn a_connected_old_install_moves_and_only_the_helper_path_changes() {
        let w = World::new(Some(ORIGINAL));
        w.old_app(true);
        let connected_by_v0 = w.settings();
        let legacy_shim = legacy_shim_command_path(&w.legacy);
        assert!(connected_by_v0.contains(&legacy_shim));

        let outcome = w.run();
        assert_eq!(outcome, Outcome::Moved { status_line: StatusLineMove::Switched, autostart: true });
        let new_shim = cmdline::shim_path_for_command(&connect::installed_shim(&w.paths));
        assert_eq!(w.settings(), connected_by_v0.replacen(&legacy_shim, &new_shim, 1), "byte for byte");
        assert_eq!(fs::read(connect::installed_shim(&w.paths)).unwrap(), b"new helper");
        // Data copied, old folder kept (its record retired), marker written.
        assert!(w.paths.settings_file().is_file() && w.paths.history_file().is_file());
        assert!(w.paths.capture_dir().join("00000000-0000-4000-8000-000000000001.json").is_file());
        assert!(w.legacy.join("settings.json").is_file() && w.legacy.join(RETIRED_WRAP_FILE).is_file());
        assert!(!w.legacy.join("wrap.json").exists());
        assert!(w.paths.data_root().join(MARKER_FILE).is_file());
        let rec: WrapRecord = serde_json::from_slice(&fs::read(w.paths.wrap_file()).unwrap()).unwrap();
        assert_eq!(rec.shim_path, new_shim);
        assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
        assert_eq!(notice(&outcome).unwrap().0, "Moved from Claude Usage Widget");

        // (a) Idempotent: a second start changes nothing.
        let moved = w.settings();
        assert_eq!(w.run(), Outcome::NotNeeded);
        assert_eq!(w.settings(), moved);
        assert_eq!(notice(&Outcome::NotNeeded), None);

        // SovaWatch's Disconnect restores the user's original command exactly.
        connect::disconnect(&w.paths, 2_000).unwrap();
        assert_eq!(w.settings(), ORIGINAL);
    }

    /// (b) The old uninstaller runs `claude-usage-widget.exe --disconnect --quiet` with the
    /// released v0 code and its own data folder. Against the moved status line it must be a
    /// no-op — with the retired record, and even with the old `wrap.json` still in place.
    #[test]
    fn the_old_apps_disconnect_leaves_the_moved_status_line_alone() {
        for keep_old_record in [false, true] {
            let w = World::new(Some(ORIGINAL));
            w.old_app(true);
            let old_record = fs::read(w.legacy.join("wrap.json")).unwrap();
            w.run();
            if keep_old_record {
                fs::write(w.legacy.join("wrap.json"), &old_record).unwrap();
            }
            let moved = w.settings();
            let old_paths = w.old_paths();
            assert_eq!(legacy_v0::disconnect(&old_paths), Ok(false), "nothing of the old app's to undo");
            assert_eq!(w.settings(), moved, "not removed or corrupted (record kept: {keep_old_record})");
            // The old app also no longer sees its own connection.
            assert!(!matches!(
                legacy_v0::claude_settings::status(moved.as_bytes()).unwrap(),
                legacy_v0::claude_settings::Status::Connected { .. }
            ));
            // SovaWatch is still connected and still disconnects exactly.
            assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
            connect::disconnect(&w.paths, 2_000).unwrap();
            assert_eq!(w.settings(), ORIGINAL);
        }
    }

    /// Without any move (e.g. under a data-folder override) SovaWatch still recognises the old
    /// helper's command and restores the user's command. With no record of the original literal
    /// it is written in canonical JSON (`\u00e9` becomes `é`); every other byte is kept.
    #[test]
    fn disconnect_works_on_the_old_prefix_even_without_the_move() {
        let w = World::new(Some(ORIGINAL));
        w.old_app(true);
        assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
        connect::disconnect(&w.paths, 2_000).unwrap();
        assert_eq!(w.settings(), ORIGINAL.replace("\\u00e9", "\u{e9}"));
    }

    #[test]
    fn an_unconnected_old_install_moves_its_data_only() {
        let w = World::new(Some(ORIGINAL));
        w.old_app(false);
        let outcome = w.run();
        assert_eq!(outcome, Outcome::Moved { status_line: StatusLineMove::NotConnected, autostart: true });
        assert_eq!(w.settings(), ORIGINAL);
        assert!(!w.paths.wrap_file().exists(), "no record, so no false 'connection lost' warning");
        assert!(!w.paths.bin_dir().exists());
        assert!(notice(&outcome).unwrap().1.contains("uninstall Claude Usage Widget"));
    }

    #[test]
    fn a_failed_switch_keeps_the_old_status_line_and_its_record() {
        let w = World::new(Some(ORIGINAL));
        w.old_app(true);
        let before = w.settings();
        let outcome = run(&w.paths, Some(&w.legacy), None, 1_000);
        let Outcome::Moved { status_line: StatusLineMove::Failed(reason), .. } = &outcome else {
            panic!("{outcome:?}")
        };
        assert!(reason.contains("helper"), "{reason}");
        assert_eq!(w.settings(), before);
        assert!(w.legacy.join("wrap.json").is_file(), "the old uninstaller can still restore it");
        // Settings shows the old helper's command as Connected and offers only Disconnect there,
        // so the notice must ask for Disconnect, then Connect.
        assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
        let body = notice(&outcome).unwrap().1;
        assert!(body.contains("Disconnect") && body.contains("Connect again"), "{body}");
        // SovaWatch can still undo it exactly with the copied record.
        connect::disconnect(&w.paths, 2_000).unwrap();
        assert_eq!(w.settings(), ORIGINAL);
        // ...and the Connect that follows wraps with SovaWatch's own helper.
        let env = ConnectEnv {
            paths: w.paths.clone(),
            shell: Shell { kind: ShellKind::Pwsh, exe: PathBuf::from("pwsh.exe") },
            shim_source: Some(w.sidecar.clone()),
            selftest: false,
        };
        connect::connect(&env, 3_000).unwrap();
        let text = w.settings();
        assert!(!text.contains(LEGACY_SHIM_EXE_NAME) && text.matches(SHIM_EXE_NAME).count() == 1, "{text}");
        connect::disconnect(&w.paths, 4_000).unwrap();
        assert_eq!(w.settings(), ORIGINAL);
    }

    #[test]
    fn existing_new_data_is_never_overwritten() {
        let w = World::new(None);
        w.old_app(false);
        fs::create_dir_all(w.paths.data_root()).unwrap();
        fs::write(w.paths.history_file(), "mine\n").unwrap();
        w.run();
        assert_eq!(fs::read_to_string(w.paths.history_file()).unwrap(), "mine\n");
        // Existing settings mean SovaWatch already ran: nothing moves.
        let w2 = World::new(None);
        w2.old_app(false);
        fs::create_dir_all(w2.paths.data_root()).unwrap();
        fs::write(w2.paths.settings_file(), "{}").unwrap();
        assert_eq!(w2.run(), Outcome::NotNeeded);
        assert!(!w2.paths.history_file().exists());
    }

    /// Connect in SovaWatch after a failed switch (what the notice asks for) re-wraps the old
    /// command instead of nesting a second helper, and keeps the copied record's exact literal.
    #[test]
    fn connect_replaces_the_old_helper_instead_of_nesting() {
        let w = World::new(Some(ORIGINAL));
        w.old_app(true);
        let failed = run(&w.paths, Some(&w.legacy), None, 1_000);
        assert!(matches!(failed, Outcome::Moved { status_line: StatusLineMove::Failed(_), .. }));
        let env = ConnectEnv {
            paths: w.paths.clone(),
            shell: Shell { kind: ShellKind::Pwsh, exe: PathBuf::from("pwsh.exe") },
            shim_source: Some(w.sidecar.clone()),
            selftest: false,
        };
        connect::connect(&env, 3_000).unwrap();
        let text = w.settings();
        assert!(!text.contains(LEGACY_SHIM_EXE_NAME) && text.matches(SHIM_EXE_NAME).count() == 1, "{text}");
        connect::disconnect(&w.paths, 4_000).unwrap();
        assert_eq!(w.settings(), ORIGINAL);
    }
}
