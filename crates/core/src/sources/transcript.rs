//! Claude Code transcripts (`*.jsonl`) → current model and context size.
//!
//! Locations: `<claude_home>/projects/<project>/<session>.jsonl` (terminal CLI: entrypoint `cli`;
//! Desktop Code tab: `claude-desktop`) and Cowork under
//! `<desktop_root>/local-agent-mode-sessions/**/.claude/projects/**/<session>.jsonl`
//! (entrypoint `local-agent`). Files under any directory named `subagents` are ignored.
//!
//! Assistant line shape (only these fields are read; message content is NEVER parsed or kept):
//! ```json
//! {"type":"assistant","isSidechain":false,"sessionId":"…","entrypoint":"cli","cwd":"C:\\…\\proj",
//!  "timestamp":"2026-09-24T12:00:00.000Z",
//!  "message":{"model":"claude-opus-5-5","usage":{"input_tokens":3,"cache_creation_input_tokens":120,
//!   "cache_read_input_tokens":42000,"output_tokens":800}}}
//! ```
//! Context tokens = `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` of the
//! LAST qualifying assistant line (not a sum). Qualifying: `type == "assistant"`, `isSidechain` not
//! true, `message.model` present and not `"<synthetic>"`, `message.usage` present.
//!
//! 1M detection: the transcript head (first [`HEAD_BYTES`]) may contain an `attachment` line whose
//! identity model id ends with `[1m]`, e.g. `{"type":"attachment","attachment":{"type":"model",
//! "identity":{"modelId":"claude-opus-5-5[1m]"}}}` — search tolerantly for any string value
//! matching `claude-…[1m]` inside lines with `"type":"attachment"`. `message.model` never has the
//! suffix.

use std::path::{Path, PathBuf};

use crate::engine::types::Entrypoint;
use crate::saferead::SafeReader;
use crate::sources::SourceError;
use crate::time::Ms;

/// Bytes read from the end of the file on the first attempt.
pub const TAIL_BYTES: u64 = 256 * 1024;
/// Retry size when no complete qualifying line fits in [`TAIL_BYTES`].
pub const TAIL_RETRY_BYTES: u64 = 1024 * 1024;
/// Bytes read from the start of the file to find the identity attachment.
pub const HEAD_BYTES: u64 = 64 * 1024;
/// Directory depth limit when walking roots.
pub const MAX_WALK_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptTail {
    pub path: PathBuf,
    pub session_id: String,
    pub entrypoint: Entrypoint,
    /// `message.model` of the last qualifying line (never has `[1m]`).
    pub model_id: Option<String>,
    /// Context tokens of the last qualifying line.
    pub ctx_tokens: u64,
    /// Largest context-token count among qualifying lines in the scanned tail.
    pub max_ctx_tokens_seen: u64,
    /// `Some(true)` if the head's identity attachment says `[1m]`, `Some(false)` if an identity
    /// was found without it, `None` if no identity line was found.
    pub identity_1m: Option<bool>,
    /// `timestamp` of the last qualifying line.
    pub last_assistant_ms: Ms,
    /// Last path component of `cwd` (folder name only), if present.
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFile {
    pub path: PathBuf,
    pub modified_ms: Ms,
    pub len: u64,
}

/// Scans the end of one transcript. Reads the last [`TAIL_BYTES`] (retrying with
/// [`TAIL_RETRY_BYTES`] if no qualifying line was found and the file is larger); drops the first
/// partial line by searching for `\n` in BYTES before UTF-8 decoding (the seek may split a
/// multi-byte char); tolerates a partial last line (Claude Code may be mid-write) and CRLF.
/// Walks lines from the end; a cheap substring check for `"assistant"` precedes JSON parsing.
///
/// `cached_identity`: pass `Some(v)` to reuse a previous head scan for this path (skip reading
/// the head); `None` to scan the head now.
///
/// Returns `Ok(None)` if no qualifying line exists. `session_id` falls back to the file stem and
/// `entrypoint` to `Unknown` when missing.
pub fn scan_tail(
    reader: &SafeReader,
    path: &Path,
    cached_identity: Option<Option<bool>>,
) -> Result<Option<TranscriptTail>, SourceError> {
    let _ = (reader, path, cached_identity);
    todo!("transcript::scan_tail")
}

/// Walks `roots` (recursively, at most [`MAX_WALK_DEPTH`] levels, skipping dirs named
/// `subagents`, listing only via `reader.read_dir`) and returns `*.jsonl` files modified after
/// `newer_than_ms`, newest first, at most `limit`. Unreadable dirs are skipped silently.
pub fn find_recent(reader: &SafeReader, roots: &[PathBuf], newer_than_ms: Ms, limit: usize) -> Vec<RecentFile> {
    let _ = (reader, roots, newer_than_ms, limit);
    todo!("transcript::find_recent")
}
