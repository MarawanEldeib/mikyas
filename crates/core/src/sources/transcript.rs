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

use std::ffi::OsStr;
use std::fs::{DirEntry, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::engine::types::Entrypoint;
use crate::saferead::{ReadError, SafeReader};
use crate::sources::SourceError;
use crate::time::{Ms, json_time_to_ms};

/// Bytes read from the end of the file on the first attempt.
pub const TAIL_BYTES: u64 = 256 * 1024;
/// Retry size when no complete qualifying line fits in [`TAIL_BYTES`].
pub const TAIL_RETRY_BYTES: u64 = 1024 * 1024;
/// Bytes read from the start of the file to find the identity attachment.
pub const HEAD_BYTES: u64 = 64 * 1024;
/// Directory depth limit when walking roots.
pub const MAX_WALK_DEPTH: usize = 8;

/// Model placeholder Claude Code writes for locally generated (non-API) messages.
const SYNTHETIC_MODEL: &str = "<synthetic>";
/// Object keys whose string value is the session's identity model id.
const IDENTITY_KEYS: &[&[u8]] = &[b"modelId", b"model_id"];

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
/// `entrypoint` to `Unknown` when missing. A qualifying line without a usable `timestamp` falls
/// back to the file's modification time.
pub fn scan_tail(
    reader: &SafeReader,
    path: &Path,
    cached_identity: Option<Option<bool>>,
) -> Result<Option<TranscriptTail>, SourceError> {
    let mut file = reader.open(path).map_err(open_error)?;
    let meta = file.metadata().map_err(io_error)?;
    let len = meta.len();

    let (mut chunk, mut chunk_start) = read_tail(&mut file, len, TAIL_BYTES)?;
    let mut scan = scan_chunk(&chunk, chunk_start > 0);
    if scan.is_none() && len > TAIL_BYTES {
        (chunk, chunk_start) = read_tail(&mut file, len, TAIL_RETRY_BYTES)?;
        scan = scan_chunk(&chunk, chunk_start > 0);
    }
    let Some(TailScan { last, max_ctx_tokens }) = scan else {
        return Ok(None);
    };

    let identity_1m = match cached_identity {
        Some(cached) => cached,
        // The chunk already holds the start of the file.
        None if chunk_start == 0 => scan_head(&chunk[..chunk.len().min(HEAD_BYTES as usize)]),
        None => scan_head(&read_range(&mut file, 0, len.min(HEAD_BYTES))?),
    };

    let session_id = last
        .session_id
        .filter(|s| !s.is_empty())
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let last_assistant_ms = last
        .timestamp_ms
        .or_else(|| meta.modified().ok().and_then(system_time_ms))
        .unwrap_or(0);

    Ok(Some(TranscriptTail {
        path: path.to_path_buf(),
        session_id,
        entrypoint: last
            .entrypoint
            .as_deref()
            .map_or(Entrypoint::Unknown, Entrypoint::from_transcript),
        model_id: Some(last.model),
        ctx_tokens: last.ctx_tokens,
        max_ctx_tokens_seen: max_ctx_tokens.max(last.ctx_tokens),
        identity_1m,
        last_assistant_ms,
        project: last.cwd.as_deref().and_then(project_name),
    }))
}

/// Walks `roots` (recursively, at most [`MAX_WALK_DEPTH`] levels, skipping dirs named
/// `subagents`, listing only via `reader.read_dir`) and returns `*.jsonl` files modified after
/// `newer_than_ms`, newest first, at most `limit`. Unreadable dirs are skipped silently.
///
/// Depth: files directly inside a root are at level 1. Ties on modification time are ordered by
/// path; files the reader would refuse to open are left out; symlinks are not followed.
pub fn find_recent(reader: &SafeReader, roots: &[PathBuf], newer_than_ms: Ms, limit: usize) -> Vec<RecentFile> {
    if limit == 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let skip_subagents = |name: &OsStr| name.eq_ignore_ascii_case("subagents");
    for root in roots {
        walk_files(reader, root, MAX_WALK_DEPTH, &skip_subagents, &mut |entry| {
            let path = entry.path();
            if !has_extension(&path, "jsonl") {
                return;
            }
            let Ok(meta) = entry.metadata() else { return };
            let Some(modified_ms) = meta.modified().ok().and_then(system_time_ms) else {
                return;
            };
            if modified_ms > newer_than_ms && reader.allows(&path) {
                found.push(RecentFile {
                    path,
                    modified_ms,
                    len: meta.len(),
                });
            }
        });
    }
    found.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.path.cmp(&b.path)));
    found.dedup_by(|a, b| a.path == b.path); // overlapping roots
    found.truncate(limit);
    found
}

/// Calls `on_file` for every regular file below `root`, descending at most `max_depth` levels
/// (entries directly in `root` are level 1). Directories are listed only through the reader;
/// unreadable ones are skipped. Symlinks and junctions are never followed.
pub(crate) fn walk_files(
    reader: &SafeReader,
    root: &Path,
    max_depth: usize,
    skip_dir: &dyn Fn(&OsStr) -> bool,
    on_file: &mut dyn FnMut(&DirEntry),
) {
    walk_level(reader, root, 1, max_depth, skip_dir, on_file);
}

