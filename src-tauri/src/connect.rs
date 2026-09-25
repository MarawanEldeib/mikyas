//! Connect / Disconnect Claude Code: file handling around `cuw_core::claude_settings`.
//!
//! Every path comes from a [`Paths`] value, so the whole flow is tested against temp dirs; the
//! real `~/.claude/settings.json` is only touched when the user clicks Connect in the app.
//!
//! Connect (real run):
//! 1. copy the shim sidecar into `<data_root>/bin/` (only when its bytes differ),
//! 2. compute the edit with `claude_settings::connect`,
//! 3. back up the current file to `<data_root>/backups/` (newest [`KEEP_BACKUPS`] kept),
//! 4. write `wrap.json`, re-read the settings file and only write if it is unchanged since step 2
//!    (Claude Code writes this file too; retried a few times), atomically (temp + rename),
//! 5. self-test: run the original and the wrapped command through the detected shell with the
//!    same synthetic statusline JSON (captures redirected to a temp dir) and compare stdout with
//!    digits stripped (clocks and timers differ between the two runs).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cuw_core::claude_settings::{self, SettingsError, Status, WrapRecord};
use cuw_core::cmdline::{self, SHIM_EXE_NAME, ShellKind, WrapMode};
use cuw_core::fingerprint::Fnv64;
use cuw_core::paths::{CLAUDE_CONFIG_DIR_ENV, DATA_DIR_ENV, Paths};
use cuw_core::saferead::{ReadError, SafeReader};
use cuw_core::time::Ms;
use serde::Serialize;

use crate::state::{save_json, write_atomic};

pub const KEEP_BACKUPS: usize = 10;
const MAX_SETTINGS_BYTES: u64 = 4 * 1024 * 1024;
const CAS_ATTEMPTS: u32 = 3;
const SELFTEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Keeps console windows of spawned shells from flashing up.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `ConnectionStatus` in `src/lib/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionStatus {
    NotConfigured,
    Foreign { command: Option<String> },
    Connected { mode: WrapMode, original: Option<String> },
    Error { message: String },
}

/// `ConnectPreview` in `src/lib/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectPreview {
    pub before: Option<String>,
    pub after: String,
    pub shell: ShellKind,
    pub warnings: Vec<String>,
    pub selftest_ok: Option<bool>,
}

/// The shell Claude Code will run the statusline command in, and its executable.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    pub kind: ShellKind,
    pub exe: PathBuf,
}

/// Decides which shell Claude Code uses for the statusline command on this machine.
///
/// TODO(diag spike): this mirrors Claude Code's documented Windows behaviour (Git Bash when it is
/// installed or `CLAUDE_CODE_GIT_BASH_PATH` is set, otherwise PowerShell). It must be confirmed
/// with `cuw-capture --diag` on a machine with Git Bash and on one without Git Bash / pwsh 7
/// before release; keep every shell decision inside this one function.
pub fn detect_shell() -> Shell {
    if let Some(bash) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Shell { kind: ShellKind::Bash, exe: bash };
    }
    let mut candidates = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(base).join("Git");
            candidates.push(base.join("bin").join("bash.exe"));
            candidates.push(base.join("usr").join("bin").join("bash.exe"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs").join("Git").join("bin").join("bash.exe"));
    }
    if let Some(bash) = candidates.into_iter().find(|p| p.is_file()) {
        return Shell { kind: ShellKind::Bash, exe: bash };
    }
    if let Some(pwsh) = find_on_path("pwsh.exe").filter(|p| pwsh_at_least_7_4(p)) {
        return Shell { kind: ShellKind::Pwsh, exe: pwsh };
    }
    let legacy = find_on_path("powershell.exe").unwrap_or_else(|| PathBuf::from("powershell.exe"));
    Shell { kind: ShellKind::LegacyPowerShell, exe: legacy }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn pwsh_at_least_7_4(exe: &Path) -> bool {
    let mut cmd = Command::new(exe);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()"]);
    let Ok(out) = run_with_timeout(cmd, None, Duration::from_secs(10)) else { return false };
    parse_version_at_least(&String::from_utf8_lossy(&out.stdout), 7, 4)
}

fn parse_version_at_least(text: &str, major: u32, minor: u32) -> bool {
    let mut parts = text.trim().split(['.', '-']).map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(a), Some(b)) => (a, b) >= (major, minor),
        _ => false,
    }
}

