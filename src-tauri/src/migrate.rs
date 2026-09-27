//! The move from one of the app's former names to Mikyas, run once on the first start, before
//! anything else reads the data folder.
//!
//! The former names ([`mikyas_core::paths::LEGACY_APPS`]), newest first:
//! - SovaWatch 0.1.0: `%LOCALAPPDATA%\SovaWatch`, helper `bin\sovawatch-capture.exe`. It was
//!   installed into that same folder, so `sovawatch.exe` and `uninstall.exe` live there too;
//! - Claude Usage Widget: `%LOCALAPPDATA%\ClaudeUsageWidget`, helper `bin\cuw-capture.exe`.
//!
//! When `%LOCALAPPDATA%\Mikyas` has no `settings.json` and no [`MARKER_FILE`] yet, ONE of them is
//! the source ([`choose`]): the one whose helper Claude Code's status line runs right now; else
//! the newest whose folder holds its own data ([`COPIED_FILES`]); else Claude Usage Widget's
//! folder if it exists. From that source:
//! 1. the widget's own files ([`COPIED_FILES`] and the captures) are copied over. Nothing already
//!    in the new folder is overwritten, and the old folder is not deleted; its backups, logs,
//!    helper, program files, `watchdog.json`, `migrated.json` and `update-check.json` (its release
//!    page is on the old repository address, which `open_url` refuses) are left behind;
//! 2. if Claude Code's status line runs the source's helper, the new helper is installed into
//!    `Mikyas\bin` and ONLY the helper path in `statusLine.command` is replaced
//!    (`claude_settings::migrate_shim`: splice + verify), with a redacted backup and the same
//!    compare-and-swap as Connect. The old connection record comes along as `wrap.json`
//!    (pointing at the new helper), so Disconnect still restores the original command exactly.
//!    After a successful switch the source's `wrap.json` is renamed `wrap.json.migrated`: a
//!    still-running old app then no longer reports a lost connection, and the old uninstaller's
//!    `--disconnect` has no record to act on — it does not recognise the new helper anyway, so it
//!    leaves the status line alone (proved against the released code of both in the tests below);
//! 3. [`MARKER_FILE`] records the move (`from`: the old app's name), so it runs, and is
//!    announced, only once.
//!
//! A failed switch leaves the status line on the old helper: Mikyas still recognises that command
//! (status, Disconnect, Reconnect), and the notice asks the user to Disconnect and Connect again
//! (Settings shows that command as connected, so it offers only Disconnect).
//! Nothing here runs while `MIKYAS_DATA_DIR` overrides the data folder (tests, development).

use std::fs;
use std::path::Path;

use mikyas_core::claude_settings::{self, WrapRecord};
use mikyas_core::cmdline;
use mikyas_core::paths::{LegacyApp, LegacyRoot, Paths};
use mikyas_core::time::Ms;
use serde::Serialize;

use crate::connect::{self, ConnectionStatus};
use crate::state::{save_json, write_atomic};

