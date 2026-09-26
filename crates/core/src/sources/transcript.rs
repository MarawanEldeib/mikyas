//! Claude Code transcripts (`*.jsonl`) → current model and context size.
//!
//! Locations: `<claude_home>/projects/<project>/<session>.jsonl` (terminal CLI: entrypoint `cli`;
//! Desktop Code tab: `claude-desktop`) and Cowork under
//! `<desktop_root>/local-agent-mode-sessions/**/.claude/projects/**/<session>.jsonl`
//! (entrypoint `local-agent`). Files under any directory named `subagents` are ignored.
//!
//! Assistant line shape (only these fields are read; message text is NEVER parsed or kept — of
//! `message.content` only the block `type` fields are read, see [`TurnInfo`]):
//! ```json
//! {"type":"assistant","isSidechain":false,"sessionId":"…","entrypoint":"cli","cwd":"C:\\…\\proj",
//!  "timestamp":"2026-09-24T12:00:00.000Z",
//!  "message":{"model":"claude-opus-5-5","stop_reason":"end_turn","usage":{"input_tokens":3,
//!   "cache_creation_input_tokens":120,"cache_read_input_tokens":42000,"output_tokens":800}}}
//! ```
//! Context tokens = `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` of the
//! LAST qualifying assistant line (not a sum). Qualifying: `type == "assistant"`, `isSidechain` not
//! true, `message.model` present and not `"<synthetic>"`, `message.usage` present. The largest
//! count seen only covers lines with the last line's `message.model`.
//!
//! 1M detection: the transcript head (first [`HEAD_BYTES`], or [`HEAD_MAX_BYTES`] when those hold
//! no identity) may contain an `attachment` line whose identity model id ends with `[1m]`, e.g.
//! `{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-opus-5-5[1m]"}}}`
//! — search tolerantly for any string value matching `claude-…[1m]` inside lines with
//! `"type":"attachment"`; the last such line in the head counts. `message.model` never has the
//! suffix, and the head is written once, so the marker only counts while the last line's model is
//! the identity's model (after `/model` switches to another one it no longer applies).
//!
//! Turns ([`TurnInfo`], see `crate::turns`): walking back from the end, the first non-sidechain
//! assistant line decides whether the tail ends with a finished turn (`stop_reason == "end_turn"`
//! and not `<synthetic>`). Its start is the last human user line before it in file order: a
//! `type: "user"` line, not sidechain, whose `message.content` is a string or holds a block whose
//! `type` is not `tool_result`. For those lines only `type`, `isSidechain`, `timestamp`,
//! `message.stop_reason` and the content block types are read.

use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::engine::types::Entrypoint;
use crate::model_names::split_1m;
use crate::saferead::SafeReader;
use crate::sources::SourceError;
use crate::sources::fsutil::{has_extension, lenient, walk_files};
use crate::time::{DAY_MS, Ms, json_time_to_ms, system_time_ms};
use crate::turns::TurnInfo;

/// Bytes read from the end of the file on the first attempt.
pub const TAIL_BYTES: u64 = 256 * 1024;
/// Retry size when no complete qualifying line fits in [`TAIL_BYTES`].
pub const TAIL_RETRY_BYTES: u64 = 1024 * 1024;
/// Bytes read from the start of the file to find the identity attachment.
pub const HEAD_BYTES: u64 = 64 * 1024;
/// Head size of the second attempt when [`HEAD_BYTES`] hold no identity (a huge first prompt).
pub const HEAD_MAX_BYTES: u64 = 512 * 1024;
/// Directory depth limit when walking roots.
pub const MAX_WALK_DEPTH: usize = 8;
/// A listed file whose directory-entry mtime is at most this much older than the cutoff is
/// re-checked against its own metadata (see [`current_stamp`]).
pub const LISTING_SLACK_MS: Ms = DAY_MS;

/// Model placeholder Claude Code writes for locally generated (non-API) messages.
const SYNTHETIC_MODEL: &str = "<synthetic>";
/// Object keys whose string value is the session's identity model id.
const IDENTITY_KEYS: &[&[u8]] = &[b"modelId", b"model_id"];

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptTail {
    pub path: PathBuf,
    pub session_id: String,
    pub entrypoint: Entrypoint,
    /// `message.model` of the last qualifying line (never has `[1m]`). [`scan_tail`] always sets
    /// it (a line without a model does not qualify); it stays an `Option` for tails built
    /// elsewhere (the app's tests, the engine's fallbacks), and readers treat `None` as unknown.
    pub model_id: Option<String>,
    /// Context tokens of the last qualifying line.
    pub ctx_tokens: u64,
    /// Largest context-token count among qualifying lines of `model_id` in the scanned tail.
    pub max_ctx_tokens_seen: u64,
    /// `Some(true)` if the head's identity attachment says `[1m]` for `model_id`'s model,
    /// `Some(false)` if an identity was found without it or for another model, `None` if no
    /// identity line was found.
    pub identity_1m: Option<bool>,
    /// `timestamp` of the last qualifying line.
    pub last_assistant_ms: Ms,
    /// Last path component of `cwd` (folder name only), if present.
    pub project: Option<String>,
    /// The latest turn, as far as the scanned tail shows it.
    pub turn: TurnInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFile {
    pub path: PathBuf,
    pub modified_ms: Ms,
    pub len: u64,
}

/// A head scan, cached per transcript by the caller: `None` until the head has been scanned.
pub type HeadIdentity = Option<HeadScan>;

/// What a head scan found, and how much of the file it covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadScan {
    /// The identity model id of the head (e.g. `claude-opus-5-5[1m]`), if one was found.
    pub model_id: Option<String>,
    /// File bytes the scan covered (the file length at the time, capped at [`HEAD_MAX_BYTES`]).
    pub scanned_len: u64,
}

impl HeadScan {
    /// The head is complete: an identity found in the first [`HEAD_BYTES`], or all of
    /// [`HEAD_MAX_BYTES`] scanned. Later writes cannot change the result.
    pub fn is_final(&self) -> bool {
        self.scanned_len >= HEAD_MAX_BYTES || (self.model_id.is_some() && self.scanned_len >= HEAD_BYTES)
    }

    /// True if the scan still describes a `len`-byte file: it is final or the file has not grown.
    pub fn covers(&self, len: u64) -> bool {
        self.is_final() || len <= self.scanned_len
    }
}

