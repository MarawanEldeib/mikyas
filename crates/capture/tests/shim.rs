//! End-to-end tests of the `cuw-capture` binary. Every run points `CUW_DATA_DIR` at a temp dir.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;

const SHIM: &str = env!("CARGO_BIN_EXE_cuw-capture");
const CHILD: &str = env!("CARGO_BIN_EXE_cuw-test-child");
const FULL: &str = include_str!("../../core/tests/fixtures/statusline/full.json");
const SID: &str = "00000000-0000-4000-8000-000000000001";
/// Generous: the shared build machine can be slow. Hangs are what these tests guard against.
const TIMEOUT: Duration = Duration::from_secs(30);

struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<i32>,
    elapsed: Duration,
}

fn shim(data: &Path) -> Command {
    let mut cmd = Command::new(SHIM);
    cmd.env("CUW_DATA_DIR", data);
    cmd
}

fn read_in_background(mut r: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

/// Polls instead of `Child::wait`, which would first close our end of the child's stdin.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Runs `cmd` with `input` on stdin, then closes stdin. Kills it after [`TIMEOUT`].
fn run(cmd: &mut Command, input: &[u8]) -> Outcome {
    let start = Instant::now();
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let data = input.to_vec();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let stdout = read_in_background(child.stdout.take().expect("stdout"));
    let stderr = read_in_background(child.stderr.take().expect("stderr"));
    let status = wait_with_timeout(&mut child, TIMEOUT);
    let elapsed = start.elapsed();
    let _ = writer.join();
    let outcome = Outcome {
        stdout: stdout.join().expect("stdout reader"),
        stderr: stderr.join().expect("stderr reader"),
        code: status.and_then(|s| s.code()),
        elapsed,
    };
    assert!(status.is_some(), "timed out after {elapsed:?}");
    outcome
}

fn capture_dir(data: &Path) -> PathBuf {
    data.join("capture")
}

fn capture_file(data: &Path) -> PathBuf {
    capture_dir(data).join(format!("{SID}.json"))
}

fn errors_log(data: &Path) -> String {
    fs::read_to_string(capture_dir(data).join("_errors.log")).unwrap_or_default()
}

/// Valid statusline JSON with non-ASCII text, JSON-escaped ANSI sequences and a CRLF ending.
fn fancy_input() -> Vec<u8> {
    let mut input = FULL
        .replace(
            "Opus 5.5 (1M context)",
            r"Opus 5.5 — ünïcödé 日本語 ✓ \u001b[1mbold\u001b[0m",
        )
        .into_bytes();
    input.extend_from_slice(b"\r\n");
    input
}

/// Resets relative to the real clock, so the `--default` countdowns are predictable.
fn input_with_resets(five_hour_in_s: i64, seven_day_in_s: i64) -> Vec<u8> {
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut json: Value = serde_json::from_str(FULL).unwrap();
    json["rate_limits"]["five_hour"]["resets_at"] = (now_s + five_hour_in_s).into();
    json["rate_limits"]["seven_day"]["resets_at"] = (now_s + seven_day_in_s).into();
    serde_json::to_vec(&json).unwrap()
}

fn assert_whitelisted_capture(data: &Path) {
    let text = fs::read_to_string(capture_file(data)).expect("capture file written");
    let json: Value = serde_json::from_str(&text).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "api_ms",
            "cc_version",
            "changed_at_ms",
            "context",
            "fingerprint",
            "model",
            "rate_limits",
            "session_id",
            "transcript_path",
            "v",
            "written_at_ms",
        ]
    );
    for banned in [
        "sentinel",
        "cwd",
        "workspace",
        "total_cost_usd",
        "spend_limit",
        "output_style",
    ] {
        assert!(!text.contains(banned), "capture leaked {banned:?}: {text}");
    }
    assert_eq!(json["api_ms"], 123_456);
    assert_eq!(json["rate_limits"]["five_hour"]["used_percentage"], 22.4);
    let leftovers: Vec<String> = fs::read_dir(capture_dir(data))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn tee_forwards_json_byte_for_byte_and_captures_whitelist() {
    let tmp = tempfile::tempdir().unwrap();
    let input = fancy_input();
    let out = run(shim(tmp.path()).arg("--tee"), &input);
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, input);
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_whitelisted_capture(tmp.path());
    let json: Value = serde_json::from_slice(&fs::read(capture_file(tmp.path())).unwrap()).unwrap();
    assert_eq!(
        json["model"]["display_name"],
        "Opus 5.5 — ünïcödé 日本語 ✓ \u{1b}[1mbold\u{1b}[0m"
    );
    assert_eq!(errors_log(tmp.path()), "");
}

