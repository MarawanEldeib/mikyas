//! `mikyas-capture`: the statusline tee shim placed in front of the user's Claude Code statusline.
//!
//! Claude Code writes one JSON object to the statusline command's stdin and shows whatever the
//! command prints. The shim must be invisible and fail-open: the user's statusline has to look
//! exactly as it would without it, so every error is swallowed (at most one short line goes to
//! `<capture_dir>/_errors.log`) and nothing is ever written to stderr.
//!
//! Modes (first argument):
//! - `--tee` (also: no argument): copy stdin to stdout byte for byte, then capture. An unknown or
//!   misplaced argument also falls back to `--tee`, and `unknown_arg` (never the argument's text)
//!   is logged so a mistyped statusline command can be found.
//! - `--default`: capture, then print one compact coloured summary line.
//! - `-- <program> [args...]`: run the user's statusline program with the same stdin (used where
//!   the shell cannot pipe bytes unchanged) and exit with its exit code.
//! - `--diag`: append shell/process diagnostics to `_diag.log`: environment variable names, JSON
//!   key names, the parent/grandparent executable names and the argument COUNT. Never values,
//!   never the command line or any path.
//! - `--version`: print the version.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use mikyas_core::capture::{self, CaptureError, CaptureRecord, MAX_STDIN_BYTES};
use mikyas_core::engine::types::{WindowKind, main_kinds};
use mikyas_core::level::{UsageLevel, display_pct};
use mikyas_core::paths::Paths;
use mikyas_core::time::{self, Ms};

/// If stdin has not closed this long after start, stop reading and go on with what arrived.
const STDIN_WATCHDOG: Duration = Duration::from_secs(2);
const READ_CHUNK: usize = 64 * 1024;
const ERRORS_LOG: &str = "_errors.log";
const ERRORS_LOG_MAX: u64 = 64 * 1024;
const DIAG_LOG: &str = "_diag.log";
const DIAG_LOG_MAX: u64 = 256 * 1024;

const RESET: &str = "\x1b[0m";
const PINK: &str = "\x1b[38;5;213m";
const DIM: &str = "\x1b[38;5;245m";
const GREEN: &str = "\x1b[32m";
const ORANGE: &str = "\x1b[38;5;208m";
const RED: &str = "\x1b[31m";
const SEPARATOR: &str = " · ";
/// Longest model name shown on the `--default` line.
const MAX_MODEL_NAME_CHARS: usize = 48;

#[derive(Debug, PartialEq)]
enum Mode {
    Tee,
    Default,
    Diag,
    Version,
    Argv(Vec<OsString>),
}

fn main() {
    // A panic message on stderr would show up in (or break) the user's statusline.
    std::panic::set_hook(Box::new(|_| {}));
    let code = std::panic::catch_unwind(run).unwrap_or(0);
    std::process::exit(code);
}

fn run() -> i32 {
    let (mode, unknown_arg) = parse_args(std::env::args_os().skip(1));
    let name = mode.name();
    let code = run_mode(mode);
    // After the mode ran, so the user's statusline is never delayed by it. Kind only.
    if unknown_arg {
        log_failure(&Paths::detect_capture_dir(), name, "unknown_arg");
    }
    code
}

fn run_mode(mode: Mode) -> i32 {
    match mode {
        Mode::Version => {
            write_stdout(format!("mikyas-capture {}\n", env!("CARGO_PKG_VERSION")).as_bytes());
            0
        }
        Mode::Tee => {
            tee();
            0
        }
        Mode::Default => {
            default_line();
            0
        }
        Mode::Diag => {
            diag();
            0
        }
        Mode::Argv(command) => argv(&command),
    }
}

impl Mode {
    /// The name used in `_errors.log` lines.
    fn name(&self) -> &'static str {
        match self {
            Mode::Tee => "tee",
            Mode::Default => "default",
            Mode::Diag => "diag",
            Mode::Version => "version",
            Mode::Argv(_) => "argv",
        }
    }
}

/// Hand-rolled on purpose: no dependency, and anything unexpected falls back to `--tee`. The flag
/// is true when an argument was not understood: an unknown first argument, or anything after a
/// mode that takes none. `--diag` accepts (and only counts) extra arguments.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> (Mode, bool) {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return (Mode::Tee, false);
    };
    let mode = match first.to_str() {
        Some("--tee") => Mode::Tee,
        Some("--default") => Mode::Default,
        Some("--diag") => return (Mode::Diag, false),
        Some("--version") => Mode::Version,
        Some("--") => return (Mode::Argv(args.collect()), false),
        _ => return (Mode::Tee, true),
    };
    (mode, args.next().is_some())
}

// ---------------------------------------------------------------------------------------------
// stdin
// ---------------------------------------------------------------------------------------------

/// Reads stdin on a detached thread and hands chunks over a channel, so the main thread can give
/// up at the watchdog deadline. A reader blocked on a stdin that never closes simply dies with
/// the process.
struct StdinPump {
    rx: Receiver<Vec<u8>>,
    deadline: Instant,
    timed_out: bool,
}