/// Written into the new data folder once the move ran.
pub const MARKER_FILE: &str = "migrated.json";
/// The old folder's connection record is renamed to this after the status line was switched.
pub const RETIRED_WRAP_FILE: &str = "wrap.json.migrated";
/// The old app's own files that are copied (when present and not yet in the new folder).
/// `wrap.json` is handled separately; everything else stays behind (see the module docs).
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
    /// It now runs Mikyas's helper.
    Switched,
    /// It still runs the old helper.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to move (already moved, a fresh install, or a data-folder override).
    NotNeeded,
    Moved {
        /// The former name the data came from.
        from: LegacyApp,
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

/// Runs the move if it is due. `legacy`: the former names' data folders (empty = never move);
/// `sidecar`: the helper shipped next to the app. Errors while copying are logged and skipped;
/// the move never stops the app from starting.
pub fn run(paths: &Paths, legacy: &[LegacyRoot], sidecar: Option<&Path>, now: Ms) -> Outcome {
    let marker = paths.data_root().join(MARKER_FILE);
    let any_old_folder = legacy.iter().any(|l| l.root.is_dir());
    if !any_old_folder || paths.settings_file().exists() || marker.exists() {
        return Outcome::NotNeeded;
    }
    let status = connect::status(paths);
    let Some(source) = choose(&status, legacy) else { return Outcome::NotNeeded };
    let log = |msg: String| crate::diag::log(&format!("move from {}: {msg}", source.app.display_name));
    if let Err(e) = fs::create_dir_all(paths.data_root()) {
        log(format!("cannot create the data folder: {e}"));
        return Outcome::NotNeeded;
    }
    for name in COPIED_FILES {
        copy_if_absent(&source.root.join(name), &paths.data_root().join(name), &log);
    }
    copy_captures(&source.root.join("capture"), &paths.capture_dir(), &log);
    let autostart = old_autostart(&source.root.join("settings.json"));

    let status_line = if runs_helper_of(&status, source) {
        switch_status_line(paths, source, sidecar, now, &log)
    } else {
        StatusLineMove::NotConnected
    };
    let record = Marker { from: source.app.display_name, at_ms: now, status_line: &status_line };
    if let Err(e) = save_json(&marker, &record) {
        log(format!("cannot write {MARKER_FILE}: {e}"));
    }
    Outcome::Moved { from: source.app, status_line, autostart }
}

/// The source of the move (see the module docs); `None` when there is nothing to move.
fn choose<'a>(status: &ConnectionStatus, legacy: &'a [LegacyRoot]) -> Option<&'a LegacyRoot> {
    let existing = || legacy.iter().filter(|l| l.root.is_dir());
    if let Some(source) = existing().find(|l| runs_helper_of(status, l)) {
        return Some(source);
    }
    let has_data = |l: &&LegacyRoot| COPIED_FILES.iter().any(|name| l.root.join(name).is_file());
    existing().find(has_data).or_else(|| legacy.last().filter(|l| l.root.is_dir()))
}

/// Whether the status line runs `legacy`'s installed helper (compared like Windows paths).
fn runs_helper_of(status: &ConnectionStatus, legacy: &LegacyRoot) -> bool {
    matches!(status, ConnectionStatus::Connected { shim_path, .. }
        if shim_path.eq_ignore_ascii_case(&legacy_shim_command_path(legacy)))
}

/// The command form of an old installed helper, `<root>/bin/<its helper>`.
pub fn legacy_shim_command_path(legacy: &LegacyRoot) -> String {
    cmdline::shim_path_for_command(&legacy.installed_shim())
}

