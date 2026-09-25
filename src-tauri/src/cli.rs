//! Command-line mode, handled before Tauri starts:
//! `claude-usage-widget.exe --disconnect [--quiet]` restores Claude Code's statusline and exits
//! (the uninstaller runs it). Anything else starts the app.

use cuw_core::paths::Paths;

/// `Some(exit_code)` when the process should exit without starting the UI.
pub fn handle_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--disconnect") {
        return None;
    }
    let quiet = args.iter().any(|a| a == "--quiet");
    let result = crate::connect::disconnect(&Paths::detect(), cuw_core::time::now_ms());
    match result {
        Ok(status) => {
            if !quiet {
                println!("disconnected: {status:?}");
            }
            Some(0)
        }
        Err(e) => {
            if !quiet {
                eprintln!("disconnect failed: {e}");
            }
            Some(1)
        }
    }
}