fn walk_level(
    reader: &SafeReader,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    skip_dir: &dyn Fn(&OsStr) -> bool,
    on_file: &mut dyn FnMut(&DirEntry),
) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = reader.read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_file() {
            on_file(&entry);
        } else if kind.is_dir() && !skip_dir(&entry.file_name()) {
            walk_level(reader, &entry.path(), depth + 1, max_depth, skip_dir, on_file);
        }
    }
}

/// Deserialises a field leniently: a value of the wrong type becomes `None` instead of failing
/// the whole record.
pub(crate) fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).ok())
}

pub(crate) fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

pub(crate) fn system_time_ms(t: SystemTime) -> Option<Ms> {
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Ms::try_from(d.as_millis()).ok()
}

// ---- tail scanning ----

/// The fields of one transcript line that the widget reads. Everything else — notably
/// `message.content` — is skipped by serde without being materialised.
#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type", default, deserialize_with = "lenient")]
    kind: Option<String>,
    #[serde(rename = "isSidechain", default, deserialize_with = "lenient")]
    is_sidechain: Option<bool>,
    #[serde(rename = "sessionId", default, deserialize_with = "lenient")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    entrypoint: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    cwd: Option<String>,
    #[serde(default)]
    timestamp: Option<serde_json::Value>,
    #[serde(default)]
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default, deserialize_with = "lenient")]
    model: Option<String>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default, deserialize_with = "lenient")]
    input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    cache_read_input_tokens: Option<u64>,
}

/// A qualifying assistant line, reduced to what [`TranscriptTail`] needs.
struct AssistantLine {
    session_id: Option<String>,
    entrypoint: Option<String>,
    model: String,
    cwd: Option<String>,
    timestamp_ms: Option<Ms>,
    ctx_tokens: u64,
}

struct TailScan {
    last: AssistantLine,
    max_ctx_tokens: u64,
}

fn qualify(line: RawLine) -> Option<AssistantLine> {
    if line.kind.as_deref() != Some("assistant") || line.is_sidechain == Some(true) {
        return None;
    }
    let message = line.message?;
    let model = message.model.filter(|m| !m.is_empty() && m != SYNTHETIC_MODEL)?;
    let usage = message.usage?;
    let ctx_tokens = [
        usage.input_tokens,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
    ]
    .into_iter()
    .flatten()
    .fold(0u64, u64::saturating_add);
    Some(AssistantLine {
        session_id: line.session_id,
        entrypoint: line.entrypoint,
        model,
        cwd: line.cwd,
        timestamp_ms: line.timestamp.as_ref().and_then(json_time_to_ms),
        ctx_tokens,
    })
}

/// Parses the lines of a chunk from the end. `starts_mid_file`: the chunk's first line is
/// (potentially) partial and is dropped.
fn scan_chunk(buf: &[u8], starts_mid_file: bool) -> Option<TailScan> {
    let body = if starts_mid_file {
        let newline = buf.iter().position(|&b| b == b'\n')?;
        &buf[newline + 1..]
    } else {
        buf
    };
    let mut last = None;
    let mut max_ctx_tokens = 0;
    for raw in body.rsplit(|&b| b == b'\n') {
        let line = raw.trim_ascii(); // also removes the `\r` of CRLF files
        if line.first() != Some(&b'{') || !contains(line, br#""assistant""#) {
            continue;
        }
        // A partial (mid-write) or otherwise malformed line simply fails to parse.
        let Ok(parsed) = serde_json::from_slice::<RawLine>(line) else { continue };
        let Some(assistant) = qualify(parsed) else { continue };
        max_ctx_tokens = max_ctx_tokens.max(assistant.ctx_tokens);
        if last.is_none() {
            last = Some(assistant);
        }
    }
    last.map(|last| TailScan { last, max_ctx_tokens })
}

/// Reads the last `n` bytes of a `len`-byte file. When the read starts mid-file it includes one
/// extra preceding byte, so a chunk that happens to start exactly on a line boundary keeps that
/// line (the extra `\n` is what gets dropped). Returns the bytes and their start offset.
fn read_tail(file: &mut File, len: u64, n: u64) -> Result<(Vec<u8>, u64), SourceError> {
    let start = if len > n { len - n - 1 } else { 0 };
    Ok((read_range(file, start, len - start)?, start))
}