/// Scans the end of one transcript. Reads the last [`TAIL_BYTES`] (retrying with
/// [`TAIL_RETRY_BYTES`] if no qualifying line was found and the file is larger); drops the first
/// partial line by searching for `\n` in BYTES before UTF-8 decoding (the seek may split a
/// multi-byte char); tolerates a partial last line (Claude Code may be mid-write) and CRLF.
/// Walks lines from the end; a cheap substring check for `"assistant"` precedes JSON parsing.
///
/// `identity`: the previous head scan for this path, reused as-is (the head is not read again)
/// while it still covers the file ([`HeadScan::covers`]: the whole head was scanned, or the file
/// has not grown since). Otherwise, when a qualifying line exists, the head is scanned now and the
/// result stored there for the next call; so an identity line written after a scan (a `/model`
/// switch early in the session), or one that was missing, is still found. Either way
/// `identity_1m` is decided against the current model.
///
/// The retry reads only the bytes before the first chunk and prepends them.
///
/// Returns `Ok(None)` if no qualifying line exists. `session_id` falls back to the file stem and
/// `entrypoint` to `Unknown` when missing. A qualifying line without a usable `timestamp` falls
/// back to the file's modification time.
pub fn scan_tail(
    reader: &SafeReader,
    path: &Path,
    identity: &mut HeadIdentity,
) -> Result<Option<TranscriptTail>, SourceError> {
    let mut file = reader.open(path)?;
    let meta = file.metadata()?;
    let len = meta.len();

    let (mut chunk, mut chunk_start) = read_tail(&mut file, len, TAIL_BYTES)?;
    let mut scan = scan_chunk(&chunk, chunk_start > 0);
    if scan.is_none() && len > TAIL_BYTES {
        let start = tail_start(len, TAIL_RETRY_BYTES);
        let mut wider = read_range(&mut file, start, chunk_start - start)?;
        wider.extend_from_slice(&chunk);
        (chunk, chunk_start) = (wider, start);
        scan = scan_chunk(&chunk, chunk_start > 0);
    }
    let Some(TailScan { last, max_ctx_tokens, turn }) = scan else {
        return Ok(None);
    };

    if identity.as_ref().is_none_or(|head| !head.covers(len)) {
        *identity = Some(HeadScan {
            model_id: head_identity(&mut file, len, &chunk, chunk_start)?,
            scanned_len: len.min(HEAD_MAX_BYTES),
        });
    }
    let identity_1m =
        identity.as_ref().and_then(|found| found.model_id.as_deref()).map(|id| identity_is_1m_for(id, &last.model));

    let session_id = last
        .session_id
        .filter(|s| !s.is_empty())
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let last_assistant_ms = last.timestamp_ms.or_else(|| meta.modified().ok().and_then(system_time_ms)).unwrap_or(0);

    Ok(Some(TranscriptTail {
        path: path.to_path_buf(),
        session_id,
        entrypoint: last.entrypoint.as_deref().map_or(Entrypoint::Unknown, Entrypoint::from_transcript),
        model_id: Some(last.model),
        ctx_tokens: last.ctx_tokens,
        max_ctx_tokens_seen: max_ctx_tokens.max(last.ctx_tokens),
        identity_1m,
        last_assistant_ms,
        project: last.cwd.as_deref().and_then(project_name),
        turn,
    }))
}

/// Walks `roots` (recursively, at most [`MAX_WALK_DEPTH`] levels, skipping dirs named
/// `subagents`, listing only via `reader.read_dir`) and returns `*.jsonl` files modified after
/// `newer_than_ms`, newest first, at most `limit`. Unreadable dirs are skipped silently.
///
/// Depth: files directly inside a root are at level 1. Only files inside a directory named
/// `projects` (the root itself or one below it) count, matching the transcript locations in the
/// module docs, so e.g. a Cowork session's `.claude/history.jsonl` is ignored. Ties on
/// modification time are ordered by path; a file seen twice through overlapping roots is
/// returned once; files the reader would refuse to open are left out; symlinks are not followed.
/// Modification time and length come from the file's own metadata when the listing puts it
/// within [`LISTING_SLACK_MS`] of the cutoff or later (see [`current_stamp`]).
pub fn find_recent(reader: &SafeReader, roots: &[PathBuf], newer_than_ms: Ms, limit: usize) -> Vec<RecentFile> {
    if limit == 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let skip_subagents = |name: &OsStr| name.eq_ignore_ascii_case("subagents");
    let listing_floor = newer_than_ms.saturating_sub(LISTING_SLACK_MS);
    for root in roots {
        walk_files(reader, root, MAX_WALK_DEPTH, &skip_subagents, &mut |entry| {
            let path = entry.path();
            if !has_extension(&path, "jsonl") || !in_projects_dir(root, &path) {
                return;
            }
            let Ok(meta) = entry.metadata() else { return };
            let Some(listed_ms) = meta.modified().ok().and_then(system_time_ms) else {
                return;
            };
            if listed_ms <= listing_floor || !reader.allows(&path) {
                return;
            }
            let (modified_ms, len) = current_stamp(&path).unwrap_or((listed_ms, meta.len()));
            if modified_ms > newer_than_ms {
                found.push(RecentFile { path, modified_ms, len });
            }
        });
    }
    newest_first(found, limit)
}

/// The file's own modification time and length. On Windows a directory listing reports them from
/// the directory index, which NTFS updates lazily: while a writer holds the file open without
/// flushing, the listing can keep the stamp from before its writes (seen on Windows 11 until the
/// handle closed). Reading the file's metadata by path gets the current values; links are not
/// followed.
fn current_stamp(path: &Path) -> Option<(Ms, u64)> {
    let meta = std::fs::symlink_metadata(path).ok().filter(std::fs::Metadata::is_file)?;
    Some((meta.modified().ok().and_then(system_time_ms)?, meta.len()))
}

/// True if `path` (found below `root`) lies inside a directory named `projects` that is `root`
/// itself or below it, where Claude Code keeps transcripts (`<claude_home>/projects/…`, Cowork
/// `…/.claude/projects/…`). Other `.jsonl` files in the Cowork tree, such as a session's
/// `.claude/history.jsonl` prompt history, are not transcripts. Directories above `root` are not
/// considered, so the result never depends on where the user profile lives.
fn in_projects_dir(root: &Path, path: &Path) -> bool {
    let is_projects = |name: &OsStr| name.eq_ignore_ascii_case("projects");
    root.file_name().is_some_and(is_projects)
        || path
            .strip_prefix(root)
            .ok()
            .and_then(Path::parent)
            .is_some_and(|rel| rel.components().any(|c| is_projects(c.as_os_str())))
}

