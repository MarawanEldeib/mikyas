//! Connect / Disconnect: edits Claude Code's `settings.json` so its statusline runs through the
//! capture shim — changing NOTHING else, byte for byte.
//!
//! Approach: SURGICAL SPLICING, never re-serialisation. Parse with serde_json only to validate and
//! to read values; then locate byte spans with a small JSON scanner and splice:
//! - Connect with an existing `statusLine` object whose `type` is `"command"`: replace only the
//!   span of the `statusLine.command` string literal with the JSON-escaped wrapped command
//!   (`serde_json::to_string`). All other bytes — key order, indentation, CRLF, BOM, trailing
//!   newline, other `statusLine` keys like `padding` / `refreshInterval` — are untouched.
//! - Connect with no `statusLine` key: insert a member `"statusLine": {"type": "command",
//!   "command": "<wrapped>"}` as the LAST member of the top-level object, formatted to match the
//!   file's detected indentation and line endings (2-space + LF for `JSON.stringify(x, null, 2)`
//!   files; handle an empty object `{}`).
//! - `statusLine` present but not `type: "command"` (or not an object) → `UnsupportedStatusLine`.
//! - Already connected (the command is recognised by `cmdline::unwrap`) with the same shim path
//!   and a mode valid for `shell` → no change (idempotent: returns the input bytes). Different
//!   shim path or shell → re-wrap the recorded original.
//! - Files that don't parse as strict JSON (comments / trailing commas = JSONC) → `NotStrictJson`.
//!   Top level not an object → `NotAnObject`.
//!
//! The [`WrapRecord`] keeps the RAW JSON literal of the original `command` (exact bytes including
//! quotes and escapes), so Disconnect can restore it byte-for-byte.
//!
//! Disconnect:
//! - Current command not recognised as ours → `Ok(None)` (leave untouched).
//! - Recognised, and its original text equals `wrap.original_command` → splice
//!   `wrap.original_command_raw` back.
//! - Recognised, but the user edited the tail since (differs from the record, or no record) →
//!   splice `serde_json::to_string(tail)`.
//! - Recognised Default form (we inserted the statusLine) → remove the whole `"statusLine"` member
//!   including its separating comma and the whitespace/newline before it, so a file that was only
//!   touched by Connect returns to its exact original bytes.
//! `disconnect(connect(x).bytes) == x` must hold byte-for-byte for every input `x` that connect
//! accepts (property/round-trip tests with LF, CRLF, BOM, 2/4-space and tab indents, unicode and
//! escaped characters, `{}` and one-line files).

use serde::{Deserialize, Serialize};

use crate::cmdline::{CmdlineError, ShellKind, WrapMode};
use crate::time::Ms;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WrapRecord {
    /// Decoded original `statusLine.command`; `None` if there was no statusLine.
    pub original_command: Option<String>,
    /// Exact JSON literal of the original command as it appeared in the file (with quotes).
    pub original_command_raw: Option<String>,
    pub shim_path: String,
    pub mode: WrapMode,
    pub shell: ShellKind,
    pub at_ms: Ms,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotConfigured,
    /// A statusLine the widget did not create (`command` is `None` for non-command types).
    Foreign { command: Option<String> },
    Connected {
        mode: WrapMode,
        shim_path: String,
        original: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("settings.json is not strict JSON (comments or trailing commas?) — edit it manually")]
    NotStrictJson,
    #[error("settings.json top level is not an object")]
    NotAnObject,
    #[error("statusLine is not a command-type statusline")]
    UnsupportedStatusLine,
    #[error(transparent)]
    Cmdline(#[from] CmdlineError),
}

/// Empty input (missing file) → `NotConfigured`.
pub fn status(bytes: &[u8]) -> Result<Status, SettingsError> {
    let _ = bytes;
    todo!("claude_settings::status")
}

/// Returns the new file bytes and the record to persist in `wrap.json`. Empty input (missing
/// file) is treated as `{}` and produces a minimal file `{\n  "statusLine": …\n}\n`.
pub fn connect(bytes: &[u8], shim_path: &str, shell: ShellKind, now_ms: Ms) -> Result<(Vec<u8>, WrapRecord), SettingsError> {
    let _ = (bytes, shim_path, shell, now_ms);
    todo!("claude_settings::connect")
}

/// `Ok(None)` when there is nothing of ours to undo.
pub fn disconnect(bytes: &[u8], wrap: Option<&WrapRecord>) -> Result<Option<Vec<u8>>, SettingsError> {
    let _ = (bytes, wrap);
    todo!("claude_settings::disconnect")
}