/// Reads up to `n` bytes at `start`; a file that shrank meanwhile just yields fewer bytes.
fn read_range(file: &mut File, start: u64, n: u64) -> Result<Vec<u8>, SourceError> {
    file.seek(SeekFrom::Start(start)).map_err(io_error)?;
    let mut buf = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    file.take(n).read_to_end(&mut buf).map_err(io_error)?;
    Ok(buf)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Folder name of a Windows or POSIX style `cwd` (no drive or root on its own).
fn project_name(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    let name = trimmed.rsplit(['\\', '/']).next()?;
    if name.is_empty() || name.ends_with(':') {
        None
    } else {
        Some(name.to_string())
    }
}

fn open_error(e: ReadError) -> SourceError {
    match e {
        ReadError::Io(io) if io.kind() == io::ErrorKind::NotFound => SourceError::NotFound,
        other => SourceError::Read(other),
    }
}

fn io_error(e: io::Error) -> SourceError {
    SourceError::Read(ReadError::Io(e))
}

// ---- head (identity) scanning ----

/// Looks for the identity attachment in the first bytes of a transcript. The last line may be
/// cut off at the head boundary; the scan tolerates that.
fn scan_head(head: &[u8]) -> Option<bool> {
    let mut identity_seen = false;
    for line in head.split(|&b| b == b'\n') {
        if !contains(line, b"attachment") {
            continue;
        }
        let scan = scan_attachment_line(line);
        if scan.is_attachment {
            if scan.has_1m {
                return Some(true);
            }
            identity_seen |= scan.has_identity;
        }
    }
    identity_seen.then_some(false)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AttachmentScan {
    /// The top-level `type` is `attachment`, or — for a line cut off before its `type` key — it
    /// has a top-level `attachment` key.
    is_attachment: bool,
    /// A `modelId` string value was seen.
    has_identity: bool,
    /// A `claude-…[1m]` string value (or a `modelId` ending in `[1m]`) was seen.
    has_1m: bool,
}

/// A tiny JSON tokenizer over one (possibly truncated) line: it tracks nesting depth and string
/// literals, so text that merely mentions a model id inside a longer string never matches, and
/// key order or whitespace do not matter.
fn scan_attachment_line(line: &[u8]) -> AttachmentScan {
    let mut out = AttachmentScan::default();
    let mut top_type: Option<&[u8]> = None;
    let mut attachment_key = false;
    let mut depth = 0usize;
    let mut pending_key: Option<&[u8]> = None;
    let mut i = 0;
    while i < line.len() {
        match line[i] {
            b'"' => {
                let Some((text, next)) = string_literal(line, i) else { break };
                let after = skip_whitespace(line, next);
                if line.get(after) == Some(&b':') {
                    attachment_key |= depth == 1 && text == b"attachment";
                    pending_key = Some(text);
                    i = after + 1;
                    continue;
                }
                let key = pending_key.take();
                if depth == 1 && key == Some(b"type".as_slice()) && top_type.is_none() {
                    top_type = Some(text);
                }
                if key.is_some_and(|k| IDENTITY_KEYS.contains(&k)) {
                    out.has_identity = true;
                    out.has_1m |= ends_with_1m(text);
                }
                if text.starts_with(b"claude-") && ends_with_1m(text) && is_model_id_like(text) {
                    out.has_1m = true;
                }
                i = next;
            }
            b'{' | b'[' => {
                depth += 1;
                pending_key = None;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                pending_key = None;
                i += 1;
            }
            b',' => {
                pending_key = None;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.is_attachment = top_type.map_or(attachment_key, |t| t == b"attachment");
    out
}

/// The raw (still escaped) contents of the string literal opening at `line[open]`, and the index
/// after its closing quote. `None` if the literal is unterminated (truncated line).
fn string_literal(line: &[u8], open: usize) -> Option<(&[u8], usize)> {
    let mut j = open + 1;
    while j < line.len() {
        match line[j] {
            b'\\' => j += 2,
            b'"' => return Some((&line[open + 1..j], j + 1)),
            _ => j += 1,
        }
    }
    None
}

fn skip_whitespace(line: &[u8], mut i: usize) -> usize {
    while line.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn ends_with_1m(text: &[u8]) -> bool {
    text.len() >= 4 && text[text.len() - 4..].eq_ignore_ascii_case(b"[1m]")
}

fn is_model_id_like(text: &[u8]) -> bool {
    text.len() <= 128
        && text
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'[' | b']' | b'@' | b':'))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::paths::Paths;

    const SID: &str = "00000000-0000-4000-8000-000000000001";
    const FIXTURE: &str = include_str!("../../tests/fixtures/transcript/basic.jsonl");

    struct Env {
        tmp: tempfile::TempDir,
        paths: Paths,
        reader: SafeReader,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_roots(
            tmp.path().join(".claude"),
            vec![tmp.path().join("Roaming").join("Claude")],
            tmp.path().join("data"),
        );
        let reader = SafeReader::new(&paths);
        Env { tmp, paths, reader }
    }

    impl Env {
        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.paths.projects_dir().join(rel);
            write_file(&path, bytes);
            path
        }

        fn scan(&self, path: &Path) -> Option<TranscriptTail> {
            scan_tail(&self.reader, path, None).unwrap()
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn set_mtime(path: &Path, ms: Ms) {
        let t = UNIX_EPOCH + Duration::from_millis(ms as u64);
        File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    /// A realistic assistant line with all the extra fields Claude Code writes.
    fn assistant(model: &str, input: u64, cache_create: u64, cache_read: u64) -> serde_json::Value {
        json!({
            "parentUuid": "00000000-0000-4000-8000-000000000002",
            "isSidechain": false,
            "userType": "external",
            "cwd": "C:\\Users\\tester\\proj",
            "sessionId": SID,
            "version": "3.1.0",
            "gitBranch": "main",
            "entrypoint": "cli",
            "message": {
                "model": model,
                "id": "msg_00000000000000000000000001",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Done. The assistant finished the task."}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": cache_create,
                    "cache_read_input_tokens": cache_read,
                    "output_tokens": 800,
                    "output_tokens_details": {"thinking_tokens": 0},
                    "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": cache_create},
                    "iterations": 1,
                    "service_tier": "standard",
                    "speed": "standard"
                }
            },
            "requestId": "req_00000000000000000000000001",
            "type": "assistant",
            "uuid": "00000000-0000-4000-8000-000000000003",
            "timestamp": "2026-09-24T12:00:00.000Z"
        })
    }

    fn user(text: &str) -> serde_json::Value {
        json!({
            "parentUuid": null,
            "isSidechain": false,
            "cwd": "C:\\Users\\tester\\proj",
            "sessionId": SID,
            "entrypoint": "cli",
            "type": "user",
            "message": {"role": "user", "content": text},
            "uuid": "00000000-0000-4000-8000-000000000004",
            "timestamp": "2026-09-24T12:00:01.000Z"
        })
    }

    fn with(mut v: serde_json::Value, pointer: &str, new: serde_json::Value) -> serde_json::Value {
        *v.pointer_mut(pointer).unwrap() = new;
        v
    }

    fn lines(values: &[serde_json::Value]) -> Vec<u8> {
        let mut out = String::new();
        for v in values {
            out.push_str(&v.to_string());
            out.push('\n');
        }
        out.into_bytes()
    }

    const T_NOON: Ms = 1_790_251_200_000; // 2026-09-24T12:00:00Z

    #[test]
    fn fixture_picks_last_qualifying_line() {
        let e = env();
        let path = e.write("proj/00000000-0000-4000-8000-000000000001.jsonl", FIXTURE.as_bytes());
        let tail = e.scan(&path).expect("qualifying line");
        assert_eq!(
            tail,
            TranscriptTail {
                path: path.clone(),
                session_id: SID.into(),
                entrypoint: Entrypoint::Cli,
                model_id: Some("claude-opus-5-5".into()),
                ctx_tokens: 3 + 1_200 + 423_933,
                max_ctx_tokens_seen: 3 + 1_200 + 423_933,
                identity_1m: Some(true),
                last_assistant_ms: T_NOON + 5 * 60_000,
                project: Some("proj".into()),
            }
        );
    }

    #[test]
    fn last_line_wins_and_max_is_tracked() {
        let e = env();
        let first = assistant("claude-opus-5-5", 10, 0, 250_000);
        let second = with(assistant("claude-sonnet-5", 1, 2, 3), "/timestamp", json!("2026-09-24T12:01:00Z"));
        let path = e.write("p/s.jsonl", &lines(&[first, user("next"), second, user("thanks")]));
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.model_id.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(tail.ctx_tokens, 6);
        assert_eq!(tail.max_ctx_tokens_seen, 250_010);
        assert_eq!(tail.last_assistant_ms, T_NOON + 60_000);
    }

    #[test]
    fn non_qualifying_lines_are_skipped() {
        let e = env();
        let good = assistant("claude-opus-5-5", 5, 0, 0);
        let sidechain = with(assistant("claude-haiku-4-5", 1, 1, 1), "/isSidechain", json!(true));
        let synthetic = with(assistant("<synthetic>", 0, 0, 0), "/message/usage", json!({"input_tokens": 0}));
        let no_usage = {
            let mut v = assistant("claude-haiku-4-5", 1, 1, 1);
            v["message"].as_object_mut().unwrap().remove("usage");
            v
        };
        let null_usage = with(assistant("claude-haiku-4-5", 1, 1, 1), "/message/usage", json!(null));
        let no_model = {
            let mut v = assistant("x", 1, 1, 1);
            v["message"].as_object_mut().unwrap().remove("model");
            v
        };
        let user_with_usage = with(assistant("claude-haiku-4-5", 1, 1, 1), "/type", json!("user"));
        let bad_shape = with(assistant("claude-haiku-4-5", 1, 1, 1), "/message", json!("assistant"));
        let path = e.write(
            "p/s.jsonl",
            &lines(&[
                good,
                sidechain,
                synthetic,
                no_usage,
                null_usage,
                no_model,
                user_with_usage,
                bad_shape,
                user("assistant"),
                json!(["assistant", false]),
            ]),
        );
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.model_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!((tail.ctx_tokens, tail.max_ctx_tokens_seen), (5, 5));
    }

    #[test]
    fn lenient_field_types() {
        let e = env();
        // Odd field types degrade to "missing" instead of dropping the line.
        let v = assistant("claude-opus-5-5", 5, 0, 0);
        let v = with(v, "/message/usage/cache_read_input_tokens", json!("lots"));
        let v = with(v, "/isSidechain", json!("no"));
        let v = with(v, "/sessionId", json!(42));
        let v = with(v, "/timestamp", json!(1_790_251_200));
        let path = e.write("p/fallback-stem.jsonl", &lines(&[v]));
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.ctx_tokens, 5);
        assert_eq!(tail.session_id, "fallback-stem");
        assert_eq!(tail.last_assistant_ms, T_NOON, "epoch-second timestamps are accepted");
    }

    #[test]
    fn no_qualifying_line_is_none() {
        let e = env();
        let path = e.write("p/s.jsonl", &lines(&[user("hi"), user("assistant please")]));
        assert_eq!(e.scan(&path), None);
        let empty = e.write("p/empty.jsonl", b"");
        assert_eq!(e.scan(&empty), None);
        let garbage = e.write("p/garbage.jsonl", b"\x00\xff\xfe not json \"assistant\"\n{{{{\n");
        assert_eq!(e.scan(&garbage), None);
    }

    #[test]
    fn missing_or_denied_files_are_errors() {
        let e = env();
        let missing = e.paths.projects_dir().join("p").join("missing.jsonl");
        assert!(matches!(scan_tail(&e.reader, &missing, None), Err(SourceError::NotFound)));
        let outside = e.tmp.path().join("elsewhere").join("s.jsonl");
        write_file(&outside, &lines(&[assistant("claude-opus-5-5", 1, 1, 1)]));
        assert!(matches!(
            scan_tail(&e.reader, &outside, None),
            Err(SourceError::Read(ReadError::Denied(_)))
        ));
    }

    #[test]
    fn missing_fields_fall_back() {
        let e = env();
        let mut v = assistant("claude-opus-5-5", 1, 2, 3);
        let obj = v.as_object_mut().unwrap();
        obj.remove("sessionId");
        obj.remove("entrypoint");
        obj.remove("cwd");
        obj.remove("timestamp");
        let path = e.write("p/00000000-0000-4000-8000-00000000000a.jsonl", &lines(&[v]));
        set_mtime(&path, T_NOON + 1234);
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.session_id, "00000000-0000-4000-8000-00000000000a");
        assert_eq!(tail.entrypoint, Entrypoint::Unknown);
        assert_eq!(tail.project, None);
        assert_eq!(tail.last_assistant_ms, T_NOON + 1234, "mtime fallback");
    }

    #[test]
    fn entrypoints_are_mapped() {
        let e = env();
        for (raw, expected) in [
            ("cli", Entrypoint::Cli),
            ("claude-desktop", Entrypoint::Desktop),
            ("local-agent", Entrypoint::Cowork),
            ("something-new", Entrypoint::Unknown),
        ] {
            let v = with(assistant("claude-opus-5-5", 1, 1, 1), "/entrypoint", json!(raw));
            let path = e.write(&format!("p/{raw}.jsonl"), &lines(&[v]));
            assert_eq!(e.scan(&path).unwrap().entrypoint, expected, "{raw}");
        }
    }

    #[test]
    fn project_from_windows_and_posix_cwd() {
        assert_eq!(project_name("C:\\Users\\tester\\proj").as_deref(), Some("proj"));
        assert_eq!(project_name("C:\\Users\\tester\\proj\\").as_deref(), Some("proj"));
        assert_eq!(project_name("/home/tester/proj").as_deref(), Some("proj"));
        assert_eq!(project_name("/home/tester/my proj/").as_deref(), Some("my proj"));
        assert_eq!(project_name("C:/Users/tester/mixed\\café").as_deref(), Some("café"));
        assert_eq!(project_name("\\\\server\\share\\proj").as_deref(), Some("proj"));
        assert_eq!(project_name("proj").as_deref(), Some("proj"));
        assert_eq!(project_name("C:\\"), None);
        assert_eq!(project_name("/"), None);
        assert_eq!(project_name(""), None);

        let e = env();
        let v = with(assistant("claude-opus-5-5", 1, 1, 1), "/cwd", json!("/home/tester/posix-proj"));
        let path = e.write("p/s.jsonl", &lines(&[v]));
        assert_eq!(e.scan(&path).unwrap().project.as_deref(), Some("posix-proj"));
    }

    #[test]
    fn partial_last_line_is_ignored() {
        let e = env();
        let good = assistant("claude-opus-5-5", 1, 0, 0);
        let newer = assistant("claude-sonnet-5", 999, 0, 0).to_string();
        let mut bytes = lines(&[good]);
        bytes.extend_from_slice(&newer.as_bytes()[..newer.len() / 2]); // mid-write, no newline
        let path = e.write("p/s.jsonl", &bytes);
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.model_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(tail.max_ctx_tokens_seen, 1);

        // A complete final line without a trailing newline is fine.
        let mut bytes = lines(&[assistant("claude-opus-5-5", 1, 0, 0)]);
        bytes.extend_from_slice(newer.as_bytes());
        let path = e.write("p/t.jsonl", &bytes);
        assert_eq!(e.scan(&path).unwrap().ctx_tokens, 999);
    }

    #[test]
    fn crlf_files() {
        let e = env();
        let body = [
            user("hi").to_string(),
            assistant("claude-opus-5-5", 7, 0, 0).to_string(),
            user("bye").to_string(),
        ]
        .join("\r\n")
            + "\r\n";
        let path = e.write("p/s.jsonl", body.as_bytes());
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.ctx_tokens, 7);
        assert_eq!(tail.project.as_deref(), Some("proj"));
        assert_eq!(tail.session_id, SID);
    }

    /// `{"type":"user","pad":"xxx…` — a line prefix with no newline, to be continued by `rest`.
    fn filler_prefix(len: usize) -> Vec<u8> {
        let head = br#"{"type":"user","pad":""#;
        let mut v = head.to_vec();
        v.resize(len.max(head.len()), b'x');
        v
    }

    #[test]
    fn partial_first_line_after_seek_is_dropped() {
        let e = env();
        let good = lines(&[assistant("claude-opus-5-5", 1_000, 0, 0)]);
        // The line straddling the seek point continues with text that would parse as a
        // qualifying assistant line if the partial first line were not dropped.
        let mut decoy = assistant("claude-opus-5-5", 999_999, 0, 0);
        let base_len = decoy.to_string().len() + 1;
        let pad = TAIL_BYTES as usize - good.len() - base_len - r#","pad":"""#.len();
        decoy.as_object_mut().unwrap().insert("pad".into(), json!("y".repeat(pad)));
        let decoy = lines(&[decoy]);
        assert_eq!(decoy.len() + good.len(), TAIL_BYTES as usize);

        let mut bytes = filler_prefix(10_000);
        bytes.extend_from_slice(&decoy);
        bytes.extend_from_slice(&good);
        let path = e.write("p/s.jsonl", &bytes);
        let tail = e.scan(&path).unwrap();
        assert_eq!((tail.ctx_tokens, tail.max_ctx_tokens_seen), (1_000, 1_000));
    }

    #[test]
    fn chunk_starting_on_a_line_boundary_keeps_the_first_line() {
        let e = env();
        let big = {
            let mut v = assistant("claude-opus-5-5", 42, 0, 0);
            v.as_object_mut().unwrap().insert("pad".into(), json!("z".repeat(10)));
            let base = lines(&[v.clone()]).len();
            v["pad"] = json!("z".repeat(10 + TAIL_BYTES as usize - base));
            lines(&[v])
        };
        assert_eq!(big.len(), TAIL_BYTES as usize, "exactly fills the first chunk");
        let mut bytes = lines(&[user("older")]);
        bytes.extend_from_slice(&big);
        let path = e.write("p/s.jsonl", &bytes);
        assert_eq!(e.scan(&path).unwrap().ctx_tokens, 42);
    }

    #[test]
    fn multibyte_char_split_at_seek_boundary() {
        let e = env();
        let cwd = json!("C:\\Users\\tester\\café ☕");
        let good = lines(&[with(assistant("claude-opus-5-5", 77, 0, 0), "/cwd", cwd)]);
        // A user line full of 4-byte chars; choose ASCII padding so the seek lands inside one.
        let emoji = "😀";
        let count = TAIL_BYTES as usize / 4 + 1_000;
        let make = |pad: usize| {
            let mut s = String::from(r#"{"type":"user","message":{"content":""#);
            s.push_str(&emoji.repeat(count));
            s.push_str(&"a".repeat(pad));
            s.push_str("\"}}\n");
            s.into_bytes()
        };
        let pad = (0..4)
            .find(|&pad| {
                let filler = make(pad);
                let start = filler.len() + good.len() - TAIL_BYTES as usize;
                filler[start] & 0xC0 == 0x80 && filler[start - 1] & 0xC0 == 0x80
            })
            .unwrap();
        let mut bytes = make(pad);
        bytes.extend_from_slice(&good);
        let start = bytes.len() - TAIL_BYTES as usize;
        assert!(std::str::from_utf8(&bytes[start..]).is_err(), "boundary splits a character");
        let path = e.write("p/s.jsonl", &bytes);
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.ctx_tokens, 77);
        assert_eq!(tail.project.as_deref(), Some("café ☕"));
    }

    #[test]
    fn long_line_triggers_retry() {
        let e = env();
        let earlier = assistant("claude-opus-5-5", 9_000, 0, 0);
        let long = {
            let mut v = assistant("claude-opus-5-5", 7_777, 0, 0);
            v["message"]["content"] = json!([{"type": "tool_use", "input": "q".repeat(300 * 1024)}]);
            v
        };
        let bytes = lines(&[earlier, long, user("short")]);
        assert!(bytes.len() as u64 > TAIL_BYTES && (bytes.len() as u64) < TAIL_RETRY_BYTES);
        let path = e.write("p/s.jsonl", &bytes);
        let tail = e.scan(&path).unwrap();
        assert_eq!(tail.ctx_tokens, 7_777);
        assert_eq!(tail.max_ctx_tokens_seen, 9_000, "max covers the retry chunk");
    }

    #[test]
    fn line_longer_than_retry_is_none() {
        let e = env();
        let huge = {
            let mut v = assistant("claude-opus-5-5", 1, 0, 0);
            v["message"]["content"] = json!("w".repeat(TAIL_RETRY_BYTES as usize + 1024));
            v
        };
        let path = e.write("p/s.jsonl", &lines(&[huge, user("short")]));
        assert_eq!(e.scan(&path), None);
    }

    #[test]
    fn retry_not_needed_keeps_first_chunk_max() {
        let e = env();
        // A high-token line outside the first chunk is not part of max_ctx_tokens_seen.
        let old = assistant("claude-opus-5-5", 500_000, 0, 0);
        let filler = user(&"f".repeat(300 * 1024));
        let recent = assistant("claude-opus-5-5", 10, 0, 0);
        let path = e.write("p/s.jsonl", &lines(&[old, filler, recent]));
        let tail = e.scan(&path).unwrap();
        assert_eq!((tail.ctx_tokens, tail.max_ctx_tokens_seen), (10, 10));
    }

    fn identity_line(model_id: &str) -> serde_json::Value {
        json!({
            "parentUuid": null,
            "isSidechain": false,
            "attachment": {"type": "model", "identity": {"modelId": model_id, "displayName": "Opus 5.5"}},
            "type": "attachment",
            "uuid": "00000000-0000-4000-8000-000000000005",
            "timestamp": "2026-09-24T11:59:00.000Z"
        })
    }

    #[test]
    fn identity_detection() {
        let e = env();
        let a = assistant("claude-opus-5-5", 1, 1, 1);
        let cases: Vec<(&str, Vec<u8>, Option<bool>)> = vec![
            ("one_m", lines(&[user("hi"), identity_line("claude-opus-5-5[1m]"), a.clone()]), Some(true)),
            ("plain", lines(&[identity_line("claude-opus-5-5"), a.clone()]), Some(false)),
            ("none", lines(&[user("hi"), a.clone()]), None),
            // Whitespace and key order do not matter.
            (
                "pretty",
                format!(
                    "{}{}\n{a}\n",
                    r#"{"uuid": "00000000-0000-4000-8000-000000000006", "type" : "attachment", "#,
                    r#""attachment": { "identity": { "modelId" : "claude-opus-5-5[1m]" } } }"#,
                )
                .into_bytes(),
                Some(true),
            ),
            // Any claude-…[1m] string value inside an attachment line counts.
            (
                "other_key",
                lines(&[json!({"type": "attachment", "attachment": {"model": "claude-sonnet-5[1m]"}}), a.clone()]),
                Some(true),
            ),
            // Mentions outside attachment lines or inside longer strings do not.
            (
                "mentions",
                lines(&[
                    user("switch to claude-opus-5-5[1m] please"),
                    json!({"type": "user", "note": "claude-opus-5-5[1m]", "attachment": "x"}),
                    json!({"type": "attachment", "attachment": {"content": "\"claude-opus-5-5[1m]\""}}),
                    a.clone(),
                ]),
                None,
            ),
        ];
        for (name, bytes, expected) in cases {
            let path = e.write(&format!("p/{name}.jsonl"), &bytes);
            assert_eq!(e.scan(&path).unwrap().identity_1m, expected, "{name}");
        }
    }

    #[test]
    fn identity_in_head_of_large_file_and_truncated_line() {
        let e = env();
        // The identity line itself is longer than HEAD_BYTES and gets cut off.
        let mut ident = identity_line("claude-opus-5-5[1m]");
        ident["attachment"]["extra"] = json!("e".repeat(HEAD_BYTES as usize * 2));
        let filler = user(&"f".repeat(TAIL_BYTES as usize));
        let path = e.write("p/s.jsonl", &lines(&[ident, filler, assistant("claude-opus-5-5", 1, 1, 1)]));
        assert!(std::fs::metadata(&path).unwrap().len() > TAIL_BYTES + HEAD_BYTES);
        assert_eq!(e.scan(&path).unwrap().identity_1m, Some(true));
    }

    #[test]
    fn cached_identity_skips_head_scan() {
        let e = env();
        let path = e.write(
            "p/s.jsonl",
            &lines(&[identity_line("claude-opus-5-5[1m]"), assistant("claude-opus-5-5", 1, 1, 1)]),
        );
        let scan = |cached| scan_tail(&e.reader, &path, cached).unwrap().unwrap().identity_1m;
        assert_eq!(scan(None), Some(true));
        assert_eq!(scan(Some(Some(false))), Some(false), "cached value is used as-is");
        assert_eq!(scan(Some(None)), None);
        assert_eq!(scan(Some(Some(true))), Some(true));
    }

    #[test]
    fn attachment_tokenizer() {
        let scan = |s: &str| scan_attachment_line(s.as_bytes());
        assert_eq!(
            scan(r#"{"type":"attachment","attachment":{"identity":{"modelId":"claude-opus-5-5[1m]"}}}"#),
            AttachmentScan {
                is_attachment: true,
                has_identity: true,
                has_1m: true
            }
        );
        // Nested "type":"attachment" is not a top-level type, and an explicit other type wins
        // over an `attachment` key.
        assert!(!scan(r#"{"type":"user","x":{"type":"attachment"}}"#).is_attachment);
        assert!(!scan(r#"{"attachment":{"modelId":"claude-opus-5-5[1m]"},"type":"user"}"#).is_attachment);
        assert!(scan(r#"{"attachment":{"modelId":"claude-opus-5-5[1m]"},"type":"attachment"}"#).is_attachment);
        // Truncated inside a string: what was seen so far still counts.
        let s = scan(r#"{"attachment":{"identity":{"modelId":"claude-opus-5-5[1m]","text":"unterminated"#);
        assert!(s.is_attachment && s.has_1m);
        // Escaped quotes do not end a string early.
        let s = scan(r#"{"type":"attachment","a":"say \"claude-opus-5-5[1m]\" twice"}"#);
        assert!(s.is_attachment && !s.has_1m);
        // Aliases in modelId count too.
        assert!(scan(r#"{"type":"attachment","identity":{"modelId":"opus[1M]"}}"#).has_1m);
        // Garbage never panics.
        for junk in ["\"", "\"\\", "{{{{", "}}}]]]", ":::", "\"a\":", "{\"a\":\"b\\"] {
            let _ = scan(junk);
        }
    }

    #[test]
    fn cowork_transcripts_are_readable() {
        let e = env();
        let path = e.paths.desktop_roots()[0]
            .join("local-agent-mode-sessions")
            .join("acct")
            .join("org")
            .join("sess")
            .join(".claude")
            .join("projects")
            .join("proj")
            .join("s.jsonl");
        let v = with(assistant("claude-opus-5-5", 1, 1, 1), "/entrypoint", json!("local-agent"));
        write_file(&path, &lines(&[v]));
        assert_eq!(e.scan(&path).unwrap().entrypoint, Entrypoint::Cowork);
    }

    // ---- find_recent ----

    fn touch(path: &Path, ms: Ms) {
        write_file(path, b"{}\n");
        set_mtime(path, ms);
    }

    fn names(files: &[RecentFile]) -> Vec<String> {
        files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn find_recent_filters_orders_and_limits() {
        let e = env();
        let root = e.paths.projects_dir();
        touch(&root.join("a").join("old.jsonl"), T_NOON - 10_000);
        touch(&root.join("a").join("edge.jsonl"), T_NOON);
        touch(&root.join("a").join("new.jsonl"), T_NOON + 3_000);
        touch(&root.join("b").join("newer.jsonl"), T_NOON + 5_000);
        touch(&root.join("b").join("tie-b.jsonl"), T_NOON + 1_000);
        touch(&root.join("a").join("tie-a.JSONL"), T_NOON + 1_000);
        touch(&root.join("b").join("notes.json"), T_NOON + 9_000);
        touch(&root.join("b").join("notes.txt"), T_NOON + 9_000);
        std::fs::create_dir_all(root.join("b").join("dir.jsonl")).unwrap();

        let roots = std::slice::from_ref(&root);
        let all = find_recent(&e.reader, roots, T_NOON, 10);
        assert_eq!(names(&all), ["newer.jsonl", "new.jsonl", "tie-a.JSONL", "tie-b.jsonl"]);
        assert_eq!(all[0].modified_ms, T_NOON + 5_000);
        assert_eq!(all[0].len, 3);

        assert_eq!(names(&find_recent(&e.reader, roots, T_NOON, 2)), ["newer.jsonl", "new.jsonl"]);
        assert!(find_recent(&e.reader, roots, T_NOON, 0).is_empty());
        assert_eq!(find_recent(&e.reader, roots, T_NOON - 1, 10).len(), 5, "strictly newer");
        // Overlapping roots do not duplicate files.
        assert_eq!(find_recent(&e.reader, &[root.clone(), root.join("a")], T_NOON, 10).len(), 4);
    }

    #[test]
    fn find_recent_depth_limit_and_subagents() {
        let e = env();
        let root = e.paths.projects_dir();
        let mut deep = root.clone();
        for level in 1..MAX_WALK_DEPTH {
            deep = deep.join(format!("d{level}"));
        }
        touch(&deep.join("at-limit.jsonl"), T_NOON + 1);
        touch(&deep.join("d8").join("too-deep.jsonl"), T_NOON + 1);
        touch(&root.join("p").join("main.jsonl"), T_NOON + 2);
        touch(&root.join("p").join("sess").join("subagents").join("agent-1.jsonl"), T_NOON + 3);
        touch(&root.join("p").join("SubAgents").join("agent-2.jsonl"), T_NOON + 3);

        let found = find_recent(&e.reader, &[root], 0, 10);
        assert_eq!(names(&found), ["main.jsonl", "at-limit.jsonl"]);
    }

    #[test]
    fn find_recent_skips_denied_and_missing_dirs() {
        let e = env();
        let root = e.paths.projects_dir();
        touch(&root.join("p").join("ok.jsonl"), T_NOON + 1);
        touch(&root.join("p").join("Local Storage").join("x.jsonl"), T_NOON + 1);
        touch(&root.join("p").join("ant-secret.jsonl"), T_NOON + 1);
        let outside = e.tmp.path().join("elsewhere");
        touch(&outside.join("x.jsonl"), T_NOON + 1);
        let cowork = e.paths.desktop_roots()[0].join("local-agent-mode-sessions");
        touch(&cowork.join("a").join("b").join("c.jsonl"), T_NOON + 2);

        let roots = [root, outside, e.tmp.path().join("missing"), cowork];
        let found = find_recent(&e.reader, &roots, 0, 10);
        assert_eq!(names(&found), ["c.jsonl", "ok.jsonl"]);
    }
}