/// Sorts newest first (ties by path), keeps one entry per path and applies `limit`.
fn newest_first(mut found: Vec<RecentFile>, limit: usize) -> Vec<RecentFile> {
    // Overlapping roots list a file twice, with different mtimes if it was appended to between
    // the two listings: keep only the newest sighting of each path.
    found.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| b.modified_ms.cmp(&a.modified_ms)));
    found.dedup_by(|later, kept| later.path == kept.path);
    found.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.path.cmp(&b.path)));
    found.truncate(limit);
    found
}

// ---- tail scanning ----

/// The fields of one transcript line that the widget reads. Everything else — notably the text in
/// `message.content` — is skipped by serde without being materialised.
#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type", default, deserialize_with = "lenient")]
    kind: Option<String>,
    #[serde(rename = "isSidechain", default, deserialize_with = "lenient")]
    is_sidechain: Option<bool>,
    /// Claude Code's own notices written as user lines; never a turn start.
    #[serde(rename = "isMeta", default, deserialize_with = "lenient")]
    is_meta: Option<bool>,
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
    #[serde(default, deserialize_with = "lenient")]
    stop_reason: Option<String>,
    #[serde(default)]
    content: Option<ContentKind>,
}

/// What `message.content` holds, decided from the block `type` fields alone: string contents and
/// the other block fields are skipped without being kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    /// A string, or blocks of which at least one has a `type` other than `tool_result`: a person
    /// wrote it.
    Prompt,
    /// Only `tool_result` blocks (or none), or an unexpected shape.
    Other,
}

/// The `type` of one content block; blocks of any other shape have none.
struct BlockType(Option<String>);

/// A block's `type` value: a string is kept, anything else becomes `None`.
struct TypeValue(Option<String>);

/// Visitor methods that accept any JSON scalar as `$value`, so an unexpected shape never fails
/// the whole line.
macro_rules! accept_scalars {
    ($ty:ty, $value:expr) => {
        fn visit_bool<E>(self, _: bool) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_i64<E>(self, _: i64) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_u64<E>(self, _: u64) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_f64<E>(self, _: f64) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_unit<E>(self) -> Result<$ty, E> {
            Ok($value)
        }
    };
}

struct ContentVisitor;

impl<'de> Visitor<'de> for ContentVisitor {
    type Value = ContentKind;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("message content")
    }

    fn visit_str<E>(self, _: &str) -> Result<ContentKind, E> {
        Ok(ContentKind::Prompt)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ContentKind, A::Error> {
        let mut prompt = false;
        while let Some(BlockType(kind)) = seq.next_element()? {
            prompt |= kind.is_some_and(|k| k != "tool_result");
        }
        Ok(if prompt { ContentKind::Prompt } else { ContentKind::Other })
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<ContentKind, A::Error> {
        IgnoredAny.visit_map(map).map(|_| ContentKind::Other)
    }

    accept_scalars!(ContentKind, ContentKind::Other);
}

impl<'de> Deserialize<'de> for ContentKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ContentVisitor)
    }
}

struct BlockVisitor;

impl<'de> Visitor<'de> for BlockVisitor {
    type Value = BlockType;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a content block")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<BlockType, A::Error> {
        let mut kind = None;
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            if key == "type" && kind.is_none() {
                kind = map.next_value::<TypeValue>()?.0;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(BlockType(kind))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<BlockType, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| BlockType(None))
    }

    fn visit_str<E>(self, _: &str) -> Result<BlockType, E> {
        Ok(BlockType(None))
    }

    accept_scalars!(BlockType, BlockType(None));
}

impl<'de> Deserialize<'de> for BlockType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(BlockVisitor)
    }
}

struct TypeValueVisitor;

impl<'de> Visitor<'de> for TypeValueVisitor {
    type Value = TypeValue;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a block type")
    }

    fn visit_str<E>(self, v: &str) -> Result<TypeValue, E> {
        Ok(TypeValue(Some(v.to_owned())))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<TypeValue, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| TypeValue(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<TypeValue, A::Error> {
        IgnoredAny.visit_map(map).map(|_| TypeValue(None))
    }

    accept_scalars!(TypeValue, TypeValue(None));
}

impl<'de> Deserialize<'de> for TypeValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(TypeValueVisitor)
    }
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
    turn: TurnInfo,
}

/// Builds [`TurnInfo`] from lines fed newest first (see the module docs).
#[derive(Default)]
struct TurnScan {
    /// The deciding assistant line was seen: `Some(end)` when it finished a turn.
    decided: Option<Option<Ms>>,
    /// Timestamp of the prompt that started the finished turn.
    prompt_ms: Option<Ms>,
    /// No more lines are needed.
    done: bool,
    earliest_ms: Option<Ms>,
}

impl TurnScan {
    fn feed(&mut self, line: &RawLine) {
        let ts = line.timestamp.as_ref().and_then(json_time_to_ms);
        if let Some(ts) = ts {
            self.earliest_ms = Some(self.earliest_ms.map_or(ts, |e| e.min(ts)));
        }
        if line.is_sidechain == Some(true) {
            return;
        }
        let message = line.message.as_ref();
        match (line.kind.as_deref(), self.decided) {
            (Some("assistant"), None) => {
                let finished = message.is_some_and(|m| {
                    m.stop_reason.as_deref() == Some("end_turn") && m.model.as_deref() != Some(SYNTHETIC_MODEL)
                });
                let end = if finished { ts } else { None };
                self.decided = Some(end);
                self.done = end.is_none();
            }
            (Some("user"), Some(Some(_)))
                if line.is_meta != Some(true) && message.and_then(|m| m.content) == Some(ContentKind::Prompt) =>
            {
                self.prompt_ms = ts;
                self.done = true;
            }
            _ => {}
        }
    }

    fn finish(self) -> TurnInfo {
        let Some(Some(end)) = self.decided else {
            return TurnInfo::default();
        };
        let lower_bound = self.prompt_ms.is_none() && !self.done;
        TurnInfo {
            ended_ms: Some(end),
            started_ms: if lower_bound { self.earliest_ms } else { self.prompt_ms },
            start_is_lower_bound: lower_bound,
        }
    }
}