impl StdinPump {
    fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        // If the thread cannot be created, `tx` is dropped and the pump reports an immediate EOF.
        let _ = thread::Builder::new().spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut buf = vec![0; READ_CHUNK];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
        Self { rx, deadline: Instant::now() + STDIN_WATCHDOG, timed_out: false }
    }

    /// The next chunk, or `None` at EOF or once the watchdog has fired.
    fn next_chunk(&mut self) -> Option<Vec<u8>> {
        if self.timed_out {
            return None;
        }
        let wait = self.deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(wait) {
            Ok(chunk) => Some(chunk),
            Err(RecvTimeoutError::Timeout) => {
                self.timed_out = true;
                None
            }
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Buffers stdin until EOF, the watchdog, or more than [`MAX_STDIN_BYTES`] have arrived
    /// (the capture then fails with `TooLarge`; the rest can still be [`Self::drain`]ed).
    fn read_head(&mut self) -> Vec<u8> {
        let mut head = Vec::new();
        while head.len() <= MAX_STDIN_BYTES {
            match self.next_chunk() {
                Some(chunk) => head.extend_from_slice(&chunk),
                None => break,
            }
        }
        head
    }

    /// Hands every remaining chunk to `sink` until EOF or the watchdog.
    fn drain(&mut self, mut sink: impl FnMut(&[u8])) {
        while let Some(chunk) = self.next_chunk() {
            sink(&chunk);
        }
    }
}

/// Keeps at most `MAX_STDIN_BYTES + 1` bytes: enough to detect "too large" without holding an
/// unbounded amount of memory.
fn retain(head: &mut Vec<u8>, chunk: &[u8]) {
    let room = (MAX_STDIN_BYTES + 1).saturating_sub(head.len());
    head.extend_from_slice(&chunk[..chunk.len().min(room)]);
}

/// Reads all of stdin (bounded by the watchdog) for modes that do not forward it.
fn read_all_stdin() -> (Vec<u8>, bool) {
    let mut pump = StdinPump::start();
    let head = pump.read_head();
    pump.drain(|_| {});
    (head, pump.timed_out)
}

// ---------------------------------------------------------------------------------------------
// modes
// ---------------------------------------------------------------------------------------------

fn tee() {
    let mut pump = StdinPump::start();
    let mut head = Vec::new();
    {
        let mut out = io::stdout().lock();
        let mut out_ok = true;
        // Forward each chunk as it arrives, before any parsing: whatever happens afterwards, the
        // user's statusline has already received exactly what Claude Code sent.
        pump.drain(|chunk| {
            if out_ok {
                out_ok = out.write_all(chunk).and_then(|()| out.flush()).is_ok();
            }
            retain(&mut head, chunk);
        });
    }
    // In the pipe form the user's statusline reads until EOF; let it run while we capture.
    close_stdout();
    capture_input("tee", &head, pump.timed_out);
}

fn default_line() {
    let (head, timed_out) = read_all_stdin();
    capture_and_print_line("default", &head, timed_out);
}

fn capture_and_print_line(mode: &str, bytes: &[u8], timed_out: bool) {
    let rec = capture_record(mode, bytes, timed_out);
    write_stdout(render_line(rec.as_ref(), time::now_ms()).as_bytes());
}

fn argv(command: &[OsString]) -> i32 {
    let mut pump = StdinPump::start();
    let head: Arc<[u8]> = pump.read_head().into();
    let timed_out = pump.timed_out;

    let Some((program, args)) = command.split_first() else {
        capture_and_print_line("argv", &head, timed_out);
        log_failure(&Paths::detect_capture_dir(), "argv", "no_program");
        return 0;
    };

    // Joining the job before spawning makes the child a member from its first instruction, so
    // nothing it starts can slip out before we could assign it.
    let job = job::Job::kill_on_close();
    let shim_in_job = job.as_ref().is_some_and(job::Job::assign_current);

    let spawned = Command::new(resolve_program(program))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            capture_and_print_line("argv", &head, timed_out);
            // The user's statusline was replaced by the fallback line: record why (kind only;
            // the program name could be a private path).
            log_failure(&Paths::detect_capture_dir(), "argv", &format!("spawn_{:?}", e.kind()));
            if let Some(job) = &job {
                release_job(job);
            }
            return 0;
        }
    };
    if !shim_in_job {
        if let Some(job) = &job {
            job.assign(&child);
        }
    }

    // Written on a separate, never-joined thread: a child that does not read its stdin must not
    // block us once the pipe buffer is full. Its exit turns the write into a BrokenPipe error.
    if let Some(mut child_stdin) = child.stdin.take() {
        let head = Arc::clone(&head);
        let _ = thread::Builder::new().spawn(move || {
            if child_stdin.write_all(&head).is_ok() {
                // Only reached when stdin exceeded the capture cap: stream the rest through.
                pump.drain(|chunk| {
                    let _ = child_stdin.write_all(chunk);
                });
            }
            // Dropping `child_stdin` closes the pipe: the child sees EOF.
        });
    }

    // The shim and the user's program share a kill-on-close job: a panic unwinding out of here
    // would close it and kill the program. Contain a bug in our own capture instead.
    let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture_input("argv", &head, timed_out);
    }));
    if captured.is_err() {
        log_failure(&Paths::detect_capture_dir(), "argv", "panic");
    }

    let code = match child.wait() {
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => 1,
    };
    // The child finished normally: let anything it deliberately left running in the background
    // outlive us. Only an abnormal end of the shim (Claude Code cancelling it) kills the tree.
    if let Some(job) = &job {
        release_job(job);
    }
    code
}

