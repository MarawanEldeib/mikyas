//! Claude Desktop Code-tab session metadata:
//! `<desktop_root>/claude-code-sessions/<account>/<org>/local_<id>.json`.
//!
//! Only these fields are read (serde must ignore every other field):
//! `cliSessionId` (string; equals the transcript `sessionId`), `model` (string, may end with
//! `[1m]`), `lastFocusedAt`, `lastActivityAt` (epoch ms/s number or RFC 3339 string — use
//! [`crate::time::json_time_to_ms`]). PRIVACY: the `<account>` and `<org>` directory names are
//! never stored.

use std::path::PathBuf;

use crate::saferead::SafeReader;
use crate::time::Ms;

pub const MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_WALK_DEPTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopSession {
    pub cli_session_id: Option<String>,
    pub model: Option<String>,
    pub last_focused_ms: Option<Ms>,
    pub last_activity_ms: Option<Ms>,
}

/// Loads every `local_*.json` below `dirs` (depth ≤ [`MAX_WALK_DEPTH`]) through the reader.
/// Files that fail to read or parse are skipped. Sorted by `last_activity_ms` descending.
pub fn load_all(reader: &SafeReader, dirs: &[PathBuf]) -> Vec<DesktopSession> {
    let _ = (reader, dirs);
    todo!("desktop_sessions::load_all")
}
