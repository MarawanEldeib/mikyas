//! Connect / Disconnect Claude Code: file handling around `cuw_core::claude_settings`.
//!
//! Every path comes from a [`Paths`] value, so the whole flow is tested against temp dirs; the
//! real `~/.claude/settings.json` is only touched when the user clicks Connect in the app.
//!
//! Connect (real run):
//! 1. copy the shim sidecar into `<data_root>/bin/` (only when its bytes differ),
//! 2. compute the edit with `claude_settings::connect`,
//! 3. back up the current file to `<data_root>/backups/`, redacted (see [`backup`]),
//! 4. write `wrap.json`, re-read the settings file and only write if it is unchanged since step 2
//!    (Claude Code writes this file too; retried a few times), atomically (temp + rename). When
//!    the settings file is not written after all, the previous `wrap.json` is put back,
//! 5. self-test: run the original and the wrapped command through the detected shell with the
//!    same synthetic statusline JSON (captures redirected to a temp dir) and compare stdout with
//!    digits stripped (clocks and timers differ between the two runs).
//!
//! A successful Disconnect deletes `wrap.json` and the backups. While connected, the installed
//! shim is compared byte for byte with the bundled one and rewritten when they differ
//! ([`refresh_installed_shim`]: after an app update, or when it was changed on disk).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cuw_core::claude_settings::{self, SettingsError, Status, WrapRecord};
use cuw_core::cmdline::{self, SHIM_EXE_NAME, ShellKind, WrapMode};
use cuw_core::paths::{CLAUDE_CONFIG_DIR_ENV, DATA_DIR_ENV, Paths};
use cuw_core::saferead::{ReadError, SafeReader};
use cuw_core::time::{DAY_MS, Ms};
use serde::Serialize;

use crate::state::{save_json, write_atomic};

/// Backups of Claude Code's `settings.json` kept (newest first) ...
pub const KEEP_BACKUPS: usize = 3;
/// ... and never longer than this.
pub const BACKUP_MAX_AGE_MS: Ms = 30 * DAY_MS;
/// Replaces secret-looking values in backups.
pub const REDACTED: &str = "<redacted>";
/// Replaces secret-looking values in the commands the Connect preview shows.
pub const MASK: &str = "\u{2022}\u{2022}\u{2022}";
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
    Connected {
        mode: WrapMode,
        original: Option<String>,
        /// The shim the status line runs (as written in the command); not part of the UI contract.
        #[serde(skip)]
        shim_path: String,
    },
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
/// Mirrors Claude Code's documented Windows behaviour: Git Bash when `CLAUDE_CODE_GIT_BASH_PATH`
/// names it or it is installed in a usual place, else pwsh 7.4+, else Windows PowerShell. A
/// machine where Claude Code picks differently is caught by the self-test after Connect (and
/// `cuw-capture --diag` shows the shell actually used); every shell decision stays in here.
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
        installed_shim(&self.paths)
    }

    fn shim_command_path(&self) -> String {
        cmdline::shim_path_for_command(&self.installed_shim())
    }
}

/// Where Connect installs the shim: `<data_root>/bin/cuw-capture.exe`.
pub fn installed_shim(paths: &Paths) -> PathBuf {
    paths.bin_dir().join(SHIM_EXE_NAME)
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
        Ok(Status::Connected { mode, original, shim_path }) => ConnectionStatus::Connected { mode, original, shim_path },
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
        before: record.original_command.as_deref().map(mask_secrets),
        after: mask_secrets(&after),
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

    let wrap_file = env.paths.wrap_file();
    let previous_wrap = fs::read(&wrap_file).ok();
    let result = write_connected(env, &shim, now);
    if result.is_err() {
        // settings.json was not changed: leave no record of a connection that did not happen.
        match &previous_wrap {
            Some(bytes) => {
                let _ = write_atomic(&wrap_file, bytes);
            }
            None => {
                let _ = fs::remove_file(&wrap_file);
            }
        }
    }
    let record = result?;

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
        before: record.original_command.as_deref().map(mask_secrets),
        after: mask_secrets(&after),
        shell: env.shell.kind,
        warnings: warnings(&env.paths),
        selftest_ok,
    })
}

