//! Command-line mode, handled before Tauri starts:
//! `mikyas.exe --disconnect [--quiet]` restores Claude Code's statusline and exits
//! (the uninstaller runs it). Anything else starts the app. Without `--quiet` it also prints the
//! Claude Code `settings.json` it used (`CLAUDE_CONFIG_DIR` or `~/.claude`).
//!
//! Exit codes: 0 when the statusline is restored or there was nothing of ours to undo (never
//! connected, already disconnected, no `settings.json`); 1 when `settings.json` could not be read
//! or written (e.g. it is not strict JSON), in which case nothing was changed.

use std::io::Write;

use mikyas_core::paths::Paths;
use mikyas_core::time::Ms;

/// `Some(exit_code)` when the process should exit without starting the UI.
pub fn handle_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    handle(&args, Paths::detect, mikyas_core::time::now_ms(), &mut std::io::stdout(), &mut std::io::stderr())
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
    let paths = paths();
    let settings = paths.claude_settings();
    // Writes are best-effort: a release build has no console attached.
    Some(match crate::connect::disconnect(&paths, now) {
        Ok(status) => {
            if !quiet {
                let _ = writeln!(out, "disconnected: {status:?}\nsettings: {}", settings.display());
            }
            0
        }
        Err(e) => {
            if !quiet {
                let _ = writeln!(err, "disconnect failed: {e}\nsettings: {}", settings.display());
            }
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use mikyas_core::cmdline::{SHIM_EXE_NAME, ShellKind};

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
        assert!(out.ends_with(&format!("\nsettings: {}\n", p.claude_settings().display())), "{out}");
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
            shell: Shell { kind: ShellKind::Pwsh, exe: PathBuf::from("pwsh.exe") },
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
        let (code, _, err) = run(&["--disconnect"], p.clone());
        assert_eq!(code, Some(1));
        assert!(err.starts_with("disconnect failed: "), "{err}");
        assert!(err.ends_with(&format!("\nsettings: {}\n", p.claude_settings().display())), "{err}");
    }

    /// The uninstaller (windows/hooks.nsh) can't share these names with the code, so it is
    /// checked against them here.
    #[test]
    fn uninstaller_hooks_match_the_app() {
        let hooks = include_str!("../windows/hooks.nsh");
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        // The data folder is $LOCALAPPDATA\${PRODUCTNAME}.
        assert_eq!(conf["productName"], mikyas_core::paths::APP_DIR_NAME);
        assert!(hooks.contains(r"$LOCALAPPDATA\${PRODUCTNAME}"));
        assert!(!hooks.contains(r"$LOCALAPPDATA\Mikyas"));
        assert!(hooks.contains(&format!("ReadEnvStr $MikyasSettings {}", mikyas_core::paths::CLAUDE_CONFIG_DIR_ENV)));
        assert!(hooks.contains(&format!(r"\bin\{SHIM_EXE_NAME}")));
        // The files it names inside the data folder and Claude Code's config folder.
        let p = Paths::with_roots(PathBuf::from("claude"), Vec::new(), PathBuf::from("data"));
        let name = |path: PathBuf| path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(p.bin_dir(), p.data_root().join("bin"));
        assert!(hooks.contains(&format!(r"$MikyasDataDir\{}", name(p.wrap_file()))));
        assert!(hooks.contains(&format!(r"$MikyasDataDir\{}", name(p.backups_dir()))));
        assert!(hooks.contains(&format!(r#"StrCpy $MikyasSettings "$MikyasSettings\{}""#, name(p.claude_settings()))));
        assert!(hooks.contains(r#"StrCpy $MikyasSettings "$PROFILE\.claude""#));
    }
}