fn qualify(line: RawLine) -> Option<AssistantLine> {
    if line.kind.as_deref() != Some("assistant") || line.is_sidechain == Some(true) {
        return None;
    }
    let message = line.message?;
    let model = message.model.filter(|m| !m.is_empty() && m != SYNTHETIC_MODEL)?;
    let usage = message.usage?;
    let ctx_tokens = [usage.input_tokens, usage.cache_creation_input_tokens, usage.cache_read_input_tokens]
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
    let mut last: Option<AssistantLine> = None;
    let mut max_ctx_tokens = 0;
    let mut turn = TurnScan::default();
    for raw in body.rsplit(|&b| b == b'\n') {
        let line = raw.trim_ascii(); // also removes the `\r` of CRLF files
        // User lines matter only until the turn's prompt is found.
        let wanted = contains(line, br#""assistant""#) || (!turn.done && contains(line, br#""user""#));
        if line.first() != Some(&b'{') || !wanted {
            continue;
        }
        // A partial (mid-write) or otherwise malformed line simply fails to parse.
        let Ok(parsed) = serde_json::from_slice::<RawLine>(line) else { continue };
        if !turn.done {
            turn.feed(&parsed);
        }
        let Some(assistant) = qualify(parsed) else { continue };
        // Lines of a model used before a `/model` switch say nothing about the current window.
        if last.as_ref().is_none_or(|l| l.model == assistant.model) {
            max_ctx_tokens = max_ctx_tokens.max(assistant.ctx_tokens);
        }
        if last.is_none() {
            last = Some(assistant);
        }
    }
    let turn = turn.finish();
    last.map(|last| TailScan { last, max_ctx_tokens, turn })
}

/// Reads the last `n` bytes of a `len`-byte file. When the read starts mid-file it includes one
/// extra preceding byte, so a chunk that happens to start exactly on a line boundary keeps that
/// line (the extra `\n` is what gets dropped). Returns the bytes and their start offset.
fn read_tail(file: &mut File, len: u64, n: u64) -> Result<(Vec<u8>, u64), SourceError> {
    let start = tail_start(len, n);
    Ok((read_range(file, start, len - start)?, start))
}

/// Where [`read_tail`] starts reading the last `n` bytes of a `len`-byte file.
fn tail_start(len: u64, n: u64) -> u64 {
    if len > n { len - n - 1 } else { 0 }
}

/// Reads up to `n` bytes at `start`; a file that shrank meanwhile just yields fewer bytes.
fn read_range(file: &mut File, start: u64, n: u64) -> Result<Vec<u8>, SourceError> {
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    file.take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Folder name of a Windows or POSIX style `cwd` (no drive or root on its own).
fn project_name(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    let name = trimmed.rsplit(['\\', '/']).next()?;
    if name.is_empty() || name.ends_with(':') { None } else { Some(name.to_string()) }
}

// ---- head (identity) scanning ----

/// The identity model id in the first [`HEAD_BYTES`], or in the first [`HEAD_MAX_BYTES`] if those
/// hold none and the file is longer. `chunk` is the tail chunk read at `chunk_start`; when that is
/// the start of the file it already holds the head.
fn head_identity(file: &mut File, len: u64, chunk: &[u8], chunk_start: u64) -> Result<Option<String>, SourceError> {
    let mut scanned = 0;
    for size in [HEAD_BYTES, HEAD_MAX_BYTES].map(|n| n.min(len)) {
        if size == scanned {
            break;
        }
        scanned = size;
        let found = if chunk_start == 0 {
            scan_head(&chunk[..chunk.len().min(size as usize)])
        } else {
            scan_head(&read_range(file, 0, size)?)
        };
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Looks for the identity attachment in the first bytes of a transcript: the model id of the last
/// attachment line that has one (a later identity line describes a newer choice), its `[1m]` value
/// preferred over a plain identity in the same line. The last line may be cut off at the head
/// boundary; the scan tolerates that.
fn scan_head(head: &[u8]) -> Option<String> {
    let mut found = None;
    for line in head.split(|&b| b == b'\n') {
        if !contains(line, b"attachment") {
            continue;
        }
        let scan = scan_attachment_line(line);
        if scan.is_attachment {
            found = scan.one_m.or(scan.identity).or(found);
        }
    }
    found.map(|id| String::from_utf8_lossy(id).into_owned())
}

/// True if the identity model id has `[1m]` and names the same model as `model` (a
/// `message.model`, never suffixed): equal once a trailing `-YYYYMMDD` is ignored, or an alias
/// such as `opus` that is one of the parts of `model`.
fn identity_is_1m_for(identity: &str, model: &str) -> bool {
    let (base, one_m) = split_1m(identity.trim());
    let (base, model) = (without_date(base), without_date(model.trim()));
    let is_alias = !base.is_empty() && base.bytes().all(|b| b.is_ascii_alphabetic());
    one_m
        && (base.eq_ignore_ascii_case(model)
            || (is_alias && model.split('-').skip(1).any(|part| part.eq_ignore_ascii_case(base))))
}

/// `id` without a trailing `-YYYYMMDD` snapshot date.
fn without_date(id: &str) -> &str {
    match id.rsplit_once('-') {
        Some((rest, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => id,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AttachmentScan<'a> {
    /// The top-level `type` is `attachment`, or — for a line cut off before its `type` key — it
    /// has a top-level `attachment` key.
    is_attachment: bool,
    /// The first model-id-like `modelId` string value.
    identity: Option<&'a [u8]>,
    /// The first `claude-…[1m]` string value (or model-id-like `modelId` ending in `[1m]`).
    one_m: Option<&'a [u8]>,
}

/// A tiny JSON tokenizer over one (possibly truncated) line: it tracks nesting depth and string
/// literals, so text that merely mentions a model id inside a longer string never matches, and
/// key order or whitespace do not matter.
fn scan_attachment_line(line: &[u8]) -> AttachmentScan<'_> {
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
                let is_identity = key.is_some_and(|k| IDENTITY_KEYS.contains(&k)) && is_model_id_like(text);
                if is_identity {
                    out.identity.get_or_insert(text);
                }
                if ends_with_1m(text) && (is_identity || (text.starts_with(b"claude-") && is_model_id_like(text))) {
                    out.one_m.get_or_insert(text);
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
    use std::time::{Duration, UNIX_EPOCH};

    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::paths::Paths;
    use crate::saferead::ReadError;

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
            scan_tail(&self.reader, path, &mut None).unwrap()
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
                turn: TurnInfo::default(),
            }
        );
    }

    #[test]
    fn last_line_wins_and_max_is_tracked() {
        let e = env();
        let first = assistant("claude-sonnet-5", 10, 0, 250_000);
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
        assert!(matches!(scan_tail(&e.reader, &missing, &mut None), Err(SourceError::NotFound)));
        let outside = e.tmp.path().join("elsewhere").join("s.jsonl");
        write_file(&outside, &lines(&[assistant("claude-opus-5-5", 1, 1, 1)]));
        assert!(matches!(scan_tail(&e.reader, &outside, &mut None), Err(SourceError::Read(ReadError::Denied(_)))));
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
        let body = [user("hi").to_string(), assistant("claude-opus-5-5", 7, 0, 0).to_string(), user("bye").to_string()]
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
        // The line straddling the seek point (a crashed write continued without a newline) ends
        // with text that parses as a qualifying assistant line if the partial first line were
        // not dropped. `extra == 1` makes the chunk (including its one extra byte) start exactly
        // at the decoy's `{`, so only the partial-line drop keeps it out.
        for extra in [0, 1] {
            let mut decoy = assistant("claude-opus-5-5", 999_999, 0, 0);
            let base_len = decoy.to_string().len() + 1;
            let pad = TAIL_BYTES as usize + extra - good.len() - base_len - r#","pad":"""#.len();
            decoy.as_object_mut().unwrap().insert("pad".into(), json!("y".repeat(pad)));
            let decoy = lines(&[decoy]);
            assert_eq!(decoy.len() + good.len(), TAIL_BYTES as usize + extra);

            let mut bytes = filler_prefix(10_000);
            bytes.extend_from_slice(&decoy);
            bytes.extend_from_slice(&good);
            let path = e.write(&format!("p/s{extra}.jsonl"), &bytes);
            assert_eq!(bytes[bytes.len() - TAIL_BYTES as usize - 1] == b'{', extra == 1);
            let tail = e.scan(&path).unwrap();
            assert_eq!((tail.ctx_tokens, tail.max_ctx_tokens_seen), (1_000, 1_000), "extra={extra}");
        }
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
                lines(&[json!({"type": "attachment", "attachment": {"model": "claude-opus-5-5[1m]"}}), a.clone()]),
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
    fn identity_after_a_huge_first_prompt() {
        let e = env();
        // Up to 512 KiB of head are searched when the first 64 KiB hold no identity line, both when
        // the tail chunk already covers the head and when the head is read separately.
        let prompt = |kib: usize| user(&"p".repeat(kib * 1024));
        let ident = identity_line("claude-opus-5-5[1m]");
        let a = assistant("claude-opus-5-5", 1, 1, 1);
        let filler = user(&"f".repeat(TAIL_RETRY_BYTES as usize / 2));
        let cases = [
            ("in_tail_chunk", lines(&[prompt(100), ident.clone(), a.clone()]), Some(true)),
            ("read_separately", lines(&[prompt(400), ident.clone(), filler.clone(), a.clone()]), Some(true)),
            ("beyond_512k", lines(&[prompt(520), ident.clone(), filler, a.clone()]), None),
        ];
        for (name, bytes, expected) in cases {
            let path = e.write(&format!("p/{name}.jsonl"), &bytes);
            assert_eq!(e.scan(&path).unwrap().identity_1m, expected, "{name}");
        }
    }

    #[test]
    fn head_identity_is_the_last_one() {
        let ids = |ids: &[&str]| lines(&ids.iter().map(|id| identity_line(id)).collect::<Vec<_>>());
        // A later identity line (a `/model` switch early in the session) replaces an earlier one.
        let head = ids(&["claude-sonnet-5[1m]", "claude-opus-5-5[1m]", "claude-opus-5-5"]);
        assert_eq!(scan_head(&head).as_deref(), Some("claude-opus-5-5"));
        let head = ids(&["claude-opus-5-5", "claude-sonnet-5[1m]"]);
        assert_eq!(scan_head(&head).as_deref(), Some("claude-sonnet-5[1m]"));
        // Within one line a 1M value still wins over a plain identity.
        let mixed = json!({"type": "attachment", "a": {"modelId": "claude-opus-5-5", "b": "claude-sonnet-5[1m]"}});
        let head = lines(&[identity_line("claude-opus-5-5"), mixed, user("hi")]);
        assert_eq!(scan_head(&head).as_deref(), Some("claude-sonnet-5[1m]"));
        assert_eq!(scan_head(&lines(&[user("hi")])), None);

        // End to end: the switch from the 1M to the 200K variant of the same model is visible
        // while both identity lines are in the head.
        let e = env();
        let path = e.write(
            "p/variant.jsonl",
            &lines(&[
                identity_line("claude-opus-5-5[1m]"),
                assistant("claude-opus-5-5", 1, 1, 1),
                identity_line("claude-opus-5-5"),
                assistant("claude-opus-5-5", 2, 2, 2),
            ]),
        );
        assert_eq!(e.scan(&path).unwrap().identity_1m, Some(false));
    }

    #[test]
    fn identity_applies_only_to_its_model() {
        let e = env();
        // `/model` switched away from the 1M model named by the identity attachment.
        let cases = [
            ("claude-opus-5-5[1m]", "claude-opus-5-5", Some(true)),
            ("claude-opus-5-5[1m]", "claude-sonnet-5", Some(false)),
            ("claude-opus-5-5", "claude-opus-5-5", Some(false)),
            // A dated API model id is the same model; an alias matches its family.
            ("claude-sonnet-4-5[1m]", "claude-sonnet-4-5-20250929", Some(true)),
            ("opus[1m]", "claude-opus-5-5", Some(true)),
            ("opus[1m]", "claude-sonnet-5", Some(false)),
        ];
        for (i, (identity, model, expected)) in cases.into_iter().enumerate() {
            let before = assistant("claude-opus-5-5", 1, 1, 1);
            let path = e.write(
                &format!("p/switch{i}.jsonl"),
                &lines(&[identity_line(identity), before, assistant(model, 2, 2, 2)]),
            );
            assert_eq!(e.scan(&path).unwrap().identity_1m, expected, "{identity} vs {model}");
        }
    }

    #[test]
    fn max_ctx_counts_only_the_last_lines_model() {
        let e = env();
        let path = e.write(
            "p/a.jsonl",
            &lines(&[assistant("claude-opus-5-5", 0, 0, 300_000), assistant("claude-sonnet-5", 0, 0, 20_000)]),
        );
        let tail = e.scan(&path).unwrap();
        assert_eq!((tail.ctx_tokens, tail.max_ctx_tokens_seen), (20_000, 20_000));
        let path = e.write(
            "p/b.jsonl",
            &lines(&[
                assistant("claude-sonnet-5", 0, 0, 250_000),
                assistant("claude-opus-5-5", 0, 0, 400_000),
                assistant("claude-sonnet-5", 0, 0, 20_000),
            ]),
        );
        assert_eq!(e.scan(&path).unwrap().max_ctx_tokens_seen, 250_000);
    }

    #[test]
    fn model_switch_away_from_1m_resolves_to_the_lower_rules() {
        use crate::engine::context::{self, ContextInputs, DEFAULT_CTX, ONE_M_CTX};
        use crate::engine::types::CtxBasis;
        let e = env();
        let resolve = |tail: &TranscriptTail| {
            let r = context::resolve(&ContextInputs {
                tail: Some(tail),
                capture: None,
                desktop_session: None,
                overrides: &Default::default(),
            });
            (r.size, r.basis)
        };
        let mut body = lines(&[identity_line("claude-opus-5-5[1m]"), assistant("claude-opus-5-5", 0, 0, 300_000)]);
        let path = e.write("p/s.jsonl", &body);
        assert_eq!(resolve(&e.scan(&path).unwrap()), (ONE_M_CTX, CtxBasis::Identity));
        body.extend(lines(&[assistant("claude-sonnet-5", 0, 0, 20_000)]));
        let path = e.write("p/s.jsonl", &body);
        assert_eq!(resolve(&e.scan(&path).unwrap()), (DEFAULT_CTX, CtxBasis::Default));
    }

    #[test]
    fn find_recent_reads_the_current_stamp_of_a_file_held_open() {
        // NTFS answers directory listings from the directory index, which lags behind a file that a
        // writer holds open without flushing: on Windows 11 the listing kept the pre-write size and
        // mtime until the handle closed. Where listings are current this passes trivially.
        let e = env();
        let now = crate::time::now_ms();
        let path = e.write("p/open.jsonl", b"{}\n");
        set_mtime(&path, now - 2 * 3_600_000);
        let mut writer = File::options().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut writer, b"{\"type\":\"user\"}\n").unwrap();
        let found = find_recent(&e.reader, &[e.paths.projects_dir()], now - 3_600_000, 10);
        drop(writer);
        assert_eq!(names(&found), ["open.jsonl"]);
        assert_eq!(found[0].len, 19);
        assert!(found[0].modified_ms > now - 3_600_000);
    }

    #[test]
    fn cached_identity_skips_head_scan() {
        let e = env();
        let path = e
            .write("p/s.jsonl", &lines(&[identity_line("claude-opus-5-5[1m]"), assistant("claude-opus-5-5", 1, 1, 1)]));
        let len = std::fs::metadata(&path).unwrap().len();
        let scan = |cached: HeadIdentity| {
            let mut identity = cached;
            let one_m = scan_tail(&e.reader, &path, &mut identity).unwrap().unwrap().identity_1m;
            (one_m, identity)
        };
        let head = |id: Option<&str>| Some(HeadScan { model_id: id.map(str::to_string), scanned_len: len });
        let found = head(Some("claude-opus-5-5[1m]"));
        assert_eq!(scan(None), (Some(true), found.clone()), "the head scan is handed back");
        let plain = head(Some("claude-opus-5-5"));
        assert_eq!(scan(plain.clone()), (Some(false), plain), "cached value is used as-is");
        assert_eq!(scan(head(None)), (None, head(None)));
        assert_eq!(scan(found.clone()), (Some(true), found));
        // The cached identity is still compared with the current model.
        let other = head(Some("claude-sonnet-5[1m]"));
        assert_eq!(scan(other.clone()), (Some(false), other));
    }

    #[test]
    fn cached_head_is_rescanned_while_the_head_grows() {
        let e = env();
        let first = lines(&[identity_line("claude-opus-5-5"), assistant("claude-opus-5-5", 1, 1, 1)]);
        let path = e.write("p/s.jsonl", &first);
        let mut identity = None;
        let tail = scan_tail(&e.reader, &path, &mut identity).unwrap().unwrap();
        assert_eq!(tail.identity_1m, Some(false));
        // `/model` early in the session: a second identity line lands in the head.
        let mut grown = first.clone();
        grown.extend(lines(&[identity_line("claude-opus-5-5[1m]"), assistant("claude-opus-5-5", 2, 2, 2)]));
        write_file(&path, &grown);
        let tail = scan_tail(&e.reader, &path, &mut identity).unwrap().unwrap();
        assert_eq!(tail.identity_1m, Some(true), "the newer identity line is seen");
        assert_eq!(identity.as_ref().map(|h| h.scanned_len), Some(grown.len() as u64));
    }

    #[test]
    fn missing_identity_is_retried_once_the_file_grows() {
        let e = env();
        let first = lines(&[user("hi"), assistant("claude-opus-5-5", 1, 1, 1)]);
        let path = e.write("p/s.jsonl", &first);
        let mut identity = None;
        assert_eq!(scan_tail(&e.reader, &path, &mut identity).unwrap().unwrap().identity_1m, None);
        assert_eq!(identity.as_ref().map(|h| h.model_id.clone()), Some(None));
        let mut grown = first.clone();
        grown.extend(lines(&[identity_line("claude-opus-5-5[1m]"), assistant("claude-opus-5-5", 2, 2, 2)]));
        write_file(&path, &grown);
        assert_eq!(scan_tail(&e.reader, &path, &mut identity).unwrap().unwrap().identity_1m, Some(true));
    }

    #[test]
    fn a_complete_head_scan_is_final() {
        let done =
            |model_id: Option<&str>, scanned_len: u64| HeadScan { model_id: model_id.map(str::to_string), scanned_len };
        assert!(done(None, HEAD_MAX_BYTES).is_final());
        assert!(done(Some("x"), HEAD_BYTES).is_final());
        assert!(!done(Some("x"), HEAD_BYTES - 1).is_final());
        assert!(!done(None, HEAD_BYTES).is_final());
        assert!(done(None, 10).covers(10) && !done(None, 10).covers(11));
        assert!(done(None, HEAD_MAX_BYTES).covers(u64::MAX));
    }

    #[test]
    fn attachment_tokenizer() {
        fn scan(s: &str) -> AttachmentScan<'_> {
            scan_attachment_line(s.as_bytes())
        }
        let id = b"claude-opus-5-5[1m]".as_slice();
        assert_eq!(
            scan(r#"{"type":"attachment","attachment":{"identity":{"modelId":"claude-opus-5-5[1m]"}}}"#),
            AttachmentScan { is_attachment: true, identity: Some(id), one_m: Some(id) }
        );
        // A 1M value under another key; a plain identity next to it.
        let s = scan(r#"{"type":"attachment","a":{"modelId":"claude-opus-5-5","b":"claude-sonnet-5[1m]"}}"#);
        assert_eq!(s.identity, Some(b"claude-opus-5-5".as_slice()));
        assert_eq!(s.one_m, Some(b"claude-sonnet-5[1m]".as_slice()));
        // Nested "type":"attachment" is not a top-level type, and an explicit other type wins
        // over an `attachment` key.
        assert!(!scan(r#"{"type":"user","x":{"type":"attachment"}}"#).is_attachment);
        assert!(!scan(r#"{"attachment":{"modelId":"claude-opus-5-5[1m]"},"type":"user"}"#).is_attachment);
        assert!(scan(r#"{"attachment":{"modelId":"claude-opus-5-5[1m]"},"type":"attachment"}"#).is_attachment);
        // Truncated inside a string: what was seen so far still counts.
        let s = scan(r#"{"attachment":{"identity":{"modelId":"claude-opus-5-5[1m]","text":"unterminated"#);
        assert!(s.is_attachment && s.one_m == Some(id));
        // Escaped quotes do not end a string early.
        let s = scan(r#"{"type":"attachment","a":"say \"claude-opus-5-5[1m]\" twice"}"#);
        assert!(s.is_attachment && s.one_m.is_none());
        // Aliases in modelId count too; free text in modelId does not.
        let s = scan(r#"{"type":"attachment","identity":{"modelId":"opus[1M]"}}"#);
        assert_eq!(s.one_m, Some(b"opus[1M]".as_slice()));
        assert_eq!(
            scan(r#"{"type":"attachment","modelId":"not a model [1m]"}"#),
            AttachmentScan { is_attachment: true, ..AttachmentScan::default() }
        );
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
        files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect()
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
        let overlapping = [root.clone(), e.paths.claude_home().to_path_buf()];
        assert_eq!(names(&find_recent(&e.reader, &overlapping, T_NOON, 10)), names(&all));
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
        touch(&cowork.join("a").join(".claude").join("projects").join("b").join("c.jsonl"), T_NOON + 2);

        let roots = [root, outside, e.tmp.path().join("missing"), cowork];
        let found = find_recent(&e.reader, &roots, 0, 10);
        assert_eq!(names(&found), ["c.jsonl", "ok.jsonl"]);
    }

    #[test]
    fn find_recent_ignores_jsonl_outside_projects_dirs() {
        // A Cowork session keeps a whole Claude Code config dir: its `.claude/history.jsonl`
        // (prompt history) and other logs are not transcripts and must not be returned.
        // A `projects` folder ABOVE the root (e.g. in the profile path) must not disable the check.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("projects");
        let paths = Paths::with_roots(base.join(".claude"), vec![base.join("Claude")], base.join("data"));
        let reader = SafeReader::new(&paths);
        let cowork = paths.desktop_roots()[0].join("local-agent-mode-sessions");
        let session = cowork.join("acct").join("org").join("sess");
        touch(&session.join(".claude").join("projects").join("p").join("t.jsonl"), T_NOON + 1);
        touch(&session.join(".claude").join("history.jsonl"), T_NOON + 5);
        touch(&session.join("audit.jsonl"), T_NOON + 5);
        let found = find_recent(&reader, &[cowork], 0, 10);
        assert_eq!(names(&found), ["t.jsonl"]);
    }

    #[test]
    fn duplicate_sightings_keep_only_the_newest() {
        // Overlapping roots list a file twice; if it was appended to between the two listings
        // the sightings have different mtimes and are not adjacent after sorting.
        let rf = |name: &str, modified_ms: Ms| RecentFile { path: PathBuf::from(name), modified_ms, len: 1 };
        let found = newest_first(vec![rf("x", 30), rf("y", 20), rf("x", 10), rf("y", 20)], 10);
        assert_eq!(found, vec![rf("x", 30), rf("y", 20)]);
        assert_eq!(newest_first(vec![rf("x", 10), rf("y", 20), rf("x", 30)], 1), vec![rf("x", 30)]);
    }

    #[test]
    fn deeply_nested_message_content_is_skipped_not_fatal() {
        let e = env();
        let mut v = assistant("claude-opus-5-5", 4_242, 0, 0);
        let depth = 1_000;
        let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        v["message"]["content"] = json!("__NESTED__");
        let line = v.to_string().replace("\"__NESTED__\"", &nested);
        assert!(serde_json::from_str::<serde_json::Value>(&line).is_err(), "deeper than serde's limit");
        let path = e.write("p/s.jsonl", format!("{line}\n").as_bytes());
        assert_eq!(e.scan(&path).map(|t| t.ctx_tokens), Some(4_242));
    }

    // ---- turns ----

    /// `v` with its timestamp `secs` after noon.
    fn at(v: serde_json::Value, secs: i64) -> serde_json::Value {
        let ts = chrono::DateTime::from_timestamp_millis(T_NOON + secs * 1000).unwrap();
        with(v, "/timestamp", json!(ts.to_rfc3339()))
    }

    fn working(secs: i64) -> serde_json::Value {
        let v = with(assistant("claude-opus-5-5", 1, 0, 0), "/message/stop_reason", json!("tool_use"));
        at(with(v, "/message/content", json!([{"type": "tool_use", "name": "Bash", "input": {}}])), secs)
    }

    fn done(secs: i64) -> serde_json::Value {
        at(assistant("claude-opus-5-5", 1, 0, 0), secs)
    }

    fn tool_result(secs: i64) -> serde_json::Value {
        let blocks = json!([{"type": "tool_result", "tool_use_id": "toolu_01", "content": "ok"}]);
        at(with(user("x"), "/message/content", blocks), secs)
    }

    fn prompt(secs: i64) -> serde_json::Value {
        at(user("please do the thing"), secs)
    }

    fn turn_of(e: &Env, name: &str, values: &[serde_json::Value]) -> TurnInfo {
        let path = e.write(&format!("p/{name}.jsonl"), &lines(values));
        e.scan(&path).unwrap().turn
    }

    fn finished(start_secs: i64, end_secs: i64, lower_bound: bool) -> TurnInfo {
        TurnInfo {
            ended_ms: Some(T_NOON + end_secs * 1000),
            started_ms: Some(T_NOON + start_secs * 1000),
            start_is_lower_bound: lower_bound,
        }
    }

    #[test]
    fn finished_turn_spans_prompt_to_end() {
        let e = env();
        let values = [
            prompt(-600),
            done(-590),
            prompt(0),
            working(10),
            tool_result(20),
            working(30),
            tool_result(40),
            done(700),
            done(720), // a message split into several lines, all saying end_turn
        ];
        assert_eq!(turn_of(&e, "done", &values), finished(0, 720, false));
    }

    #[test]
    fn unfinished_turns_have_no_end() {
        let e = env();
        let running = [prompt(0), working(10), tool_result(20)];
        assert_eq!(turn_of(&e, "running", &running), TurnInfo::default());
        let synthetic = [done(-5), prompt(0), with(done(10), "/message/model", json!("<synthetic>"))];
        assert_eq!(turn_of(&e, "synthetic", &synthetic), TurnInfo::default());
        let stopped = [prompt(0), with(done(10), "/message/stop_reason", json!("max_tokens"))];
        assert_eq!(turn_of(&e, "stopped", &stopped), TurnInfo::default());
    }

    #[test]
    fn sidechain_lines_never_end_or_start_a_turn() {
        let e = env();
        let side = |v: serde_json::Value| with(v, "/isSidechain", json!(true));
        // A subagent's end_turn after the main agent's tool call: the main turn is still running.
        let running = [prompt(0), working(10), side(prompt(11)), side(done(50))];
        assert_eq!(turn_of(&e, "sub_running", &running), TurnInfo::default());
        // A subagent's prompt inside a finished turn does not move its start.
        let values = [prompt(0), working(10), side(prompt(11)), side(done(50)), tool_result(51), done(300)];
        assert_eq!(turn_of(&e, "sub_done", &values), finished(0, 300, false));
    }

    #[test]
    fn prompts_with_text_blocks_count_but_tool_results_do_not() {
        let e = env();
        let blocks = json!([{"type": "image", "source": {}}, {"type": "text", "text": "look at this"}]);
        let with_image = at(with(user("x"), "/message/content", blocks), 0);
        let mixed = at(
            with(
                user("x"),
                "/message/content",
                json!([{"type": "tool_result", "content": "ok"}, {"type": "text", "text": "and stop"}]),
            ),
            100,
        );
        let values = [with_image.clone(), working(10), tool_result(20), done(200)];
        assert_eq!(turn_of(&e, "image", &values), finished(0, 200, false));
        let values = [with_image, working(10), mixed, done(200)];
        assert_eq!(turn_of(&e, "mixed", &values), finished(100, 200, false));
    }

    #[test]
    fn prompt_after_the_end_is_not_the_start() {
        let e = env();
        // The user typed the next prompt right after the finish (not answered yet).
        let values = [prompt(0), working(10), done(400), prompt(410)];
        assert_eq!(turn_of(&e, "next", &values), finished(0, 400, false));
    }

    #[test]
    fn prompt_outside_the_tail_gives_a_lower_bound() {
        let e = env();
        // No prompt at all: the earliest timestamp in the tail bounds the start.
        let values = [tool_result(5), working(10), tool_result(20), done(600)];
        assert_eq!(turn_of(&e, "no_prompt", &values), finished(5, 600, true));

        // The prompt was pushed out of the scanned tail by a long tool result.
        let mut long = tool_result(30);
        long["message"]["content"][0]["content"] = json!("r".repeat(TAIL_BYTES as usize));
        let values = [prompt(0), working(10), long, working(40), tool_result(50), done(900)];
        assert_eq!(turn_of(&e, "pushed_out", &values), finished(40, 900, true));
    }

    #[test]
    fn odd_content_shapes_do_not_break_the_scan() {
        let e = env();
        let odd = |content: serde_json::Value, secs: i64| at(with(user("x"), "/message/content", content), secs);
        let values = [
            prompt(0),
            working(10),
            odd(json!(null), 20),
            odd(json!(42), 21),
            odd(json!({"type": "text"}), 22),
            odd(json!(["text", 1, null, [{"type": "text"}], {"type": 7}, {"type": "tool_result"}]), 23),
            odd(json!([]), 24),
            done(300),
        ];
        // None of the odd lines is a prompt (a block needs a string `type`), and none fails the scan.
        assert_eq!(turn_of(&e, "odd", &values), finished(0, 300, false));
    }

    #[test]
    fn a_text_user_line_mid_turn_moves_the_start() {
        let e = env();
        // `isMeta` notices inside a turn are Claude Code's own lines: the turn still starts at the
        // prompt. A message the person types while Claude works does move the start (the duration
        // is then shorter than the real turn, never longer).
        let mut meta = prompt(100);
        meta["isMeta"] = json!(true);
        let values = [prompt(0), working(10), tool_result(20), meta, working(110), done(400)];
        assert_eq!(turn_of(&e, "meta", &values), finished(0, 400, false));
        let typed = [prompt(0), working(10), tool_result(20), prompt(100), working(110), done(400)];
        assert_eq!(turn_of(&e, "typed", &typed), finished(100, 400, false));
    }

    #[test]
    fn turns_in_crlf_files() {
        let e = env();
        let body = [prompt(0), working(10), tool_result(20), done(200)]
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n";
        let path = e.write("p/crlf.jsonl", body.as_bytes());
        assert_eq!(e.scan(&path).unwrap().turn, finished(0, 200, false));
    }
}
