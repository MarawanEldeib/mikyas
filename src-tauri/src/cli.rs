//! Command-line mode, handled before Tauri starts:
//! `claude-usage-widget.exe --disconnect [--quiet]` restores Claude Code's statusline and exits
//! (the uninstaller runs it). Anything else starts the app.
//!
//! Exit codes: 0 when the statusline is restored or there was nothing of ours to undo (never
//! connected, already disconnected, no `settings.json`); 1 when `settings.json` could not be read
//! or written (e.g. it is not strict JSON), in which case nothing was changed.

use std::io::Write;

use cuw_core::paths::Paths;
use cuw_core::time::Ms;

/// `Some(exit_code)` when the process should exit without starting the UI.
pub fn handle_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    handle(
        &args,
        Paths::detect,
        cuw_core::time::now_ms(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

fn handle(
    args: &[String],
    paths: impl FnOnce() -> Paths,
    now: Ms,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Option<i32> {
    if !args.iter().any(|a| a == "--disconnect") {
        return None;
    }
    let quiet = args.iter().any(|a| a == "--quiet");
    // Writes are best-effort: a release build has no console attached.
    Some(match crate::connect::disconnect(&paths(), now) {
        Ok(status) => {
            if !quiet {
                let _ = writeln!(out, "disconnected: {status:?}");
            }
            0
        }
        Err(e) => {
            if !quiet {
                let _ = writeln!(err, "disconnect failed: {e}");
            }
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use cuw_core::cmdline::{SHIM_EXE_NAME, ShellKind};

    use super::*;
    use crate::connect::{self, ConnectEnv, ConnectionStatus, Shell};

    const ORIGINAL: &str = r#"{
  "statusLine": {
    "type": "command",
    "command": "pwsh -NoProfile -File \"C:/Users/tester/.claude/statusline.ps1\""
  }
}
"#;

    fn paths(tmp: &tempfile::TempDir) -> Paths {
        Paths::with_roots(
            tmp.path().join(".claude"),
            vec![tmp.path().join("Roaming").join("Claude")],
            tmp.path().join("data"),
        )
    }

    fn run(args: &[&str], paths: Paths) -> (Option<i32>, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = handle(&args, || paths, 1_000, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn other_arguments_start_the_app() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(run(&[], paths(&tmp)).0, None);
        assert_eq!(run(&["--quiet", "--minimized"], paths(&tmp)).0, None);
    }

    #[test]
    fn not_connected_is_a_quiet_success() {
        let tmp = tempfile::tempdir().unwrap();
        // No ~/.claude/settings.json at all.
        assert_eq!(run(&["--disconnect", "--quiet"], paths(&tmp)), (Some(0), String::new(), String::new()));
        assert!(!paths(&tmp).claude_settings().exists(), "nothing created");
        // Someone else's statusline: left alone byte for byte.
        let p = paths(&tmp);
        fs::create_dir_all(p.claude_home()).unwrap();
        fs::write(p.claude_settings(), ORIGINAL).unwrap();
        assert_eq!(run(&["--disconnect", "--quiet"], paths(&tmp)), (Some(0), String::new(), String::new()));
        assert_eq!(fs::read_to_string(p.claude_settings()).unwrap(), ORIGINAL);
        assert!(!p.backups_dir().exists(), "no edit, no backup");
        let (code, out, _) = run(&["--disconnect"], paths(&tmp));
        assert_eq!(code, Some(0));
        assert!(out.starts_with("disconnected: Foreign"), "{out}");
    }

    #[test]
    fn restores_a_connected_statusline() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(&tmp);
        fs::create_dir_all(p.claude_home()).unwrap();
        fs::write(p.claude_settings(), ORIGINAL).unwrap();
        let shim = tmp.path().join("sidecar").join(SHIM_EXE_NAME);
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, b"fake shim").unwrap();
        let env = ConnectEnv {
            paths: p.clone(),
            shell: Shell {
                kind: ShellKind::Pwsh,
                exe: PathBuf::from("pwsh.exe"),
            },
            shim_source: Some(shim),
            selftest: false,
        };
        connect::connect(&env, 500).unwrap();
        assert!(matches!(connect::status(&p), ConnectionStatus::Connected { .. }));

        assert_eq!(run(&["--quiet", "--disconnect"], p.clone()), (Some(0), String::new(), String::new()));
        assert_eq!(fs::read_to_string(p.claude_settings()).unwrap(), ORIGINAL);
        // A second run (e.g. the uninstaller after a manual Disconnect) is still a success.
        assert_eq!(run(&["--disconnect", "--quiet"], p).0, Some(0));
    }

    #[test]
    fn unreadable_settings_fail_without_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(&tmp);
        fs::create_dir_all(p.claude_home()).unwrap();
        let jsonc = "{\n  // a comment\n  \"statusLine\": {\"type\": \"command\", \"command\": \"x\"}\n}\n";
        fs::write(p.claude_settings(), jsonc).unwrap();
        assert_eq!(run(&["--disconnect", "--quiet"], p.clone()), (Some(1), String::new(), String::new()));
        assert_eq!(fs::read_to_string(p.claude_settings()).unwrap(), jsonc);
        let (code, _, err) = run(&["--disconnect"], p);
        assert_eq!(code, Some(1));
        assert!(err.starts_with("disconnect failed: "), "{err}");
    }
}