/// Steps 2-4 of Connect; returns the record written to `wrap.json`.
fn write_connected(env: &ConnectEnv, shim: &str, now: Ms) -> Result<WrapRecord, String> {
    let wrap_file = env.paths.wrap_file();
    for attempt in 0..CAS_ATTEMPTS {
        let bytes = read_settings(&env.paths)?;
        let (next, rec) = claude_settings::connect(&bytes, shim, env.shell.kind, now).map_err(settings_error)?;
        let rec = keep_first_record(&wrap_file, rec);
        if next == bytes {
            save_json(&wrap_file, &rec).map_err(|e| format!("cannot write wrap.json: {e}"))?;
            return Ok(rec);
        }
        if !bytes.is_empty() {
            backup(&env.paths.backups_dir(), &bytes, now).map_err(|e| format!("backup failed: {e}"))?;
        }
        save_json(&wrap_file, &rec).map_err(|e| format!("cannot write wrap.json: {e}"))?;
        // Compare-and-swap: Claude Code may have rewritten the file meanwhile.
        if read_settings(&env.paths)? != bytes {
            if attempt + 1 == CAS_ATTEMPTS {
                break;
            }
            continue;
        }
        write_atomic(&env.paths.claude_settings(), &next).map_err(|e| format!("cannot write settings.json: {e}"))?;
        return Ok(rec);
    }
    Err("settings.json keeps changing; try again in a moment".into())
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

/// Undoes Connect. Nothing of ours → `settings.json` unchanged (a stale `wrap.json` is still
/// removed). After a successful write, `wrap.json` and the backups are deleted. Returns the
/// resulting status.
pub fn disconnect(paths: &Paths, now: Ms) -> Result<ConnectionStatus, String> {
    let wrap: Option<WrapRecord> = fs::read(paths.wrap_file()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    for attempt in 0..CAS_ATTEMPTS {
        let bytes = read_settings(paths)?;
        let Some(next) = claude_settings::disconnect(&bytes, wrap.as_ref()).map_err(settings_error)? else {
            let _ = fs::remove_file(paths.wrap_file());
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
        let _ = fs::remove_dir_all(paths.backups_dir());
        return Ok(status(paths));
    }
    Err("settings.json keeps changing; try again in a moment".into())
}

/// Copies the shim unless an identical copy (byte for byte) is already installed.
pub fn install_shim(source: &Path, target: &Path) -> io::Result<bool> {
    let src = fs::read(source)?;
    if fs::read(target).is_ok_and(|cur| cur == src) {
        return Ok(false);
    }
    write_atomic(target, &src)?;
    Ok(true)
}

/// While Connect's record exists, rewrites the installed shim when it differs from the bundled
/// `source` (an app update shipped a new one, or the file was changed). Returns whether it wrote.
/// Fails while Claude Code is running the shim (a running exe cannot be replaced); the next
/// check retries.
pub fn refresh_installed_shim(paths: &Paths, source: Option<&Path>) -> io::Result<bool> {
    let Some(source) = source else { return Ok(false) };
    if !paths.wrap_file().is_file() {
        return Ok(false);
    }
    install_shim(source, &installed_shim(paths))
}

/// Writes `settings-<ms>.json` with every secret-looking value replaced (see [`redact`]), then
/// keeps only the newest [`KEEP_BACKUPS`] no older than [`BACKUP_MAX_AGE_MS`]. Bytes that are not
/// JSON are not backed up at all (`Ok(None)`): the raw file could hold keys.
pub fn backup(dir: &Path, bytes: &[u8], now: Ms) -> io::Result<Option<PathBuf>> {
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(text) else {
        prune_backups(dir, now);
        return Ok(None);
    };
    redact(&mut value, false);
    let mut out = serde_json::to_vec_pretty(&value).map_err(io::Error::other)?;
    out.push(b'\n');
    fs::create_dir_all(dir)?;
    let mut path = dir.join(format!("settings-{now:015}.json"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("settings-{now:015}-{n}.json"));
        n += 1;
    }
    write_atomic(&path, &out)?;
    prune_backups(dir, now);
    Ok(Some(path))
}

/// Deletes all but the newest [`KEEP_BACKUPS`] backups, and any older than [`BACKUP_MAX_AGE_MS`].
fn prune_backups(dir: &Path, now: Ms) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut backups: Vec<(PathBuf, Option<Ms>)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_prefix("settings-")?.strip_suffix(".json")?;
            let ms = stem.split('-').next().and_then(|t| t.parse::<Ms>().ok());
            Some((e.path(), ms))
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(KEEP_BACKUPS);
    for (i, (path, ms)) in backups.iter().enumerate() {
        let too_old = ms.is_some_and(|t| now.saturating_sub(t) > BACKUP_MAX_AGE_MS);
        if i < excess || too_old {
            let _ = fs::remove_file(path);
        }
    }
}

/// Key names whose values are treated as secrets.
fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["key", "token", "secret", "password", "passwd", "auth", "credential", "cookie"]
        .iter()
        .any(|w| key.contains(w))
}

/// Replaces every value under an `env` object, and every scalar whose key looks secret
/// (`apiKeyHelper`, `awsAuthRefresh`, `…_TOKEN`, `password`, …), with [`REDACTED`].
fn redact(value: &mut serde_json::Value, secret: bool) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let env = k.eq_ignore_ascii_case("env");
                redact(v, secret || env || is_secret_key(k));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact(v, secret)),
        Value::Null => {}
        _ if secret => *value = Value::String(REDACTED.into()),
        _ => {}
    }
}

