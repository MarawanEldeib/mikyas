//! Building and recognising the statusline command that routes Claude Code's statusline JSON
//! through `cuw-capture.exe` without changing what the user's own statusline prints.
//!
//! Forms (the shim path is always double-quoted and uses forward slashes):
//! - Bash / Cmd, simple original:   `"<shim>" --tee | <original>`            (WrapMode::Pipe)
//! - Bash / Cmd, compound original: `"<shim>" --tee | (<original>)`          (WrapMode::PipeGrouped)
//!   compound = an UNQUOTED `&&`, `||`, `;`, `&` or newline. Quote rules differ per shell: bash
//!   honours '…' and "…" (with `\` escapes outside single quotes); cmd honours only "…" (a `'` is a
//!   literal character, and `^` escapes the next char). For Cmd, an original containing an
//!   unquoted `(` or `)` cannot be grouped safely → `CmdlineError::NeedsReview`.
//! - Pwsh (PowerShell ≥ 7.4, byte-preserving native pipes), simple original:
//!   `& "<shim>" --tee | <original>`                                        (WrapMode::Pipe)
//!   A compound original under Pwsh (unquoted `;`, `&&`, `||`, newline) → use Argv.
//! - LegacyPowerShell (< 7.4 re-encodes native-to-native pipes and would corrupt non-ASCII JSON),
//!   or compound under Pwsh: `& "<shim>" -- <original>`                     (WrapMode::Argv)
//!   The shell tokenises `<original>` and the shim spawns those tokens itself. Compound originals
//!   cannot be expressed this way → `CmdlineError::Unsupported`.
//! - No original statusline: `"<shim>" --default` (Bash/Cmd) or `& "<shim>" --default`
//!   (PowerShell kinds)                                                      (WrapMode::Default)
//!
//! [`unwrap`] must recognise every form produced by [`wrap`] (for any shim path that passes
//! [`validate_shim_path`]) and return the original text EXACTLY (for PipeGrouped, the text
//! between the outer parentheses). `unwrap(wrap(x).command).original == x` is a property test.

use serde::{Deserialize, Serialize};

pub const SHIM_EXE_NAME: &str = "cuw-capture.exe";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Bash,
    Cmd,
    /// PowerShell 7.4+.
    Pwsh,
    /// Windows PowerShell 5.1 or PowerShell < 7.4.
    LegacyPowerShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapMode {
    Pipe,
    PipeGrouped,
    Argv,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub command: String,
    pub mode: WrapMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwrapped {
    /// As written in the command (forward slashes).
    pub shim_path: String,
    pub mode: WrapMode,
    /// The user's original command; `None` for WrapMode::Default.
    pub original: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CmdlineError {
    #[error("shim path contains characters that are unsafe in a shell command: {0:?}")]
    UnsafeShimPath(String),
    #[error("the existing statusline command needs manual review before wrapping")]
    NeedsReview,
    #[error("this shell cannot wrap a compound statusline command")]
    Unsupported,
    #[error("the existing statusline command is empty")]
    EmptyOriginal,
}

/// Converts a filesystem path to the form used in commands (forward slashes).
pub fn shim_path_for_command(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Rejects paths containing any of `% ^ ! & | < > $ \` " ' ( ) ;` or control characters, and
/// paths not ending in [`SHIM_EXE_NAME`] (case-insensitive).
pub fn validate_shim_path(path: &str) -> Result<(), CmdlineError> {
    let _ = path;
    todo!("cmdline::validate_shim_path")
}

/// True if `cmd` contains an unquoted command separator for `shell` (see module docs).
pub fn is_compound(cmd: &str, shell: ShellKind) -> bool {
    let _ = (cmd, shell);
    todo!("cmdline::is_compound")
}

/// True if `cmd` contains an unquoted `(` or `)` for `shell`.
pub fn has_unquoted_parens(cmd: &str, shell: ShellKind) -> bool {
    let _ = (cmd, shell);
    todo!("cmdline::has_unquoted_parens")
}

/// Builds the wrapped command. `original = None` (or only whitespace → `EmptyOriginal` is NOT
/// raised; whitespace-only is treated as None) produces the Default form. If `original` is
/// already wrapped (recognised by [`unwrap`]), re-wraps its original with the new shim/shell.
pub fn wrap(original: Option<&str>, shim_path: &str, shell: ShellKind) -> Result<Wrapped, CmdlineError> {
    let _ = (original, shim_path, shell);
    todo!("cmdline::wrap")
}

/// Recognises a command produced by [`wrap`]. Returns `None` for anything else.
pub fn unwrap(command: &str) -> Option<Unwrapped> {
    let _ = command;
    todo!("cmdline::unwrap")
}
