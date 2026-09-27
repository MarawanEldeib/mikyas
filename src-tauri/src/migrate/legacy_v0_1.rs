//! SovaWatch 0.1.0's status-line code exactly as it was released (`crates/core/src/cmdline.rs`
//! and `claude_settings.rs` at tag v0.1.0, commit 9f3192c, with their tests removed and the two
//! `crate::` imports pointed here), so the migration tests can run what the OLD app — and its
//! uninstaller's `sovawatch.exe --disconnect --quiet` — would do against a moved status line.
//! Test-only; never change these files except to re-copy them from that commit (or to follow a
//! rename of the core crate in their imports).

use std::fs;

use mikyas_core::paths::Paths;

#[path = "legacy_v0_1_cmdline.rs"]
pub mod cmdline;

#[path = "legacy_v0_1_claude_settings.rs"]
pub mod claude_settings;

/// v0.1.0's `connect::disconnect` (the uninstaller's `--disconnect`): its own `wrap.json` from its
/// own data folder, the same settings file. Returns whether it wrote `settings.json`. (v0.1.0 also
/// backs up, re-reads for its compare-and-swap and deletes its backups; none of that changes
/// whether or what it writes.)
pub fn disconnect(paths: &Paths) -> Result<bool, String> {
    let wrap: Option<claude_settings::WrapRecord> =
        fs::read(paths.wrap_file()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let bytes = fs::read(paths.claude_settings()).unwrap_or_default();
    let Some(next) = claude_settings::disconnect(&bytes, wrap.as_ref()).map_err(|e| e.to_string())? else {
        let _ = fs::remove_file(paths.wrap_file());
        return Ok(false);
    };
    fs::write(paths.claude_settings(), next).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(paths.wrap_file());
    Ok(true)
}