#[test]
fn tee_forwards_arbitrary_bytes_and_logs_only_the_error_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let input = b"\x1b[31mred\x1b[0m \xff\xfe not utf-8 \x00 secret-content-sentinel\r\n".to_vec();
    let out = run(shim(tmp.path()).arg("--tee"), &input);
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, input);
    assert!(out.stderr.is_empty());
    assert!(!capture_file(tmp.path()).exists());
    let log = errors_log(tmp.path());
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.ends_with(" tee not_json\n"), "{log}");
    assert!(!log.contains("sentinel"));
}

#[test]
fn missing_or_unknown_arguments_behave_like_tee() {
    for args in [&[][..], &["--bogus"][..], &["pwsh"][..]] {
        let tmp = tempfile::tempdir().unwrap();
        let input = fancy_input();
        let out = run(shim(tmp.path()).args(args), &input);
        assert_eq!(out.code, Some(0), "{args:?}");
        assert_eq!(out.stdout, input, "{args:?}");
        assert!(out.stderr.is_empty());
        assert_whitelisted_capture(tmp.path());
    }
}

#[test]
fn empty_stdin_is_forwarded_as_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(shim(tmp.path()).arg("--tee"), b"");
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
    assert!(errors_log(tmp.path()).ends_with(" tee not_json\n"));
}

#[test]
fn default_prints_coloured_summary_and_captures() {
    let tmp = tempfile::tempdir().unwrap();
    // 30 s of slack keeps the minute/hour floors stable on a slow machine.
    let input = input_with_resets(3 * 3600 + 12 * 60 + 30, 2 * 86_400 + 4 * 3600 + 30);
    let out = run(shim(tmp.path()).arg("--default"), &input);
    assert_eq!(out.code, Some(0));
    assert!(out.stderr.is_empty());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "\x1b[38;5;213mOpus 5.5 (1M context)\x1b[0m · ctx \x1b[32m35%\x1b[0m · \
         5h \x1b[32m22%\x1b[0m \x1b[38;5;245m3h12m\x1b[0m · \
         7d \x1b[38;5;208m61%\x1b[0m \x1b[38;5;245m2d4h\x1b[0m\n"
    );
    assert!(capture_file(tmp.path()).exists());
}

#[test]
fn default_with_unusable_input_prints_neutral_line() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(shim(tmp.path()).arg("--default"), b"not json at all");
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, b"Claude\n");
    assert!(out.stderr.is_empty());
    assert!(errors_log(tmp.path()).ends_with(" default not_json\n"));
}

#[test]
fn version_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(shim(tmp.path()).arg("--version"), b"");
    assert_eq!(out.code, Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("cuw-capture {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn argv_mode_forwards_stdin_stdout_args_and_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let input = fancy_input();
    let out = run(
        shim(tmp.path()).args([
            "--",
            CHILD,
            "--print-args",
            "--exit",
            "7",
            "a b",
            r#"q"uote"#,
            "",
        ]),
        &input,
    );
    assert_eq!(out.code, Some(7));
    let mut expected = input.clone();
    expected.extend_from_slice(b"[args]\n--print-args\n--exit\n7\na b\nq\"uote\n\n");
    assert_eq!(out.stdout, expected);
    assert!(out.stderr.is_empty());
    assert_whitelisted_capture(tmp.path());
}

#[test]
fn argv_mode_falls_back_to_default_line_when_spawn_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let input = input_with_resets(600, 7200);
    let out = run(
        shim(tmp.path()).args(["--", "cuw-no-such-program-4711", "x"]),
        &input,
    );
    assert_eq!(out.code, Some(0));
    let line = String::from_utf8(out.stdout).unwrap();
    assert!(
        line.starts_with("\x1b[38;5;213mOpus 5.5 (1M context)\x1b[0m · ctx"),
        "{line:?}"
    );
    assert_eq!(line.lines().count(), 1);
    assert!(out.stderr.is_empty());
    assert!(capture_file(tmp.path()).exists());
}