fn release_job(job: &job::Job) {
    if !job.release() {
        log_failure(&Paths::detect_capture_dir(), "argv", "job_release");
    }
}

fn diag() {
    let (head, timed_out) = read_all_stdin();
    let dir = Paths::detect_capture_dir();
    let argc = std::env::args_os().len().saturating_sub(1);
    let block = diag_block(&head, timed_out, argc);
    let _ = append_capped(&dir.join(DIAG_LOG), block.as_bytes(), DIAG_LOG_MAX);
    capture_input("diag", &head, timed_out);
    write_stdout(b"mikyas diag ok\n");
}

// ---------------------------------------------------------------------------------------------
// capture + error log
// ---------------------------------------------------------------------------------------------

/// Parses and saves the capture through mikyas-core's one-call helper. Failures are logged (kind
/// only, never input content) and otherwise ignored.
fn capture_input(mode: &str, bytes: &[u8], timed_out: bool) {
    test_hook();
    let dir = Paths::detect_capture_dir();
    let result = capture::capture_from_bytes(bytes, &dir, time::now_ms());
    log_capture_result(&dir, mode, result.err().as_ref(), timed_out);
}

/// Like [`capture_input`], but also returns the record, for rendering the `--default` line (the
/// only place that needs it, so the only place that pays for the copy handed to the writer).
fn capture_record(mode: &str, bytes: &[u8], timed_out: bool) -> Option<CaptureRecord> {
    test_hook();
    let dir = Paths::detect_capture_dir();
    let result = capture::record_from_bytes(bytes, time::now_ms()).and_then(|rec| {
        capture::write_capture(&dir, rec.clone())?;
        Ok(rec)
    });
    let (rec, err) = match result {
        Ok(rec) => (Some(rec), None),
        Err(e) => (None, Some(e)),
    };
    log_capture_result(&dir, mode, err.as_ref(), timed_out);
    rec
}

fn log_capture_result(dir: &Path, mode: &str, err: Option<&CaptureError>, timed_out: bool) {
    let kind = err.map(error_kind);
    let note = match (kind, timed_out) {
        (Some(kind), true) => Some(format!("{kind} stdin_timeout")),
        (Some(kind), false) => Some(kind),
        (None, true) => Some("stdin_timeout".to_owned()),
        (None, false) => None,
    };
    if let Some(note) = note {
        log_failure(dir, mode, &note);
    }
}

/// Debug builds only (release binaries never read it): `MIKYAS_TEST_HOOK` lets the integration
/// tests make the capture panic (`panic`) or slow (`capture_delay_ms=N`) on demand.
#[cfg(debug_assertions)]
fn test_hook() {
    let Some(hook) = std::env::var_os("MIKYAS_TEST_HOOK") else {
        return;
    };
    let hook = hook.to_string_lossy();
    if hook == "panic" {
        panic!("MIKYAS_TEST_HOOK=panic");
    }
    if let Some(ms) = hook.strip_prefix("capture_delay_ms=").and_then(|ms| ms.parse().ok()) {
        thread::sleep(Duration::from_millis(ms));
    }
}

#[cfg(not(debug_assertions))]
fn test_hook() {}

/// Appends one `<timestamp> <mode> <note>` line to `<capture_dir>/_errors.log`. `note` is an
/// error kind, never input content.
fn log_failure(capture_dir: &Path, mode: &str, note: &str) {
    let line = format!("{} {mode} {note}\n", iso_now());
    let _ = append_capped(&capture_dir.join(ERRORS_LOG), line.as_bytes(), ERRORS_LOG_MAX);
}

fn error_kind(e: &CaptureError) -> String {
    match e {
        CaptureError::NotJson => "not_json".to_owned(),
        CaptureError::NoSession => "no_session".to_owned(),
        CaptureError::TooLarge => "too_large".to_owned(),
        CaptureError::Io(io) => format!("io_{:?}", io.kind()),
    }
}

/// Appends `text`, first cutting the file down to its newest half if the result would reach
/// `max` bytes. Best effort: concurrent shims may interleave, which only affects this log.
fn append_capped(path: &Path, text: &[u8], max: u64) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let len = fs::metadata(path).map_or(0, |m| m.len());
    if len + text.len() as u64 >= max {
        let mut kept = newest_lines(path, max / 2);
        kept.extend_from_slice(text);
        return fs::write(path, kept);
    }
    OpenOptions::new().create(true).append(true).open(path)?.write_all(text)
}