fn switch_status_line(
    paths: &Paths,
    source: &LegacyRoot,
    sidecar: Option<&Path>,
    now: Ms,
    log: &dyn Fn(String),
) -> StatusLineMove {
    let legacy_shim = legacy_shim_command_path(source);
    let new_shim_file = connect::installed_shim(paths);
    let new_shim = cmdline::shim_path_for_command(&new_shim_file);

    // The old record, pointing at the new helper, so Disconnect restores the original exactly.
    let old_wrap = source.root.join("wrap.json");
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
                if let Err(e) = fs::rename(&old_wrap, source.root.join(RETIRED_WRAP_FILE)) {
                    log(format!("cannot retire the old wrap.json: {e}"));
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

fn copy_if_absent(from: &Path, to: &Path, log: &dyn Fn(String)) {
    if to.exists() || !from.is_file() {
        return;
    }
    let result = fs::read(from).and_then(|bytes| write_atomic(to, &bytes));
    if let Err(e) = result {
        log(format!("cannot copy {}: {e}", display_name(from)));
    }
}

/// The capture files (`<session>.json`, the helper's logs); sub-folders and oversized files are
/// skipped.
fn copy_captures(from: &Path, to: &Path, log: &dyn Fn(String)) {
    let Ok(entries) = fs::read_dir(from) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() && meta.len() <= MAX_CAPTURE_BYTES {
            copy_if_absent(&entry.path(), &to.join(entry.file_name()), log);
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
    let Outcome::Moved { from, status_line, .. } = outcome else { return None };
    let old = from.display_name;
    let title = format!("Moved from {old}");
    let body = match status_line {
        StatusLineMove::Switched => format!(
            "Settings and history were copied, and Claude Code's status line now uses Mikyas. \
             Uninstall {old} from Windows Settings (don't click Reconnect in it)."
        ),
        StatusLineMove::NotConnected => {
            format!("Settings and history were copied. You can uninstall {old} from Windows Settings.")
        }
        StatusLineMove::Failed(_) => format!(
            "Settings and history were copied, but Claude Code's status line still uses {old}'s helper. \
             In Mikyas, open Settings → Claude Code, click Disconnect, then Connect again before \
             uninstalling {old}."
        ),
    };
    Some((title, body))
}

/// Where the former names kept their data, for the real app (empty under a data-folder override).
pub fn legacy_roots() -> Vec<LegacyRoot> {
    mikyas_core::paths::detect_legacy_data_roots()
}

/// Claude Usage Widget's released code (commit 42e52af).
#[cfg(test)]
#[allow(dead_code, clippy::all, clippy::pedantic)]
mod legacy_v0;

/// SovaWatch 0.1.0's released code (tag v0.1.0, commit 9f3192c).
#[cfg(test)]
#[allow(dead_code, clippy::all, clippy::pedantic)]
mod legacy_v0_1;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::connect::{ConnectEnv, Shell};
    use mikyas_core::cmdline::{LEGACY_SHIM_EXE_NAMES, SHIM_EXE_NAME, ShellKind};
    use mikyas_core::paths::LEGACY_APPS;

    const ORIGINAL: &str = "{\r\n  \"model\": \"opus\",\r\n  \"statusLine\": {\r\n    \"type\": \"command\",\r\n    \"command\": \"pwsh -NoProfile -File \\\"C:/Users/tester/.claude/statusline.ps1\\\" \\u00e9\",\r\n    \"padding\": 0\r\n  },\r\n  \"theme\": \"dark\"\r\n}\r\n";
    const SHELLS: [ShellKind; 4] = [ShellKind::Bash, ShellKind::Cmd, ShellKind::Pwsh, ShellKind::LegacyPowerShell];

    /// The former names, as indexes into [`World::legacy`] and [`LEGACY_APPS`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Old {
        SovaWatch = 0,
        Cuw = 1,
    }
    const OLD: [Old; 2] = [Old::SovaWatch, Old::Cuw];

    impl Old {
        fn app(self) -> LegacyApp {
            LEGACY_APPS[self as usize]
        }

        fn other(self) -> Self {
            if self == Self::SovaWatch { Self::Cuw } else { Self::SovaWatch }
        }
    }

    /// A temp home with Claude Code's settings, the former names' data folders (created by
    /// [`World::old_app`]) and Mikyas's empty data folder.
    struct World {
        _tmp: tempfile::TempDir,
        paths: Paths,
        legacy: Vec<LegacyRoot>,
        sidecar: PathBuf,
    }

    impl World {
        fn new(settings: Option<&str>) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let local = tmp.path().join("Local");
            let paths = Paths::with_roots(tmp.path().join(".claude"), vec![], local.join("Mikyas"));
            if let Some(s) = settings {
                fs::create_dir_all(paths.claude_home()).unwrap();
                fs::write(paths.claude_settings(), s).unwrap();
            }
            let sidecar = tmp.path().join("app").join(SHIM_EXE_NAME);
            fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            fs::write(&sidecar, b"new helper").unwrap();
            let legacy = LEGACY_APPS.iter().map(|&app| LegacyRoot { app, root: local.join(app.dir_name) }).collect();
            Self { _tmp: tmp, paths, legacy, sidecar }
        }

        fn root(&self, old: Old) -> &Path {
            &self.legacy[old as usize].root
        }

        /// The old app's folder as it left it: its data, captures, the files that must stay
        /// behind (for SovaWatch also its program files, since it was installed there) and,
        /// when `connected`, its Connect done with its released code in that shell's form.
        fn old_app(&self, old: Old, connected: Option<ShellKind>) {
            let root = self.root(old);
            fs::create_dir_all(root.join("capture")).unwrap();
            fs::create_dir_all(root.join("backups")).unwrap();
            let own = |name: &str| format!("{{\"from\":\"{old:?}\",\"file\":\"{name}\"}}");
            fs::write(root.join("settings.json"), r#"{"schema_version":1,"start_with_windows":true}"#).unwrap();
            for name in ["state.json", "alerts.json", "positions.json"] {
                fs::write(root.join(name), own(name)).unwrap();
            }
            fs::write(root.join("history.jsonl"), format!("{{\"t\":{}}}\n", old as u8)).unwrap();
            fs::write(root.join("capture").join("00000000-0000-4000-8000-000000000001.json"), own("capture")).unwrap();
            fs::write(root.join("capture").join("_errors.log"), "x\n").unwrap();
            for left in ["update-check.json", "watchdog.json", "migrated.json", "app.log"] {
                fs::write(root.join(left), own(left)).unwrap();
            }
            fs::write(root.join("backups").join("settings-000000000000001.json"), "{}").unwrap();
            if old == Old::SovaWatch {
                for program in ["sovawatch.exe", "uninstall.exe", "sovawatch-capture.exe", "THIRD_PARTY_NOTICES.md"] {
                    fs::write(root.join(program), b"program file").unwrap();
                }
            }
            let Some(shell) = connected else { return };
            let old_paths = self.old_paths(old);
            let helper = self.legacy[old as usize].installed_shim();
            fs::create_dir_all(helper.parent().unwrap()).unwrap();
            fs::write(&helper, b"old helper").unwrap();
            let shim = cmdline::shim_path_for_command(&helper);
            let bytes = fs::read(old_paths.claude_settings()).unwrap_or_default();
            let (next, rec) = match old {
                Old::SovaWatch => {
                    let (next, rec) =
                        legacy_v0_1::claude_settings::connect(&bytes, &shim, v0_1_shell(shell), 5).unwrap();
                    (next, serde_json::to_vec(&rec).unwrap())
                }
                Old::Cuw => {
                    let (next, rec) = legacy_v0::claude_settings::connect(&bytes, &shim, v0_shell(shell), 5).unwrap();
                    (next, serde_json::to_vec(&rec).unwrap())
                }
            };
            fs::write(old_paths.claude_settings(), next).unwrap();
            fs::write(old_paths.wrap_file(), rec).unwrap();
        }

        /// What the old app (and its uninstaller) resolves: the same Claude home, its own folder.
        fn old_paths(&self, old: Old) -> Paths {
            Paths::with_roots(self.paths.claude_home().to_path_buf(), vec![], self.root(old).to_path_buf())
        }

        /// The old app's own `--disconnect` (what its uninstaller runs), with its released code.
        fn old_disconnect(&self, old: Old) -> Result<bool, String> {
            match old {
                Old::SovaWatch => legacy_v0_1::disconnect(&self.old_paths(old)),
                Old::Cuw => legacy_v0::disconnect(&self.old_paths(old)),
            }
        }

        /// Whether the old app would see a status line connection of its own.
        fn old_sees_connected(&self, old: Old) -> bool {
            let bytes = self.settings();
            match old {
                Old::SovaWatch => matches!(
                    legacy_v0_1::claude_settings::status(bytes.as_bytes()).unwrap(),
                    legacy_v0_1::claude_settings::Status::Connected { .. }
                ),
                Old::Cuw => matches!(
                    legacy_v0::claude_settings::status(bytes.as_bytes()).unwrap(),
                    legacy_v0::claude_settings::Status::Connected { .. }
                ),
            }
        }

        fn legacy_shim(&self, old: Old) -> String {
            legacy_shim_command_path(&self.legacy[old as usize])
        }

        fn new_shim(&self) -> String {
            cmdline::shim_path_for_command(&connect::installed_shim(&self.paths))
        }

        fn settings(&self) -> String {
            fs::read_to_string(self.paths.claude_settings()).unwrap()
        }

        fn history(&self) -> String {
            fs::read_to_string(self.paths.history_file()).unwrap()
        }

        fn run(&self) -> Outcome {
            run(&self.paths, &self.legacy, Some(&self.sidecar), 1_000)
        }

        fn marker_from(&self) -> String {
            let bytes = fs::read(self.paths.data_root().join(MARKER_FILE)).unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            v["from"].as_str().unwrap().to_owned()
        }

        fn connect_env(&self) -> ConnectEnv {
            ConnectEnv {
                paths: self.paths.clone(),
                shell: Shell { kind: ShellKind::Pwsh, exe: PathBuf::from("pwsh.exe") },
                shim_source: Some(self.sidecar.clone()),
                selftest: false,
            }
        }
    }

    fn v0_shell(kind: ShellKind) -> legacy_v0::cmdline::ShellKind {
        match kind {
            ShellKind::Bash => legacy_v0::cmdline::ShellKind::Bash,
            ShellKind::Cmd => legacy_v0::cmdline::ShellKind::Cmd,
            ShellKind::Pwsh => legacy_v0::cmdline::ShellKind::Pwsh,
            ShellKind::LegacyPowerShell => legacy_v0::cmdline::ShellKind::LegacyPowerShell,
        }
    }

    fn v0_1_shell(kind: ShellKind) -> legacy_v0_1::cmdline::ShellKind {
        match kind {
            ShellKind::Bash => legacy_v0_1::cmdline::ShellKind::Bash,
            ShellKind::Cmd => legacy_v0_1::cmdline::ShellKind::Cmd,
            ShellKind::Pwsh => legacy_v0_1::cmdline::ShellKind::Pwsh,
            ShellKind::LegacyPowerShell => legacy_v0_1::cmdline::ShellKind::LegacyPowerShell,
        }
    }

    /// Every file under `dir` (relative, `/`-separated, sorted) with its bytes.
    fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                    out.push((rel, fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn the_former_names_and_their_helpers() {
        assert_eq!(LEGACY_APPS.map(|a| a.display_name), ["SovaWatch", "Claude Usage Widget"]);
        assert_eq!(LEGACY_APPS.map(|a| a.shim_exe), LEGACY_SHIM_EXE_NAMES);
        let w = World::new(None);
        assert!(w.legacy_shim(Old::SovaWatch).ends_with("/Local/SovaWatch/bin/sovawatch-capture.exe"));
        assert!(w.legacy_shim(Old::Cuw).ends_with("/Local/ClaudeUsageWidget/bin/cuw-capture.exe"));
        assert!(w.new_shim().ends_with("/Local/Mikyas/bin/mikyas-capture.exe"));
    }

    #[test]
    fn nothing_to_move_without_the_old_folders_or_under_an_override() {
        let w = World::new(Some(ORIGINAL));
        assert_eq!(w.run(), Outcome::NotNeeded);
        w.old_app(Old::SovaWatch, Some(ShellKind::Pwsh));
        let connected = w.settings();
        assert_eq!(run(&w.paths, &[], Some(&w.sidecar), 1), Outcome::NotNeeded, "MIKYAS_DATA_DIR: no roots");
        assert!(!w.paths.data_root().exists(), "nothing created");
        assert_eq!(w.settings(), connected);
    }

    /// A status line connected by either former name, in every shell, moves to Mikyas with ONLY
    /// the helper path swapped; Mikyas's Disconnect then restores the user's command exactly.
    #[test]
    fn a_connected_old_install_moves_and_only_the_helper_path_changes() {
        for old in OLD {
            for shell in SHELLS {
                let w = World::new(Some(ORIGINAL));
                // Both former names are installed; the status line runs `old`'s helper.
                w.old_app(old.other(), None);
                w.old_app(old, Some(shell));
                let other_before = tree(w.root(old.other()));
                let connected_by_old = w.settings();
                let legacy_shim = w.legacy_shim(old);
                assert!(connected_by_old.contains(&legacy_shim), "{old:?} {shell:?}");

                let outcome = w.run();
                let from = old.app();
                assert_eq!(outcome, Outcome::Moved { from, status_line: StatusLineMove::Switched, autostart: true });
                let expected = connected_by_old.replacen(&legacy_shim, &w.new_shim(), 1);
                assert_eq!(w.settings(), expected, "byte for byte ({old:?}, {shell:?})");
                assert_eq!(fs::read(connect::installed_shim(&w.paths)).unwrap(), b"new helper");
                assert_eq!(w.marker_from(), from.display_name);
                assert_eq!(notice(&outcome).unwrap().0, format!("Moved from {}", from.display_name));
                // Only the source's record is retired; the other old folder is untouched.
                assert!(w.root(old).join(RETIRED_WRAP_FILE).is_file() && !w.root(old).join("wrap.json").exists());
                assert_eq!(tree(w.root(old.other())), other_before);
                let rec: WrapRecord = serde_json::from_slice(&fs::read(w.paths.wrap_file()).unwrap()).unwrap();
                assert_eq!(rec.shim_path, w.new_shim());
                assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
                assert!(!w.old_sees_connected(old));

                // Idempotent: a second start changes nothing.
                let moved = w.settings();
                assert_eq!(w.run(), Outcome::NotNeeded);
                assert_eq!(w.settings(), moved);
                assert_eq!(notice(&Outcome::NotNeeded), None);

                // Mikyas's Disconnect restores the user's original command exactly.
                connect::disconnect(&w.paths, 2_000).unwrap();
                assert_eq!(w.settings(), ORIGINAL, "{old:?} {shell:?}");
            }
        }
    }

    /// Exactly the old app's own data is copied; its program files, helper, backups, logs and
    /// bookkeeping stay behind, and the old folder is left as it was (bar its retired record).
    #[test]
    fn only_the_old_apps_data_is_copied() {
        for old in OLD {
            let w = World::new(Some(ORIGINAL));
            w.old_app(old, Some(ShellKind::Pwsh));
            let before = tree(w.root(old));
            w.run();
            let names: Vec<String> = tree(w.paths.data_root()).into_iter().map(|(p, _)| p).collect();
            let mut expected = vec![
                "alerts.json",
                // The redacted backup of Claude Code's settings.json taken before the switch.
                "backups/settings-000000000001000.json",
                "bin/mikyas-capture.exe",
                "capture/00000000-0000-4000-8000-000000000001.json",
                "capture/_errors.log",
                "history.jsonl",
                "migrated.json",
                "positions.json",
                "settings.json",
                "state.json",
                "wrap.json",
            ];
            expected.sort_unstable();
            assert_eq!(names, expected, "{old:?}");
            for name in ["state.json", "alerts.json", "positions.json", "settings.json", "history.jsonl"] {
                assert_eq!(
                    fs::read(w.paths.data_root().join(name)).unwrap(),
                    fs::read(w.root(old).join(name)).unwrap()
                );
            }
            // Only the record was renamed in the old folder.
            let mut retired: Vec<(String, Vec<u8>)> = before
                .into_iter()
                .map(|(p, b)| (if p == "wrap.json" { RETIRED_WRAP_FILE.to_owned() } else { p }, b))
                .collect();
            retired.sort();
            assert_eq!(tree(w.root(old)), retired, "{old:?}");
        }
    }

    #[test]
    fn without_a_connection_the_newest_old_app_with_data_is_the_source() {
        // Both have data: SovaWatch (newer) wins.
        let w = World::new(Some(ORIGINAL));
        w.old_app(Old::SovaWatch, None);
        w.old_app(Old::Cuw, None);
        let outcome = w.run();
        let expected =
            Outcome::Moved { from: Old::SovaWatch.app(), status_line: StatusLineMove::NotConnected, autostart: true };
        assert_eq!(outcome, expected);
        assert_eq!(w.marker_from(), "SovaWatch");
        assert_eq!(w.history(), "{\"t\":0}\n");

        // SovaWatch only installed (program files, no data): Claude Usage Widget's data moves.
        let w = World::new(Some(ORIGINAL));
        w.old_app(Old::Cuw, None);
        fs::create_dir_all(w.root(Old::SovaWatch)).unwrap();
        fs::write(w.root(Old::SovaWatch).join("sovawatch.exe"), b"program file").unwrap();
        assert!(matches!(w.run(), Outcome::Moved { from, .. } if from == Old::Cuw.app()));
        assert_eq!(w.history(), "{\"t\":1}\n");

        // Only SovaWatch.
        let w = World::new(Some(ORIGINAL));
        w.old_app(Old::SovaWatch, None);
        assert!(matches!(w.run(), Outcome::Moved { from, .. } if from == Old::SovaWatch.app()));

        // An empty Claude Usage Widget folder still counts (as before); an empty SovaWatch one
        // (installed, never used) alone does not.
        let w = World::new(Some(ORIGINAL));
        fs::create_dir_all(w.root(Old::Cuw)).unwrap();
        assert!(matches!(w.run(), Outcome::Moved { from, .. } if from == Old::Cuw.app()));
        let w = World::new(Some(ORIGINAL));
        fs::create_dir_all(w.root(Old::SovaWatch)).unwrap();
        assert_eq!(w.run(), Outcome::NotNeeded);
        assert!(!w.paths.data_root().exists());
    }

    /// The status line decides over the data: when it runs Claude Usage Widget's helper, that is
    /// the source even though SovaWatch holds data too (and the other way round).
    #[test]
    fn the_helper_the_status_line_runs_picks_the_source() {
        for old in OLD {
            let w = World::new(Some(ORIGINAL));
            w.old_app(old.other(), None);
            w.old_app(old, Some(ShellKind::Bash));
            let outcome = w.run();
            assert!(
                matches!(&outcome, Outcome::Moved { from, status_line: StatusLineMove::Switched, .. } if *from == old.app()),
                "{old:?}: {outcome:?}"
            );
            assert_eq!(w.history(), format!("{{\"t\":{}}}\n", old as u8));
        }
    }

    /// The old uninstallers run `sovawatch.exe --disconnect --quiet` / `claude-usage-widget.exe
    /// --disconnect --quiet` with their released code and their own data folder. Against the
    /// moved status line both must be a no-op — with the retired record, and even with the old
    /// `wrap.json` still in place.
    #[test]
    fn the_old_apps_disconnect_leaves_the_moved_status_line_alone() {
        for old in OLD {
            for keep_old_record in [false, true] {
                let w = World::new(Some(ORIGINAL));
                w.old_app(old.other(), None);
                w.old_app(old, Some(ShellKind::Pwsh));
                let old_record = fs::read(w.root(old).join("wrap.json")).unwrap();
                w.run();
                if keep_old_record {
                    fs::write(w.root(old).join("wrap.json"), &old_record).unwrap();
                }
                let moved = w.settings();
                for uninstaller in OLD {
                    assert!(w.root(uninstaller).is_dir(), "its data folder is still there");
                    assert_eq!(w.old_disconnect(uninstaller), Ok(false), "{old:?} moved, {uninstaller:?} uninstalls");
                    assert_eq!(w.settings(), moved, "not removed or corrupted (record kept: {keep_old_record})");
                    // The old apps also no longer see a connection of their own.
                    assert!(!w.old_sees_connected(uninstaller));
                }
                // Mikyas is still connected and still disconnects exactly.
                assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
                connect::disconnect(&w.paths, 2_000).unwrap();
                assert_eq!(w.settings(), ORIGINAL);
            }
        }
    }

    /// Without any move (e.g. under a data-folder override) Mikyas still recognises the old
    /// helpers' commands and restores the user's command. With no record of the original literal
    /// it is written in canonical JSON (`\u00e9` becomes `é`); every other byte is kept.
    #[test]
    fn disconnect_works_on_the_old_prefixes_even_without_the_move() {
        for old in OLD {
            let w = World::new(Some(ORIGINAL));
            w.old_app(old, Some(ShellKind::Pwsh));
            assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
            connect::disconnect(&w.paths, 2_000).unwrap();
            assert_eq!(w.settings(), ORIGINAL.replace("\\u00e9", "\u{e9}"), "{old:?}");
        }
    }

    #[test]
    fn an_unconnected_old_install_moves_its_data_only() {
        for old in OLD {
            let w = World::new(Some(ORIGINAL));
            w.old_app(old, None);
            let outcome = w.run();
            let from = old.app();
            assert_eq!(outcome, Outcome::Moved { from, status_line: StatusLineMove::NotConnected, autostart: true });
            assert_eq!(w.settings(), ORIGINAL);
            assert!(!w.paths.wrap_file().exists(), "no record, so no false 'connection lost' warning");
            assert!(!w.paths.bin_dir().exists());
            assert!(notice(&outcome).unwrap().1.contains(&format!("uninstall {} from", from.display_name)));
        }
    }

    #[test]
    fn autostart_is_carried_over_only_when_it_was_on() {
        let w = World::new(Some(ORIGINAL));
        w.old_app(Old::SovaWatch, None);
        fs::write(w.root(Old::SovaWatch).join("settings.json"), r#"{"start_with_windows":false}"#).unwrap();
        assert!(matches!(w.run(), Outcome::Moved { autostart: false, .. }));
    }

    #[test]
    fn a_failed_switch_keeps_the_old_status_line_and_its_record() {
        for old in OLD {
            let w = World::new(Some(ORIGINAL));
            w.old_app(old, Some(ShellKind::Pwsh));
            let before = w.settings();
            let outcome = run(&w.paths, &w.legacy, None, 1_000);
            let Outcome::Moved { status_line: StatusLineMove::Failed(reason), .. } = &outcome else {
                panic!("{outcome:?}")
            };
            assert!(reason.contains("helper"), "{reason}");
            assert_eq!(w.settings(), before);
            assert!(w.root(old).join("wrap.json").is_file(), "the old uninstaller can still restore it");
            // Settings shows the old helper's command as Connected and offers only Disconnect
            // there, so the notice must ask for Disconnect, then Connect.
            assert!(matches!(connect::status(&w.paths), ConnectionStatus::Connected { .. }));
            let body = notice(&outcome).unwrap().1;
            let name = old.app().display_name;
            assert!(body.contains("Disconnect") && body.contains("Connect again") && body.contains(name), "{body}");
            // Mikyas can still undo it exactly with the copied record.
            connect::disconnect(&w.paths, 2_000).unwrap();
            assert_eq!(w.settings(), ORIGINAL);
            // ...and the Connect that follows wraps with Mikyas's own helper.
            connect::connect(&w.connect_env(), 3_000).unwrap();
            let text = w.settings();
            assert!(
                LEGACY_SHIM_EXE_NAMES.iter().all(|n| !text.contains(n)) && text.matches(SHIM_EXE_NAME).count() == 1,
                "{text}"
            );
            connect::disconnect(&w.paths, 4_000).unwrap();
            assert_eq!(w.settings(), ORIGINAL);
        }
    }

    #[test]
    fn existing_new_data_is_never_overwritten() {
        for old in OLD {
            let w = World::new(None);
            w.old_app(old, None);
            fs::create_dir_all(w.paths.data_root()).unwrap();
            fs::write(w.paths.history_file(), "mine\n").unwrap();
            w.run();
            assert_eq!(w.history(), "mine\n");
            // Existing settings mean Mikyas already ran: nothing moves.
            let w2 = World::new(None);
            w2.old_app(old, None);
            fs::create_dir_all(w2.paths.data_root()).unwrap();
            fs::write(w2.paths.settings_file(), "{}").unwrap();
            assert_eq!(w2.run(), Outcome::NotNeeded);
            assert!(!w2.paths.history_file().exists());
            // So does an earlier move's marker.
            let w3 = World::new(None);
            w3.old_app(old, None);
            fs::create_dir_all(w3.paths.data_root()).unwrap();
            fs::write(w3.paths.data_root().join(MARKER_FILE), "{}").unwrap();
            assert_eq!(w3.run(), Outcome::NotNeeded);
            assert!(!w3.paths.history_file().exists());
        }
    }

    /// Connect in Mikyas after a failed switch (what the notice asks for) re-wraps the old
    /// command instead of nesting a second helper, and keeps the copied record's exact literal.
    #[test]
    fn connect_replaces_the_old_helper_instead_of_nesting() {
        for old in OLD {
            let w = World::new(Some(ORIGINAL));
            w.old_app(old, Some(ShellKind::Pwsh));
            let failed = run(&w.paths, &w.legacy, None, 1_000);
            assert!(matches!(failed, Outcome::Moved { status_line: StatusLineMove::Failed(_), .. }));
            connect::connect(&w.connect_env(), 3_000).unwrap();
            let text = w.settings();
            assert!(
                LEGACY_SHIM_EXE_NAMES.iter().all(|n| !text.contains(n)) && text.matches(SHIM_EXE_NAME).count() == 1,
                "{text}"
            );
            connect::disconnect(&w.paths, 4_000).unwrap();
            assert_eq!(w.settings(), ORIGINAL);
        }
    }
}