#[test]
fn argv_mode_child_that_never_reads_stdin_does_not_hang() {
    let tmp = tempfile::tempdir().unwrap();
    // Far larger than any pipe buffer, so a blocking write on the main thread would deadlock.
    let mut json: Value = serde_json::from_str(FULL).unwrap();
    json["pad"] = "x".repeat(1 << 20).into();
    let input = serde_json::to_vec(&json).unwrap();
    let out = run(
        shim(tmp.path()).args(["--", CHILD, "--no-read", "--exit", "5"]),
        &input,
    );
    assert_eq!(out.code, Some(5));
    assert!(out.stdout.is_empty());
    assert!(out.elapsed < Duration::from_secs(20), "{:?}", out.elapsed);
    assert_whitelisted_capture(tmp.path());
}

/// Writes `input` but never closes stdin; returns (exit code, stdout, elapsed).
fn run_with_open_stdin(cmd: &mut Command, input: &[u8]) -> (Option<i32>, Vec<u8>, Duration) {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input).unwrap();
    stdin.flush().unwrap();
    let stdout = read_in_background(child.stdout.take().unwrap());
    let start = Instant::now();
    let status = wait_with_timeout(&mut child, Duration::from_secs(15));
    let elapsed = start.elapsed();
    drop(stdin);
    (
        status.and_then(|s| s.code()),
        stdout.join().unwrap(),
        elapsed,
    )
}

#[test]
fn watchdog_stops_waiting_for_stdin_that_never_closes() {
    let tmp = tempfile::tempdir().unwrap();
    let (code, stdout, elapsed) =
        run_with_open_stdin(shim(tmp.path()).arg("--tee"), FULL.as_bytes());
    assert_eq!(code, Some(0), "shim hung on an open stdin ({elapsed:?})");
    assert!(elapsed >= Duration::from_millis(1500), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    assert_eq!(stdout, FULL.as_bytes());
    assert!(capture_file(tmp.path()).exists());
    assert!(errors_log(tmp.path()).ends_with(" tee stdin_timeout\n"));
}

#[test]
fn watchdog_also_applies_in_argv_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let (code, stdout, elapsed) = run_with_open_stdin(
        shim(tmp.path()).args(["--", CHILD, "--exit", "2"]),
        FULL.as_bytes(),
    );
    assert_eq!(code, Some(2), "shim hung on an open stdin ({elapsed:?})");
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    assert_eq!(
        stdout,
        FULL.as_bytes(),
        "the child gets what arrived, then EOF"
    );
}

#[test]
fn diag_appends_names_but_never_values() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = shim(tmp.path());
    cmd.arg("--diag")
        .env("CLAUDE_CUW_DIAG_TEST", "env-value-sentinel");
    let out = run(&mut cmd, FULL.as_bytes());
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, b"cuw diag ok\n");
    assert!(out.stderr.is_empty());
    let out2 = run(&mut cmd, b"");
    assert_eq!(out2.stdout, b"cuw diag ok\n");

    let log = fs::read_to_string(capture_dir(tmp.path()).join("_diag.log")).unwrap();
    assert_eq!(
        log.matches("=== cuw-capture ").count(),
        2,
        "blocks are appended: {log}"
    );
    assert!(log.contains("cmdline: ") && log.contains("--diag"), "{log}");
    assert!(log.contains("parent: pid="), "{log}");
    assert!(log.contains("grandparent: "), "{log}");
    assert!(log.contains("CLAUDE_CUW_DIAG_TEST"), "{log}");
    assert!(
        log.contains(&format!("stdin_bytes: {}\n", FULL.len())),
        "{log}"
    );
    assert!(log.contains("stdin_bytes: 0\n"), "{log}");
    assert!(
        log.contains(
            "json_keys: hook_event_name, session_id, transcript_path, cwd, model, workspace, version, \
             output_style, cost, context_window, exceeds_200k_tokens, rate_limits\n"
        ),
        "{log}"
    );
    assert!(
        log.contains("rate_limits_keys: five_hour, seven_day, seven_day_opus, spend_limit\n"),
        "{log}"
    );
    for leaked in ["env-value-sentinel", "demo-cwd-sentinel", SID, "Opus 5.5"] {
        assert!(!log.contains(leaked), "diag leaked {leaked:?}: {log}");
    }
    assert_whitelisted_capture(tmp.path());
}