/// The last `keep` bytes of the file, starting at a line boundary.
fn newest_lines(path: &Path, keep: u64) -> Vec<u8> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(keep);
    let mut buf = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.take(keep).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    if start > 0 {
        // Drop the partial first line.
        let cut = buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1);
        buf.drain(..cut);
    }
    buf
}

fn iso_now() -> String {
    let now = time::now_ms();
    chrono::DateTime::from_timestamp_millis(now)
        .map_or_else(|| now.to_string(), |t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Writes to stdout, ignoring every error (a closed pipe must never turn into a panic).
fn write_stdout(bytes: &[u8]) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(bytes).and_then(|()| out.flush());
}

/// Closes stdout (already flushed) so a reader on the other end of the pipe sees EOF now rather
/// than when the process exits. The standard handle is detached first, so later stdout writes
/// become silent no-ops (Rust treats a missing standard handle as a sink) instead of reaching
/// whatever a reused handle value might point to.
#[cfg(windows)]
fn close_stdout() {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // SAFETY: plain Win32 calls on this process's own standard handles. The handle is closed only
    // after it has been removed from the standard-handle table, and only once.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        // One duplex handle serving as stdin too (the watchdog may have left a read pending on
        // it): leave it alone.
        if out.is_null() || out == INVALID_HANDLE_VALUE || out == GetStdHandle(STD_INPUT_HANDLE) {
            return;
        }
        if SetStdHandle(STD_OUTPUT_HANDLE, std::ptr::null_mut()) == 0 {
            return;
        }
        // A parent may pass one handle value for both streams; do not leave stderr dangling.
        if GetStdHandle(STD_ERROR_HANDLE) == out {
            SetStdHandle(STD_ERROR_HANDLE, std::ptr::null_mut());
        }
        CloseHandle(out);
    }
}

/// Without `dup2` (no libc dependency) closing fd 1 would let the next `open` reuse it, so stdout
/// simply stays open until exit.
#[cfg(not(windows))]
fn close_stdout() {}

// ---------------------------------------------------------------------------------------------
// --default line
// ---------------------------------------------------------------------------------------------

/// `<model> · ctx 34% · 5h 22% 3h12m · 7d 61% 2d4h`, omitting whatever is unknown. The windows
/// come from the data: the shortest and the longest (`main_kinds`) always, any other only while it
/// is at the orange band or above (e.g. a model-specific weekly limit about to block), ordered by
/// length (unknown lengths last).
fn render_line(rec: Option<&CaptureRecord>, now_ms: Ms) -> String {
    let mut parts = Vec::new();
    if let Some(rec) = rec {
        let name = rec
            .model
            .as_ref()
            .and_then(|m| m.display_name.as_deref().or(m.id.as_deref()))
            .map(|n| printable(n).chars().take(MAX_MODEL_NAME_CHARS).collect::<String>())
            .filter(|n| !n.is_empty());
        if let Some(name) = name {
            parts.push(format!("{PINK}{name}{RESET}"));
        }
        if let Some(pct) = rec.context.as_ref().and_then(|c| c.used_percentage) {
            parts.push(format!("ctx {}", colored_pct(pct)));
        }
        let windows: Vec<(WindowKind, &capture::RateLimit)> =
            rec.rate_limits.iter().map(|(k, w)| (WindowKind::from_key(k), w)).collect();
        let main = main_kinds(windows.iter().map(|(k, _)| k));
        let mut shown: Vec<&(WindowKind, &capture::RateLimit)> = windows
            .iter()
            .filter(|(k, w)| main.contains(k) || UsageLevel::for_pct(w.used_percentage) != UsageLevel::Ok)
            .collect();
        shown.sort_by(|(a, _), (b, _)| {
            let len = |k: &WindowKind| k.duration_ms().map_or((1, 0), |d| (0, d));
            len(a).cmp(&len(b)).then_with(|| a.key().cmp(b.key()))
        });
        for (kind, window) in shown {
            let reset = window.resets_at.map(|r| format!(" {DIM}{}{RESET}", countdown(r, now_ms))).unwrap_or_default();
            parts.push(format!("{} {}{reset}", kind.short_label(), colored_pct(window.used_percentage)));
        }
    }
    if parts.is_empty() {
        parts.push("Claude".to_owned());
    }
    let mut line = parts.join(SEPARATOR);
    line.push('\n');
    line
}

/// Strips control characters (terminal escape sequences), bidi overrides/embeddings/isolates and
/// zero-width characters (reordered or hidden text) from text shown in a terminal.
fn printable(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{FEFF}'
                )
        })
        .collect()
}

