//! Statusline capture: what `cuw-capture.exe` saves from Claude Code's statusline JSON.
//!
//! Claude Code pipes a JSON object to the statusline command on every update. The shim forwards
//! those bytes unchanged to the user's real statusline and then calls [`capture_from_bytes`],
//! which keeps ONLY the whitelisted fields below and writes one file per session:
//! `<capture_dir>/<session_id>.json`.
//!
//! Whitelist (everything else — `cwd`, `workspace`, `output_style`, cost totals, ... — is dropped):
//! - `session_id` (validated by [`sanitize_session_id`]; records without a valid id are ignored)
//! - `transcript_path`
//! - `model.id`, `model.display_name`
//! - `context_window.used_percentage`, `context_window.context_window_size`
//! - `exceeds_200k_tokens`
//! - `rate_limits.<key>.used_percentage` + `rate_limits.<key>.resets_at` (epoch **seconds**)
//!   for every key except `spend_limit` (gateway spend, not a plan window). A window missing either
//!   field is skipped. `used_percentage` is clamped to 0..=100; non-finite values are skipped.
//! - `cost.total_api_duration_ms` (only used to detect a real new API response)
//! - `version` (Claude Code version)
//!
//! Freshness: `refreshInterval` re-runs the statusline with the SAME cached data, so
//! `changed_at_ms` must only advance when the [`fingerprint`] changes. [`write_capture`] carries
//! `changed_at_ms` over from the existing file when the fingerprint is unchanged, and skips the
//! write entirely when the fingerprint is unchanged and the existing file's `written_at_ms` is
//! less than [`REWRITE_AFTER_MS`] old.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::time::{MINUTE_MS, Ms};

pub const CAPTURE_VERSION: u8 = 1;
/// The shim refuses stdin larger than this.
pub const MAX_STDIN_BYTES: usize = 4 * 1024 * 1024;
/// Captures larger than this are ignored when loading.
pub const MAX_CAPTURE_FILE_BYTES: u64 = 64 * 1024;
/// An unchanged capture is rewritten (to refresh `written_at_ms`) at most this often.
pub const REWRITE_AFTER_MS: Ms = 5 * MINUTE_MS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub v: u8,
    pub session_id: String,
    /// When the shim last wrote this file.
    pub written_at_ms: Ms,
    /// When the captured values last changed (the measurement time used by the engine).
    pub changed_at_ms: Ms,
    pub fingerprint: u64,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub model: Option<ModelInfo>,
    #[serde(default)]
    pub context: Option<CtxInfo>,
    /// Keyed by statusline window key (`five_hour`, `seven_day`, future keys).
    #[serde(default)]
    pub rate_limits: BTreeMap<String, RateLimit>,
    #[serde(default)]
    pub api_ms: Option<u64>,
    #[serde(default)]
    pub cc_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CtxInfo {
    pub used_percentage: Option<f32>,
    pub context_window_size: Option<u64>,
    #[serde(default)]
    pub exceeds_200k: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    /// 0..=100
    pub used_percentage: f32,
    /// Epoch seconds, as Claude Code reports it.
    pub resets_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    /// Fingerprint unchanged and the file is recent; nothing written.
    Skipped,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("stdin is not a JSON object")]
    NotJson,
    #[error("statusline JSON has no valid session_id")]
    NoSession,
    #[error("stdin larger than {MAX_STDIN_BYTES} bytes")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Accepts only `[A-Za-z0-9-]{1,64}` (Claude Code session ids are UUIDs). Anything else — path
/// separators, `..`, dots, spaces, over-long values — returns `None`, which blocks path traversal.
pub fn sanitize_session_id(raw: &str) -> Option<String> {
    let _ = raw;
    todo!("capture::sanitize_session_id")
}

/// Builds a record from a parsed statusline JSON value using the whitelist in the module docs.
/// Sets `written_at_ms = changed_at_ms = now_ms` and computes the fingerprint.
/// Returns `None` if `session_id` is missing or invalid.
pub fn extract_whitelisted(json: &serde_json::Value, now_ms: Ms) -> Option<CaptureRecord> {
    let _ = (json, now_ms);
    todo!("capture::extract_whitelisted")
}

/// FNV-1a over: rate limits (key, pct, resets_at in key order), context (pct, size, exceeds_200k),
/// `model.id`, and `api_ms`. Excludes timestamps and `transcript_path`.
pub fn fingerprint(rec: &CaptureRecord) -> u64 {
    let _ = rec;
    todo!("capture::fingerprint")
}

/// Atomically writes `<dir>/<session_id>.json` (create `dir` if needed; write
/// `.<session_id>.<pid>.<nanos>.tmp` in the same dir, then rename over the target, retrying the
/// rename up to 3 times 15 ms apart on Windows sharing violations). Applies the carry-over/skip
/// rules from the module docs by reading the existing file first (a missing or corrupt existing
/// file counts as "changed").
pub fn write_capture(dir: &Path, rec: CaptureRecord) -> io::Result<WriteOutcome> {
    let _ = (dir, rec);
    todo!("capture::write_capture")
}

/// The shim's whole job after forwarding stdin: size check, parse, extract, write.
pub fn capture_from_bytes(bytes: &[u8], dir: &Path, now_ms: Ms) -> Result<WriteOutcome, CaptureError> {
    let _ = (bytes, dir, now_ms);
    todo!("capture::capture_from_bytes")
}

/// Parses a capture file written by [`write_capture`]. Returns `None` for other schema versions
/// or malformed content.
pub fn read_capture(bytes: &[u8]) -> Option<CaptureRecord> {
    let _ = bytes;
    todo!("capture::read_capture")
}