#[test]
fn errors_log_stays_under_64_kib() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = capture_dir(tmp.path());
    fs::create_dir_all(&dir).unwrap();
    let old: String = (0..3000)
        .map(|i| format!("2026-01-01T00:00:00.000Z tee old-{i:05}\n"))
        .collect();
    assert!(old.len() > 64 * 1024);
    fs::write(dir.join("_errors.log"), old).unwrap();
    for _ in 0..3 {
        run(shim(tmp.path()).arg("--tee"), b"garbage");
    }
    let log = errors_log(tmp.path());
    assert!((log.len() as u64) < 64 * 1024, "{}", log.len());
    assert!(log.ends_with(" tee not_json\n"));
    assert_eq!(log.matches(" tee not_json\n").count(), 3);
    assert!(
        log.lines().all(|l| l.starts_with("20") && !l.is_empty()),
        "only whole lines are kept"
    );
}

#[cfg(windows)]
#[test]
fn argv_mode_resolves_extensionless_programs_via_path_and_pathext() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::copy(CHILD, bin.join("cuwchild.exe")).unwrap();
    fs::write(
        bin.join("cuwwrap.cmd"),
        format!("@\"{CHILD}\" %*\r\n@exit /b %ERRORLEVEL%\r\n"),
    )
    .unwrap();
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let input = fancy_input();

    for (program, code) in [("cuwchild", 3), ("cuwwrap", 4), ("cuwwrap.cmd", 6)] {
        let data = tmp.path().join(format!("data-{program}"));
        let mut cmd = shim(&data);
        cmd.env("PATH", &path)
            .args(["--", program, "--exit", &code.to_string()]);
        let out = run(&mut cmd, &input);
        assert_eq!(out.code, Some(code), "{program}");
        assert_eq!(out.stdout, input, "{program}");
        assert_whitelisted_capture(&data);
    }
}

#[cfg(windows)]
#[test]
fn cmd_pipe_form_preserves_bytes() {
    use std::os::windows::process::CommandExt;

    // Both spellings: the installer writes the shim path with forward slashes.
    for shim_path in [SHIM.to_owned(), SHIM.replace('\\', "/")] {
        let tmp = tempfile::tempdir().unwrap();
        let input = fancy_input();
        let mut cmd = Command::new("cmd");
        cmd.raw_arg(format!(r#"/d /s /c ""{shim_path}" --tee | "{CHILD}"""#))
            .env("CUW_DATA_DIR", tmp.path());
        let out = run(&mut cmd, &input);
        assert_eq!(out.code, Some(0), "{shim_path}");
        assert_eq!(out.stdout, input, "{shim_path}");
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_whitelisted_capture(tmp.path());
    }
}

#[cfg(windows)]
#[test]
fn git_bash_pipe_form_preserves_bytes() {
    let bash = Path::new(r"C:\Program Files\Git\bin\bash.exe");
    if !bash.exists() {
        eprintln!("skipped: Git Bash not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let input = fancy_input();
    let mut cmd = Command::new(bash);
    cmd.args([
        "-c",
        r#""$0" --tee | "$1""#,
        &SHIM.replace('\\', "/"),
        &CHILD.replace('\\', "/"),
    ])
    .env("CUW_DATA_DIR", tmp.path());
    let out = run(&mut cmd, &input);
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, input);
    assert_whitelisted_capture(tmp.path());
}

#[cfg(windows)]
#[test]
fn killing_the_shim_kills_the_child() {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("child.pid");
    let mut shim_proc = shim(tmp.path())
        .args([
            "--",
            CHILD,
            "--no-read",
            "--sleep-ms",
            "60000",
            "--pid-file",
        ])
        .arg(&pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let start = Instant::now();
    let pid: u32 = loop {
        if let Some(pid) = fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            break pid;
        }
        assert!(start.elapsed() < TIMEOUT, "child never started");
        thread::sleep(Duration::from_millis(20));
    };
    // SAFETY: plain Win32 calls on a handle we own and close below.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(
        !handle.is_null(),
        "child already gone before the shim was killed"
    );

    shim_proc.kill().unwrap();
    shim_proc.wait().unwrap();
    // SAFETY: `handle` is a valid process handle opened above.
    let waited = unsafe { WaitForSingleObject(handle, 10_000) };
    // SAFETY: closed exactly once.
    unsafe { CloseHandle(handle) };
    assert_eq!(waited, WAIT_OBJECT_0, "child outlived the killed shim");
}