/// The shim sidecar shipped next to the app executable.
pub fn find_sidecar() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [
        dir.join(SHIM_EXE_NAME),
        dir.join("cuw-capture-x86_64-pc-windows-msvc.exe"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Everything Connect needs; injectable for tests.
pub struct ConnectEnv {
    pub paths: Paths,
    pub shell: Shell,
    /// Shim executable to install (the sidecar); `None` → a real connect fails.
    pub shim_source: Option<PathBuf>,
    /// Run the self-test after a real connect.
    pub selftest: bool,
}

impl ConnectEnv {
    pub fn detect(paths: Paths) -> Self {
        Self {
            paths,
            shell: detect_shell(),
            shim_source: find_sidecar(),
            selftest: true,
        }
    }

    fn installed_shim(&self) -> PathBuf {
        self.paths.bin_dir().join(SHIM_EXE_NAME)
    }

    fn shim_command_path(&self) -> String {
        cmdline::shim_path_for_command(&self.installed_shim())
    }
}

/// Reads `settings.json` through the allowlist; a missing file reads as empty.
fn read_settings(paths: &Paths) -> Result<Vec<u8>, String> {
    let reader = SafeReader::new(paths);
    match reader.read(&paths.claude_settings(), MAX_SETTINGS_BYTES) {
        Ok(b) => Ok(b),
        Err(ReadError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read Claude Code settings: {e}")),
    }
}

fn settings_error(e: SettingsError) -> String {
    e.to_string()
}

/// Maps `claude_settings::status` of the current file.
pub fn status(paths: &Paths) -> ConnectionStatus {
    let bytes = match read_settings(paths) {
        Ok(b) => b,
        Err(message) => return ConnectionStatus::Error { message },
    };
    match claude_settings::status(&bytes) {
        Ok(Status::NotConfigured) => ConnectionStatus::NotConfigured,
        Ok(Status::Foreign { command }) => ConnectionStatus::Foreign { command },
        Ok(Status::Connected { mode, original, .. }) => ConnectionStatus::Connected { mode, original },
        Err(e) => ConnectionStatus::Error { message: settings_error(e) },
    }
}

fn warnings(paths: &Paths) -> Vec<String> {
    let mut w = Vec::new();
    if std::env::var_os(CLAUDE_CONFIG_DIR_ENV).is_some_and(|v| !v.is_empty()) {
        w.push(format!(
            "CLAUDE_CONFIG_DIR is set, so {} is edited. Claude Code sessions started without that \
             variable use ~/.claude and will not be captured.",
            paths.claude_settings().display()
        ));
    }
    w
}

/// Computes what Connect would write, without writing anything.
pub fn preview(env: &ConnectEnv, now: Ms) -> Result<ConnectPreview, String> {
    let bytes = read_settings(&env.paths)?;
    let (_, record) = claude_settings::connect(&bytes, &env.shim_command_path(), env.shell.kind, now)
        .map_err(settings_error)?;
    let after = cmdline::wrap(record.original_command.as_deref(), &env.shim_command_path(), env.shell.kind)
        .map_err(|e| e.to_string())?
        .command;
    Ok(ConnectPreview {
        before: record.original_command,
        after,
        shell: env.shell.kind,
        warnings: warnings(&env.paths),
        selftest_ok: None,
    })
}

/// Performs Connect (see the module docs).
pub fn connect(env: &ConnectEnv, now: Ms) -> Result<ConnectPreview, String> {
    let source = env
        .shim_source
        .as_deref()
        .ok_or("the capture shim (cuw-capture.exe) was not found next to the app")?;
    install_shim(source, &env.installed_shim()).map_err(|e| format!("cannot install the capture shim: {e}"))?;
    let shim = env.shim_command_path();
    let settings_path = env.paths.claude_settings();

    let mut record = None;
    for attempt in 0..CAS_ATTEMPTS {
        let bytes = read_settings(&env.paths)?;
        let (next, rec) = claude_settings::connect(&bytes, &shim, env.shell.kind, now).map_err(settings_error)?;
        let rec = keep_first_record(&env.paths.wrap_file(), rec);
        if next == bytes {
            save_json(&env.paths.wrap_file(), &rec).map_err(|e| format!("cannot write wrap.json: {e}"))?;
            record = Some(rec);
            break;
        }
        if !bytes.is_empty() {
            backup(&env.paths.backups_dir(), &bytes, now).map_err(|e| format!("backup failed: {e}"))?;
        }
        save_json(&env.paths.wrap_file(), &rec).map_err(|e| format!("cannot write wrap.json: {e}"))?;
        // Compare-and-swap: Claude Code may have rewritten the file meanwhile.
        if read_settings(&env.paths)? != bytes {
            if attempt + 1 == CAS_ATTEMPTS {
                return Err("settings.json keeps changing; try again in a moment".into());
            }
            continue;
        }
        write_atomic(&settings_path, &next).map_err(|e| format!("cannot write settings.json: {e}"))?;
        record = Some(rec);
        break;
    }
    let record = record.ok_or("settings.json keeps changing; try again in a moment")?;

    match status(&env.paths) {
        ConnectionStatus::Connected { .. } => {}
        other => return Err(format!("connect did not take effect ({other:?})")),
    }

    let after = cmdline::wrap(record.original_command.as_deref(), &shim, env.shell.kind)
        .map_err(|e| e.to_string())?
        .command;
    let selftest_ok = env
        .selftest
        .then(|| selftest(&env.shell, record.original_command.as_deref(), &after));
    Ok(ConnectPreview {
        before: record.original_command,
        after,
        shell: env.shell.kind,
        warnings: warnings(&env.paths),
        selftest_ok,
    })
}

/// A previous record for the same original keeps its exact raw literal (see `claude_settings`).
fn keep_first_record(wrap_file: &Path, rec: WrapRecord) -> WrapRecord {
    let prev: Option<WrapRecord> = fs::read(wrap_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    match prev {
        Some(p) if p.original_command == rec.original_command && p.original_command_raw.is_some() => WrapRecord {
            original_command_raw: p.original_command_raw,
            ..rec
        },
        _ => rec,
    }
}

/// Undoes Connect. Nothing of ours → unchanged. Returns the resulting status.
pub fn disconnect(paths: &Paths, now: Ms) -> Result<ConnectionStatus, String> {
    let wrap: Option<WrapRecord> = fs::read(paths.wrap_file()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    for attempt in 0..CAS_ATTEMPTS {
        let bytes = read_settings(paths)?;
        let Some(next) = claude_settings::disconnect(&bytes, wrap.as_ref()).map_err(settings_error)? else {
            return Ok(status(paths));
        };
        backup(&paths.backups_dir(), &bytes, now).map_err(|e| format!("backup failed: {e}"))?;
        if read_settings(paths)? != bytes {
            if attempt + 1 == CAS_ATTEMPTS {
                return Err("settings.json keeps changing; try again in a moment".into());
            }
            continue;
        }
        write_atomic(&paths.claude_settings(), &next).map_err(|e| format!("cannot write settings.json: {e}"))?;
        let _ = fs::remove_file(paths.wrap_file());
        return Ok(status(paths));
    }
    Err("settings.json keeps changing; try again in a moment".into())
}

/// Copies the shim unless an identical copy is already installed.
pub fn install_shim(source: &Path, target: &Path) -> io::Result<bool> {
    let src = fs::read(source)?;
    if fs::read(target).is_ok_and(|cur| hash(&cur) == hash(&src) && cur.len() == src.len()) {
        return Ok(false);
    }
    write_atomic(target, &src)?;
    Ok(true)
}

fn hash(bytes: &[u8]) -> u64 {
    Fnv64::new().write(bytes).finish()
}

/// Writes `settings-<ms>.json` and keeps only the newest [`KEEP_BACKUPS`].
pub fn backup(dir: &Path, bytes: &[u8], now: Ms) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut path = dir.join(format!("settings-{now:015}.json"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("settings-{now:015}-{n}.json"));
        n += 1;
    }
    write_atomic(&path, bytes)?;
    let mut backups: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("settings-") && n.ends_with(".json"))
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(KEEP_BACKUPS);
    for old in &backups[..excess] {
        let _ = fs::remove_file(old);
    }
    Ok(path)
}

// ---- self-test ----

/// Synthetic statusline input (no real session data).
fn synthetic_input() -> Vec<u8> {
    let home = dirs_home();
    serde_json::json!({
        "hook_event_name": "Status",
        "session_id": "00000000-0000-4000-8000-00000000c0de",
        "transcript_path": "",
        "cwd": home,
        "model": { "id": "claude-opus-5-5", "display_name": "Opus 5.5" },
        "workspace": { "current_dir": home, "project_dir": home },
        "version": "2.0.0",
        "output_style": { "name": "default" },
        "cost": { "total_cost_usd": 0.0, "total_duration_ms": 1000, "total_api_duration_ms": 500,
                  "total_lines_added": 0, "total_lines_removed": 0 },
        "exceeds_200k_tokens": false,
        "context_window": { "used_percentage": 12.5, "context_window_size": 200000 },
        "rate_limits": {
            "five_hour": { "used_percentage": 22.0, "resets_at": 4102444800i64 },
            "seven_day": { "used_percentage": 61.0, "resets_at": 4102444800i64 }
        }
    })
    .to_string()
    .into_bytes()
}

fn dirs_home() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into())
}

/// True when the wrapped command prints what the original prints (digits ignored). Without an
/// original (Default form) the wrapped command only has to succeed and print something.
pub fn selftest(shell: &Shell, original: Option<&str>, wrapped: &str) -> bool {
    let Ok(tmp) = tempdir() else { return false };
    let input = synthetic_input();
    let run = |cmd: &str| -> Option<Vec<u8>> {
        let mut c = shell_command(shell, cmd);
        c.env(DATA_DIR_ENV, &tmp);
        run_with_timeout(c, Some(&input), SELFTEST_TIMEOUT).ok().map(|o| o.stdout)
    };
    let result = match original {
        Some(orig) => match (run(orig), run(wrapped)) {
            (Some(a), Some(b)) => strip_digits(&a) == strip_digits(&b),
            _ => false,
        },
        None => run(wrapped).is_some_and(|out| !out.is_empty()),
    };
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn tempdir() -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("cuw-selftest-{}-{}", std::process::id(), cuw_core::time::now_ms()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn strip_digits(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|b| !b.is_ascii_digit()).collect()
}

/// How the shell is invoked for a statusline command.
pub fn shell_command(shell: &Shell, cmd: &str) -> Command {
    let mut c = Command::new(&shell.exe);
    match shell.kind {
        ShellKind::Bash => {
            c.args(["-c", cmd]);
        }
        ShellKind::Cmd => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.raw_arg("/d /s /c \"").raw_arg(cmd).raw_arg("\"");
            }
            #[cfg(not(windows))]
            c.args(["/d", "/s", "/c", cmd]);
        }
        ShellKind::Pwsh | ShellKind::LegacyPowerShell => {
            c.args(["-NoProfile", "-NonInteractive", "-Command", cmd]);
        }
    }
    c
}