/// The % in the shared usage bands ([`mikyas_core::level`]), coloured by the number shown.
fn colored_pct(pct: f32) -> String {
    let pct = display_pct(pct);
    let color = match UsageLevel::for_shown(pct) {
        UsageLevel::Ok => GREEN,
        UsageLevel::Warn => ORANGE,
        UsageLevel::Crit => RED,
    };
    format!("{color}{pct}%{RESET}")
}

/// Same rules as the PowerShell statusline: `2d4h`, `3h12m`, `5m`; a past reset is `now`.
fn countdown(resets_at_s: i64, now_ms: Ms) -> String {
    let secs = resets_at_s.saturating_sub(now_ms.div_euclid(1000));
    if secs <= 0 {
        return "now".to_owned();
    }
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if days >= 1 {
        format!("{days}d{hours}h")
    } else if hours >= 1 {
        format!("{hours}h{minutes}m")
    } else {
        format!("{minutes}m")
    }
}

// ---------------------------------------------------------------------------------------------
// argv mode: program resolution + job object
// ---------------------------------------------------------------------------------------------

/// `Command` only appends `.exe` to a bare name, so `pwsh` works but `npx` (really `npx.cmd`) does
/// not. Resolve extensionless names like cmd.exe does: each PATH dir, each PATHEXT extension.
#[cfg(windows)]
fn resolve_program(program: &OsStr) -> PathBuf {
    let path = Path::new(program);
    let exts = path_exts();
    let has_exec_ext =
        path.extension().is_some_and(|e| exts.iter().any(|x| x.get(1..).is_some_and(|x| e.eq_ignore_ascii_case(x))));
    if program.is_empty() || has_exec_ext {
        return path.to_path_buf();
    }
    let has_dir = path.has_root() || path.parent().is_some_and(|p| !p.as_os_str().is_empty());
    let found = if has_dir {
        with_first_ext(path, &exts)
    } else {
        std::env::var_os("PATH").and_then(|dirs| {
            std::env::split_paths(&dirs)
                .filter(|d| !d.as_os_str().is_empty())
                .find_map(|d| with_first_ext(&d.join(path), &exts))
        })
    };
    found.unwrap_or_else(|| path.to_path_buf())
}

#[cfg(not(windows))]
fn resolve_program(program: &OsStr) -> PathBuf {
    PathBuf::from(program)
}

/// `base` + the first PATHEXT extension that names an existing file.
#[cfg(windows)]
fn with_first_ext(base: &Path, exts: &[String]) -> Option<PathBuf> {
    exts.iter().find_map(|ext| {
        let mut candidate = base.as_os_str().to_owned();
        candidate.push(ext);
        let candidate = PathBuf::from(candidate);
        candidate.is_file().then_some(candidate)
    })
}