/// The command with secret-looking values replaced by [`MASK`], for display only: the value of
/// `NAME=value` / `$env:NAME = "value"` where NAME looks secret (see [`is_secret_key`]), the
/// token after `Bearer `, and the rest of an `sk-ant-` key.
pub fn mask_secrets(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    loop {
        let Some((start, len)) = next_secret(rest) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        out.push_str(MASK);
        rest = &rest[start + len..];
    }
}

/// Start and length of the first secret value in `s`.
fn next_secret(s: &str) -> Option<(usize, usize)> {
    let lower = s.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    let mut consider = |start: usize, len: usize| {
        if len > 0 && best.is_none_or(|(b, _)| start < b) {
            best = Some((start, len));
        }
    };
    for marker in ["sk-ant-", "bearer "] {
        if let Some(at) = lower.find(marker) {
            let start = at + marker.len();
            consider(start, value_len(&s[start..]));
        }
    }
    // NAME=value (PowerShell allows spaces around the `=`); `==` is a comparison.
    for (i, _) in s.match_indices('=') {
        let after = &s[i + 1..];
        if after.starts_with('=') || s[..i].ends_with(['=', '!', '<', '>']) {
            continue;
        }
        let name_end = s[..i].trim_end().len();
        let name_start = s[..name_end]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(0, |p| p + 1);
        if !is_secret_key(&s[name_start..name_end]) {
            continue;
        }
        let start = i + 1 + (after.len() - after.trim_start().len());
        consider(start, value_len(&s[start..]));
        break;
    }
    best
}