pub struct Output {
    pub stdout: Vec<u8>,
}

/// Spawns without a console window, feeds `input`, collects stdout, kills after `timeout`.
fn run_with_timeout(mut cmd: Command, input: Option<&[u8]>, timeout: Duration) -> io::Result<Output> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
        let data = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let mut stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let stdout = reader.join().unwrap_or_default();
    Ok(Output { stdout })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = r#"{
  "model": "opus",
  "statusLine": {
    "type": "command",
    "command": "pwsh -NoProfile -File \"C:/Users/tester/.claude/statusline.ps1\"",
    "padding": 0
  },
  "theme": "dark"
}
"#;

    fn setup(settings: Option<&str>) -> (tempfile::TempDir, ConnectEnv) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_roots(
            tmp.path().join(".claude"),
            vec![tmp.path().join("Roaming").join("Claude")],
            tmp.path().join("data"),
        );
        if let Some(s) = settings {
            fs::create_dir_all(paths.claude_home()).unwrap();
            fs::write(paths.claude_settings(), s).unwrap();
        }
        let shim = tmp.path().join("sidecar").join(SHIM_EXE_NAME);
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, b"fake shim").unwrap();
        let env = ConnectEnv {
            paths,
            shell: Shell {
                kind: ShellKind::Pwsh,
                exe: PathBuf::from("pwsh.exe"),
            },
            shim_source: Some(shim),
            selftest: false,
        };
        (tmp, env)
    }

    #[test]
    fn preview_writes_nothing() {
        let (_t, env) = setup(Some(ORIGINAL));
        let p = preview(&env, 1).unwrap();
        assert_eq!(p.before.as_deref(), Some(r#"pwsh -NoProfile -File "C:/Users/tester/.claude/statusline.ps1""#));
        assert!(p.after.starts_with("& \""), "{}", p.after);
        assert!(p.after.ends_with(r#"--tee | pwsh -NoProfile -File "C:/Users/tester/.claude/statusline.ps1""#));
        assert_eq!(p.selftest_ok, None);
        assert_eq!(fs::read_to_string(env.paths.claude_settings()).unwrap(), ORIGINAL);
        assert!(!env.paths.wrap_file().exists());
        assert!(!env.paths.bin_dir().exists());
    }

    #[test]
    fn connect_then_disconnect_roundtrips_bytes() {
        let (_t, env) = setup(Some(ORIGINAL));
        let p = connect(&env, 1_000).unwrap();
        assert_eq!(p.selftest_ok, None);
        assert!(env.paths.bin_dir().join(SHIM_EXE_NAME).is_file());
        assert!(env.paths.wrap_file().is_file());
        assert!(matches!(status(&env.paths), ConnectionStatus::Connected { mode: WrapMode::Pipe, .. }));
        let connected = fs::read_to_string(env.paths.claude_settings()).unwrap();
        assert!(connected.contains("cuw-capture.exe"));
        assert!(connected.contains("\"padding\": 0"), "other keys untouched");

        // Idempotent.
        connect(&env, 2_000).unwrap();
        assert_eq!(fs::read_to_string(env.paths.claude_settings()).unwrap(), connected);

        let st = disconnect(&env.paths, 3_000).unwrap();
        assert!(matches!(st, ConnectionStatus::Foreign { .. }));
        assert_eq!(fs::read_to_string(env.paths.claude_settings()).unwrap(), ORIGINAL);
        assert!(!env.paths.wrap_file().exists());
        // Nothing to undo now.
        assert_eq!(disconnect(&env.paths, 4_000).unwrap(), st);
        let backups = fs::read_dir(env.paths.backups_dir()).unwrap().count();
        assert_eq!(backups, 2, "one backup per write");
    }

    #[test]
    fn connect_without_settings_file_and_disconnect() {
        let (_t, env) = setup(None);
        assert_eq!(status(&env.paths), ConnectionStatus::NotConfigured);
        let p = connect(&env, 1).unwrap();
        assert_eq!(p.before, None);
        assert!(p.after.ends_with("--default"));
        assert!(matches!(status(&env.paths), ConnectionStatus::Connected { mode: WrapMode::Default, .. }));
        disconnect(&env.paths, 2).unwrap();
        assert_eq!(fs::read_to_string(env.paths.claude_settings()).unwrap(), "{}\n");
        assert_eq!(status(&env.paths), ConnectionStatus::NotConfigured);
    }

    #[test]
    fn missing_sidecar_and_jsonc_are_errors() {
        let (_t, mut env) = setup(Some(ORIGINAL));
        env.shim_source = None;
        assert!(connect(&env, 1).unwrap_err().contains("cuw-capture.exe"));
        let (_t2, env2) = setup(Some("{ // comment\n}"));
        assert!(preview(&env2, 1).is_err());
        assert!(matches!(status(&env2.paths), ConnectionStatus::Error { .. }));
    }

    #[test]
    fn shim_is_copied_only_when_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a.exe");
        let dst = tmp.path().join("bin").join(SHIM_EXE_NAME);
        fs::write(&src, b"v1").unwrap();
        assert!(install_shim(&src, &dst).unwrap());
        assert!(!install_shim(&src, &dst).unwrap());
        fs::write(&src, b"v2").unwrap();
        assert!(install_shim(&src, &dst).unwrap());
        assert_eq!(fs::read(&dst).unwrap(), b"v2");
    }

    #[test]
    fn backups_keep_newest_ten() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..13 {
            backup(tmp.path(), b"x", 1_000 + i).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), KEEP_BACKUPS);
        assert_eq!(names[0], "settings-000000000001003.json");
    }

    #[test]
    fn versions_and_digits() {
        assert!(parse_version_at_least("7.4.0\r\n", 7, 4));
        assert!(parse_version_at_least("7.5.1", 7, 4));
        assert!(parse_version_at_least("8.0.0-preview.1", 7, 4));
        assert!(!parse_version_at_least("7.3.9", 7, 4));
        assert!(!parse_version_at_least("5.1.26100.1", 7, 4));
        assert!(!parse_version_at_least("", 7, 4));
        assert_eq!(strip_digits(b"a1b22c"), b"abc");
    }

    #[test]
    fn preview_serialises_like_the_ui_contract() {
        let p = ConnectPreview {
            before: None,
            after: "x".into(),
            shell: ShellKind::LegacyPowerShell,
            warnings: vec![],
            selftest_ok: Some(true),
        };
        let v = serde_json::to_value(p).unwrap();
        assert_eq!(v["shell"], "legacy_power_shell");
        let s = serde_json::to_value(ConnectionStatus::Connected {
            mode: WrapMode::PipeGrouped,
            original: None,
        })
        .unwrap();
        assert_eq!(s, serde_json::json!({"state":"connected","mode":"pipe_grouped","original":null}));
    }

    /// Runs the real shim through the real shell when both are available (release build of
    /// cuw-capture in the target dir). Uses only temp files.
    #[cfg(windows)]
    #[test]
    fn selftest_with_real_shim_when_available() {
        let Some(target) = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) else {
            eprintln!("skipped: CARGO_TARGET_DIR not set");
            return;
        };
        let built = target.join("release").join(SHIM_EXE_NAME);
        if !built.is_file() {
            eprintln!("skipped: {} not built", built.display());
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("bin").join(SHIM_EXE_NAME);
        install_shim(&built, &shim).unwrap();
        let shim_cmd = cmdline::shim_path_for_command(&shim);
        let cmd_exe = Shell {
            kind: ShellKind::Cmd,
            exe: PathBuf::from(std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into())),
        };
        let original = "findstr session_id";
        let wrapped = cmdline::wrap(Some(original), &shim_cmd, ShellKind::Cmd).unwrap().command;
        assert!(selftest(&cmd_exe, Some(original), &wrapped));
        assert!(!selftest(&cmd_exe, Some(original), "echo different"));
    }
}