#[cfg(windows)]
fn path_exts() -> Vec<String> {
    let raw = std::env::var("PATHEXT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned());
    raw.split(';').map(str::trim).filter(|e| e.len() > 1 && e.starts_with('.')).map(str::to_owned).collect()
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// A job object whose member processes are killed when its last handle closes. The shim holds
    /// the only (non-inheritable) handle, so the shim dying — for any reason — kills the child.
    pub struct Job(HANDLE);

    impl Job {
        pub fn kill_on_close() -> Option<Self> {
            // SAFETY: null security attributes and a null name are valid; the returned handle is
            // owned by `Job` and closed exactly once in `Drop`.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let job = Job(handle);
            job.set_limit_flags(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE).then_some(job)
        }

        fn set_limit_flags(&self, flags: u32) -> bool {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = flags;
            // SAFETY: `info` is a valid, fully initialised struct of the size we pass, and
            // outlives the call.
            unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) != 0
            }
        }

        /// Puts the shim itself in the job; children it spawns afterwards inherit membership.
        pub fn assign_current(&self) -> bool {
            // SAFETY: both handles are valid for the duration of the call (the pseudo-handle from
            // GetCurrentProcess needs no closing).
            unsafe { AssignProcessToJobObject(self.0, GetCurrentProcess()) != 0 }
        }

        pub fn assign(&self, child: &Child) -> bool {
            // SAFETY: `child` owns a valid process handle for as long as it is borrowed.
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
        }

        /// Clears the kill-on-close limit so closing the handle no longer kills anything. False
        /// if Windows refused (closing the handle would then still kill the tree).
        pub fn release(&self) -> bool {
            self.set_limit_flags(0)
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Unwinding from a bug in the shim is not Claude Code cancelling it: do not take the
            // user's program (or the shim itself, also a member) down with it.
            if std::thread::panicking() {
                self.release();
            }
            // SAFETY: the handle came from CreateJobObjectW and is closed only here.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    use std::process::Child;

    /// Job objects are Windows-only; elsewhere the child is simply not tied to the shim.
    pub struct Job;

    impl Job {
        pub fn kill_on_close() -> Option<Self> {
            None
        }
        pub fn assign_current(&self) -> bool {
            false
        }
        pub fn assign(&self, _child: &Child) -> bool {
            false
        }
        pub fn release(&self) -> bool {
            true
        }
    }
}

// ---------------------------------------------------------------------------------------------
// --diag
// ---------------------------------------------------------------------------------------------

/// Environment variables whose presence tells which shell ran us. Only names are ever logged.
const DIAG_ENV_NAMES: &[&str] = &["SHELL", "COMSPEC", "MSYSTEM", "TERM_PROGRAM", "PSMODULEPATH"];
const DIAG_MAX_KEYS: usize = 64;
const DIAG_MAX_KEY_LEN: usize = 64;

/// `argc` is the number of arguments after the executable; their text is never logged (a path
/// argument would reveal the user name).
fn diag_block(stdin: &[u8], timed_out: bool, argc: usize) -> String {
    let json: Option<serde_json::Value> =
        serde_json::from_slice(stdin.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(stdin)).ok();
    let (parent, grandparent) = ancestry();
    let mut s = String::new();
    let _ = writeln!(s, "=== mikyas-capture {} --diag at {} ===", env!("CARGO_PKG_VERSION"), iso_now());
    let _ = writeln!(s, "args: mode=--diag argc={argc}");
    let _ = writeln!(s, "parent: {parent}");
    let _ = writeln!(s, "grandparent: {grandparent}");
    let _ = writeln!(s, "env_names: {}", diag_env_names().join(", "));
    let _ = writeln!(s, "stdin_bytes: {}{}", stdin.len(), if timed_out { " (watchdog fired)" } else { "" });
    let _ = writeln!(s, "json_keys: {}", key_names(json.as_ref()));
    let _ = writeln!(s, "rate_limits_keys: {}", key_names(json.as_ref().and_then(|j| j.get("rate_limits"))));
    s.push('\n');
    s
}

fn key_names(value: Option<&serde_json::Value>) -> String {
    let Some(obj) = value.and_then(serde_json::Value::as_object) else {
        return "(none)".to_owned();
    };
    let names: Vec<String> =
        obj.keys().take(DIAG_MAX_KEYS).map(|k| printable(k).chars().take(DIAG_MAX_KEY_LEN).collect()).collect();
    names.join(", ")
}

fn diag_env_names() -> Vec<String> {
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _value)| name.into_string().ok())
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            DIAG_ENV_NAMES.contains(&upper.as_str()) || upper.starts_with("CLAUDE")
        })
        .collect();
    names.sort();
    names
}

/// `pid=… exe=…` of the parent and grandparent process.
#[cfg(windows)]
fn ancestry() -> (String, String) {
    let table = process_table();
    let describe = |pid: u32| match table.get(&pid) {
        Some((_, exe)) => format!("pid={pid} exe={exe}"),
        None => format!("pid={pid} exe=?"),
    };
    let Some(&(parent, _)) = table.get(&std::process::id()) else {
        return ("unknown".to_owned(), "unknown".to_owned());
    };
    let grandparent = table.get(&parent).map_or_else(|| "unknown".to_owned(), |&(gp, _)| describe(gp));
    (describe(parent), grandparent)
}

/// pid → (parent pid, exe name) for every running process.
#[cfg(windows)]
fn process_table() -> std::collections::HashMap<u32, (u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };

    let mut table = std::collections::HashMap::new();
    // SAFETY: plain Win32 calls; `entry` has `dwSize` set as the API requires, and the snapshot
    // handle is closed before returning.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() || snapshot == INVALID_HANDLE_VALUE {
            return table;
        }
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
        while ok {
            let name = &entry.szExeFile;
            let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            table.insert(entry.th32ProcessID, (entry.th32ParentProcessID, String::from_utf16_lossy(&name[..len])));
            ok = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    table
}

#[cfg(unix)]
fn ancestry() -> (String, String) {
    fn comm(pid: u32) -> String {
        fs::read_to_string(format!("/proc/{pid}/comm")).map_or_else(|_| "?".to_owned(), |s| s.trim().to_owned())
    }
    fn parent_of(pid: u32) -> Option<u32> {
        // /proc/<pid>/stat: "pid (comm) state ppid ..."; comm may contain spaces, so split after ')'.
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
    }
    let parent = std::os::unix::process::parent_id();
    let grandparent = parent_of(parent).map_or_else(|| "unknown".to_owned(), |gp| format!("pid={gp} exe={}", comm(gp)));
    (format!("pid={parent} exe={}", comm(parent)), grandparent)
}