/// Length of the (possibly quoted) value at the start of `s`.
fn value_len(s: &str) -> usize {
    match s.chars().next() {
        Some(q @ ('"' | '\'')) => s[1..].find(q).map_or(s.len(), |end| end + 2),
        _ => s
            .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '"' | '\'' | ')'))
            .unwrap_or(s.len()),
    }
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
    // The reader hands its buffer over a channel: a grandchild that inherited the pipe can keep
    // it open long after the child exited, and a join would wait for it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let timed_out = || io::Error::new(io::ErrorKind::TimedOut, "timed out");
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timed_out());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let stdout = rx.recv_timeout(remaining).map_err(|_| timed_out())?;
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
        assert!(backup_names(&env.paths.backups_dir()).is_empty(), "deleted after the disconnect");
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

    fn backup_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn backups_keep_the_newest_three() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..13 {
            backup(tmp.path(), b"{}", 1_000 + i).unwrap();
        }
        let names = backup_names(tmp.path());
        assert_eq!(names.len(), KEEP_BACKUPS);
        assert_eq!(KEEP_BACKUPS, 3);
        assert_eq!(names[0], "settings-000000000001010.json");
    }

    #[test]
    fn backups_older_than_thirty_days_are_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let now = 100 * cuw_core::time::DAY_MS;
        backup(tmp.path(), b"{}", now - 31 * cuw_core::time::DAY_MS).unwrap();
        backup(tmp.path(), b"{}", now - 29 * cuw_core::time::DAY_MS).unwrap();
        backup(tmp.path(), b"{}", now).unwrap();
        let names = backup_names(tmp.path());
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(names[1], format!("settings-{now:015}.json"));
    }

    #[test]
    fn backups_never_keep_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = br#"{
  "env": {"FOO_TOKEN": "PLACEHOLDER-A", "PLAIN": "PLACEHOLDER-B", "N": 5},
  "apiKeyHelper": "PLACEHOLDER-C",
  "awsAuthRefresh": "PLACEHOLDER-D",
  "nested": {"Password": "PLACEHOLDER-E", "list": [{"client_secret": "PLACEHOLDER-F"}]},
  "statusLine": {"type": "command", "command": "my-line"},
  "theme": "dark"
}"#;
        let path = backup(tmp.path(), settings, 1).unwrap().expect("written");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("PLACEHOLDER"), "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["env"]["FOO_TOKEN"], REDACTED);
        assert_eq!(v["env"]["PLAIN"], REDACTED, "every env value");
        assert_eq!(v["env"]["N"], REDACTED);
        assert_eq!(v["apiKeyHelper"], REDACTED);
        assert_eq!(v["nested"]["list"][0]["client_secret"], REDACTED);
        assert_eq!(v["statusLine"]["command"], "my-line", "what a restore needs is kept");
        assert_eq!(v["theme"], "dark");
        // Not JSON: nothing is written rather than the raw bytes.
        assert_eq!(backup(tmp.path(), b"{ // jsonc", 2).unwrap(), None);
        assert_eq!(backup_names(tmp.path()).len(), 1);
    }

    #[test]
    fn a_successful_disconnect_deletes_the_backups() {
        let (_t, env) = setup(Some(ORIGINAL));
        connect(&env, 1_000).unwrap();
        assert_eq!(backup_names(&env.paths.backups_dir()).len(), 1);
        disconnect(&env.paths, 2_000).unwrap();
        assert!(backup_names(&env.paths.backups_dir()).is_empty());
    }

    #[cfg(windows)]
    #[test]
    #[allow(clippy::permissions_set_readonly_false)] // Windows only: clears the read-only attribute
    fn a_failed_connect_leaves_no_wrap_json() {
        let (_t, env) = setup(Some(ORIGINAL));
        let settings = env.paths.claude_settings();
        let mut perms = fs::metadata(&settings).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&settings, perms.clone()).unwrap();
        let result = connect(&env, 1_000);
        perms.set_readonly(false);
        fs::set_permissions(&settings, perms).unwrap();
        assert!(result.is_err());
        assert!(!env.paths.wrap_file().exists(), "no record of a connect that did not happen");
        assert_eq!(fs::read_to_string(&settings).unwrap(), ORIGINAL);
    }

    #[test]
    fn disconnect_clears_a_stale_wrap_json() {
        let (_t, env) = setup(Some(ORIGINAL));
        connect(&env, 1_000).unwrap();
        // Someone else put their own status line back: nothing of ours to undo, but the stale
        // record (which makes the watchdog warn) goes.
        fs::write(env.paths.claude_settings(), ORIGINAL).unwrap();
        assert!(matches!(disconnect(&env.paths, 2_000).unwrap(), ConnectionStatus::Foreign { .. }));
        assert!(!env.paths.wrap_file().exists());
    }

    #[test]
    fn installed_shim_is_refreshed_while_connected() {
        let (tmp, env) = setup(Some(ORIGINAL));
        let source = env.shim_source.clone().unwrap();
        let installed = installed_shim(&env.paths);
        // Not connected: nothing is installed.
        assert!(!refresh_installed_shim(&env.paths, Some(&source)).unwrap());
        assert!(!installed.exists());
        connect(&env, 1_000).unwrap();
        assert!(!refresh_installed_shim(&env.paths, Some(&source)).unwrap(), "identical");
        // An app update ships a new shim, or the installed one was tampered with.
        fs::write(&installed, b"fake shim, tampered").unwrap();
        assert!(refresh_installed_shim(&env.paths, Some(&source)).unwrap());
        assert_eq!(fs::read(&installed).unwrap(), b"fake shim");
        assert!(!refresh_installed_shim(&env.paths, None).unwrap(), "no sidecar: nothing to do");
        drop(tmp);
    }

    #[test]
    fn previews_mask_secret_looking_values() {
        assert_eq!(mask_secrets("my-line --flag"), "my-line --flag");
        assert_eq!(
            mask_secrets("API_KEY=PLACEHOLDER npx line"),
            format!("API_KEY={MASK} npx line")
        );
        assert_eq!(
            mask_secrets(r#"$env:MY_TOKEN="PLACEHOLDER"; line"#),
            format!("$env:MY_TOKEN={MASK}; line")
        );
        assert_eq!(
            mask_secrets("my-line --auth 'Bearer PLACEHOLDER' x"),
            format!("my-line --auth 'Bearer {MASK}' x")
        );
        assert_eq!(mask_secrets("run sk-ant-PLACEHOLDER end"), format!("run sk-ant-{MASK} end"));
        assert_eq!(mask_secrets("VERSION=2 line"), "VERSION=2 line");
        let (_t, env) = setup(Some(
            r#"{"statusLine":{"type":"command","command":"SECRET=PLACEHOLDER my-line"}}"#,
        ));
        let p = preview(&env, 1).unwrap();
        assert_eq!(p.before, Some(format!("SECRET={MASK} my-line")));
        assert!(!p.after.contains("PLACEHOLDER"), "{}", p.after);
    }

    #[cfg(windows)]
    #[test]
    fn a_grandchild_holding_stdout_does_not_block_past_the_timeout() {
        let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let mut cmd = Command::new(comspec);
        // The child exits at once; the background ping keeps the stdout pipe open for ~6 s.
        cmd.args(["/d", "/c", "start /b ping -n 7 127.0.0.1"]);
        let start = Instant::now();
        let _ = run_with_timeout(cmd, None, Duration::from_millis(1_500));
        assert!(start.elapsed() < Duration::from_secs(4), "{:?}", start.elapsed());
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
            shim_path: "C:/x/cuw-capture.exe".into(),
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