#[cfg(not(any(windows, unix)))]
fn ancestry() -> (String, String) {
    ("unknown".to_owned(), "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mikyas_core::capture::{CtxInfo, ModelInfo, RateLimit};

    use super::*;

    const NOW_S: i64 = 1_790_200_000;

    fn record() -> CaptureRecord {
        let mut rate_limits = BTreeMap::new();
        rate_limits.insert(
            "five_hour".to_owned(),
            RateLimit { used_percentage: 22.4, resets_at: Some(NOW_S + 3 * 3600 + 12 * 60 + 5) },
        );
        rate_limits.insert(
            "seven_day".to_owned(),
            RateLimit { used_percentage: 61.0, resets_at: Some(NOW_S + 2 * 86_400 + 4 * 3600 + 59 * 60) },
        );
        rate_limits
            .insert("seven_day_opus".to_owned(), RateLimit { used_percentage: 99.0, resets_at: Some(NOW_S + 60) });
        CaptureRecord {
            v: capture::CAPTURE_VERSION,
            session_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            written_at_ms: NOW_S * 1000,
            changed_at_ms: NOW_S * 1000,
            fingerprint: 0,
            model: Some(ModelInfo {
                id: Some("claude-opus-5-5".to_owned()),
                display_name: Some("Opus 5.5".to_owned()),
            }),
            context: Some(CtxInfo {
                used_percentage: Some(34.0),
                context_window_size: Some(200_000),
                exceeds_200k: None,
            }),
            rate_limits,
            api_ms: None,
        }
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_modes_and_defaults_to_tee() {
        assert_eq!(parse_args(args(&[])), (Mode::Tee, false));
        assert_eq!(parse_args(args(&["--tee"])), (Mode::Tee, false));
        assert_eq!(parse_args(args(&["--tee", "extra"])), (Mode::Tee, true));
        assert_eq!(parse_args(args(&["--default"])), (Mode::Default, false));
        assert_eq!(parse_args(args(&["--default", "x"])), (Mode::Default, true));
        assert_eq!(parse_args(args(&["--diag"])), (Mode::Diag, false));
        assert_eq!(parse_args(args(&["--diag", "x"])), (Mode::Diag, false));
        assert_eq!(parse_args(args(&["--version"])), (Mode::Version, false));
        assert_eq!(parse_args(args(&["--bogus"])), (Mode::Tee, true));
        assert_eq!(parse_args(args(&["pwsh"])), (Mode::Tee, true));
        assert_eq!(parse_args(args(&["--"])), (Mode::Argv(vec![]), false));
        assert_eq!(
            parse_args(args(&["--", "pwsh", "-File", "a b.ps1"])),
            (Mode::Argv(args(&["pwsh", "-File", "a b.ps1"])), false)
        );
    }

    #[test]
    fn countdown_matches_powershell_statusline() {
        let now = NOW_S * 1000;
        assert_eq!(countdown(NOW_S + 2 * 86_400 + 4 * 3600 + 59 * 60, now), "2d4h");
        assert_eq!(countdown(NOW_S + 86_400, now), "1d0h");
        assert_eq!(countdown(NOW_S + 3 * 3600 + 12 * 60 + 59, now), "3h12m");
        assert_eq!(countdown(NOW_S + 3600, now), "1h0m");
        assert_eq!(countdown(NOW_S + 59 * 60 + 59, now), "59m");
        assert_eq!(countdown(NOW_S + 30, now), "0m");
        assert_eq!(countdown(NOW_S, now), "now");
        assert_eq!(countdown(NOW_S - 100, now), "now");
        assert_eq!(countdown(i64::MIN, now), "now");
        assert_eq!(countdown(i64::MAX, i64::MIN), format!("{}d{}h", i64::MAX / 86_400, i64::MAX % 86_400 / 3600));
    }

    #[test]
    fn percent_colours_follow_thresholds() {
        assert_eq!(colored_pct(0.0), format!("{GREEN}0%{RESET}"));
        assert_eq!(colored_pct(39.4), format!("{GREEN}39%{RESET}"));
        assert_eq!(colored_pct(39.5), format!("{ORANGE}40%{RESET}"));
        assert_eq!(colored_pct(69.0), format!("{ORANGE}69%{RESET}"));
        assert_eq!(colored_pct(69.5), format!("{RED}70%{RESET}"), "coloured by the number shown");
        assert_eq!(colored_pct(70.0), format!("{RED}70%{RESET}"));
        assert_eq!(colored_pct(250.0), format!("{RED}100%{RESET}"));
        assert_eq!(colored_pct(f32::NAN), format!("{GREEN}0%{RESET}"));
    }

    #[test]
    fn renders_full_line() {
        let line = render_line(Some(&record()), NOW_S * 1000);
        assert_eq!(
            line,
            format!(
                "{PINK}Opus 5.5{RESET} · ctx {GREEN}34%{RESET} · 5h {GREEN}22%{RESET} {DIM}3h12m{RESET} · \
                 7d {ORANGE}61%{RESET} {DIM}2d4h{RESET} · 7d Opus {RED}99%{RESET} {DIM}1m{RESET}\n"
            )
        );
    }

    #[test]
    fn windows_on_the_line_come_from_the_data() {
        let mut rec = record();
        rec.model = None;
        rec.context = None;
        rec.rate_limits.clear();
        let limit = |pct: f32, reset: Option<i64>| RateLimit { used_percentage: pct, resets_at: reset };
        // Renamed main keys, a longer cycle, a quiet extra limit and one without a reset time.
        rec.rate_limits.insert("4_hour".into(), limit(10.0, Some(NOW_S + 3600)));
        rec.rate_limits.insert("thirty_day".into(), limit(20.0, None));
        rec.rate_limits.insert("seven_day_newmodel".into(), limit(12.0, Some(NOW_S + 60)));
        rec.rate_limits.insert("mystery".into(), limit(80.0, None));
        assert_eq!(
            render_line(Some(&rec), NOW_S * 1000),
            format!("4h {GREEN}10%{RESET} {DIM}1h0m{RESET} · 30d {GREEN}20%{RESET} · mystery {RED}80%{RESET}\n")
        );
    }

    #[test]
    fn renders_gracefully_with_missing_fields() {
        let mut rec = record();
        rec.model = Some(ModelInfo { id: Some("claude-x".to_owned()), display_name: None });
        rec.context = None;
        rec.rate_limits.remove("five_hour");
        assert_eq!(
            render_line(Some(&rec), NOW_S * 1000),
            format!(
                "{PINK}claude-x{RESET} · 7d {ORANGE}61%{RESET} {DIM}2d4h{RESET} · 7d Opus {RED}99%{RESET} {DIM}1m{RESET}\n"
            )
        );

        rec.model = Some(ModelInfo { id: None, display_name: Some("\x1b]0;evil\x07Opus".to_owned()) });
        rec.rate_limits.clear();
        assert_eq!(render_line(Some(&rec), 0), format!("{PINK}]0;evilOpus{RESET}\n"));

        // Bidi overrides/isolates and zero-width characters could reorder or hide what follows.
        rec.model = Some(ModelInfo {
            id: None,
            display_name: Some("\u{202E}Opus\u{200B}\u{2066}x\u{2069}\u{FEFF}\u{200F}".to_owned()),
        });
        assert_eq!(render_line(Some(&rec), 0), format!("{PINK}Opusx{RESET}\n"));

        // Names are capped so a huge one cannot push the rest of the line off screen.
        rec.model = Some(ModelInfo { id: None, display_name: Some("é".repeat(500)) });
        assert_eq!(render_line(Some(&rec), 0), format!("{PINK}{}{RESET}\n", "é".repeat(MAX_MODEL_NAME_CHARS)));

        rec.model = None;
        assert_eq!(render_line(Some(&rec), 0), "Claude\n");
        assert_eq!(render_line(None, 0), "Claude\n");
    }

    #[test]
    fn retain_caps_buffer() {
        let mut head = vec![0; MAX_STDIN_BYTES - 1];
        retain(&mut head, &[1, 2, 3, 4]);
        assert_eq!(head.len(), MAX_STDIN_BYTES + 1);
        retain(&mut head, &[5]);
        assert_eq!(head.len(), MAX_STDIN_BYTES + 1);
    }

    #[test]
    fn append_capped_keeps_newest_lines_under_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub").join("log.txt");
        for i in 0..200 {
            append_capped(&path, format!("line {i:04}\n").as_bytes(), 256).unwrap();
            assert!(fs::metadata(&path).unwrap().len() < 256);
        }
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("line 0199\n"));
        assert!(text.lines().all(|l| l.starts_with("line ") && l.len() == 9), "{text}");
    }

    #[test]
    fn diag_block_lists_names_only() {
        let stdin = br#"{"session_id":"00000000-0000-4000-8000-000000000001","cwd":"C:\\work\\value-sentinel","rate_limits":{"five_hour":{"used_percentage":5}}}"#;
        let block = diag_block(stdin, false, 2);
        assert!(block.contains("args: mode=--diag argc=2\n"), "{block}");
        assert!(block.contains("json_keys: session_id, cwd, rate_limits\n"), "{block}");
        assert!(block.contains("rate_limits_keys: five_hour\n"), "{block}");
        assert!(block.contains(&format!("stdin_bytes: {}\n", stdin.len())), "{block}");
        assert!(!block.contains("value-sentinel"));
        assert!(!block.contains("00000000-0000-4000-8000-000000000001"));
        assert!(diag_block(b"garbage", true, 1).contains("json_keys: (none)"));
    }

    #[cfg(windows)]
    #[test]
    fn process_table_knows_this_process() {
        let (parent, grandparent) = ancestry();
        assert!(parent.starts_with("pid="), "{parent}");
        assert!(parent.to_ascii_lowercase().contains(".exe"), "{parent}");
        assert!(!grandparent.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn resolves_extensionless_names_through_path() {
        let comspec = resolve_program(OsStr::new("cmd"));
        assert!(comspec.is_absolute() && comspec.is_file(), "{}", comspec.display());
        assert!(comspec.to_string_lossy().to_ascii_lowercase().ends_with("cmd.exe"));
        assert_eq!(resolve_program(OsStr::new("x.exe")), PathBuf::from("x.exe"));
        assert_eq!(resolve_program(OsStr::new("mikyas-no-such-program")), PathBuf::from("mikyas-no-such-program"));
    }
}
