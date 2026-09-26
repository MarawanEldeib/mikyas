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
//!   field, or resetting before 2001 or after 2200, is skipped. `used_percentage` is clamped to
//!   0..=100; non-finite values are skipped.
//! - `cost.total_api_duration_ms` (only used to detect a real new API response)
//! - `version` (Claude Code version)
//!
//! [`read_capture`] applies the same rules again, so a hand-edited file cannot bring back what
//! extraction drops.
//!
//! Freshness: `refreshInterval` re-runs the statusline with the SAME cached data, so
//! `changed_at_ms` must only advance when the [`fingerprint`] changes. [`write_capture`] carries
//! `changed_at_ms` over from the existing file when the fingerprint is unchanged, and skips the
//! write entirely when the fingerprint is unchanged and the existing file's `written_at_ms` is
//! less than [`REWRITE_AFTER_MS`] old.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fingerprint::Fnv64;
use crate::time::{MAX_PLAUSIBLE_MS, MIN_PLAUSIBLE_MS, MINUTE_MS, Ms};

/// Schema version written to (and required in) every capture file.
pub const CAPTURE_VERSION: u8 = 1;
/// The shim refuses stdin larger than this.
pub const MAX_STDIN_BYTES: usize = 4 * 1024 * 1024;
/// Captures larger than this are ignored when loading.
pub const MAX_CAPTURE_FILE_BYTES: u64 = 64 * 1024;
/// An unchanged capture is rewritten (to refresh `written_at_ms`) at most this often.
pub const REWRITE_AFTER_MS: Ms = 5 * MINUTE_MS;

/// Longest accepted session id (a UUID is 36 characters).
const MAX_SESSION_ID_LEN: usize = 64;
/// Whitelisted strings longer than these are dropped (not truncated: a cut-off path or id would be
/// wrong rather than short). The caps keep every record far below [`MAX_CAPTURE_FILE_BYTES`].
const MAX_PATH_LEN: usize = 4096;
const MAX_LABEL_LEN: usize = 256;
/// At most this many rate-limit windows are kept, each keyed `[A-Za-z0-9_-]{1,64}`.
const MAX_WINDOWS: usize = 16;
const MAX_WINDOW_KEY_LEN: usize = 64;
/// Gateway spend limit reported alongside the plan windows; not a usage window.
const SPEND_LIMIT_KEY: &str = "spend_limit";
const RENAME_RETRIES: u32 = 3;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(15);

/// One session's whitelisted statusline values, as stored in `<capture_dir>/<session_id>.json`.
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

/// `model` object of the statusline JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

/// Context-window usage reported by the statusline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CtxInfo {
    pub used_percentage: Option<f32>,
    pub context_window_size: Option<u64>,
    #[serde(default)]
    pub exceeds_200k: Option<bool>,
}

/// One plan usage window from `rate_limits`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    /// 0..=100
    pub used_percentage: f32,
    /// Epoch seconds, as Claude Code reports it.
    pub resets_at: i64,
}

/// What [`write_capture`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    /// Fingerprint unchanged and the file is recent; nothing written.
    Skipped,
}

/// Why the shim could not capture a statusline payload. Messages never include input content.
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
/// Windows device names (`CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9`, any case) are
/// rejected too: `NUL.json` opens the device, not a file.
pub fn sanitize_session_id(raw: &str) -> Option<String> {
    let valid = (1..=MAX_SESSION_ID_LEN).contains(&raw.len())
        && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !is_windows_device_name(raw);
    valid.then(|| raw.to_owned())
}

fn is_windows_device_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    match upper.as_bytes() {
        [b'C', b'O', b'M', digit] | [b'L', b'P', b'T', digit] => digit.is_ascii_digit(),
        _ => matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL"),
    }
}

/// Builds a record from a parsed statusline JSON value using the whitelist in the module docs.
/// Sets `written_at_ms = changed_at_ms = now_ms` and computes the fingerprint.
/// Returns `None` if `session_id` is missing or invalid.
pub fn extract_whitelisted(json: &serde_json::Value, now_ms: Ms) -> Option<CaptureRecord> {
    let obj = json.as_object()?;
    let session_id = sanitize_session_id(obj.get("session_id")?.as_str()?)?;

    let model = obj.get("model").and_then(Value::as_object).and_then(|m| {
        model_info(
            bounded_str(m.get("id"), MAX_LABEL_LEN),
            bounded_str(m.get("display_name"), MAX_LABEL_LEN),
        )
    });

    let ctx = obj.get("context_window").and_then(Value::as_object);
    let context = ctx_info(
        ctx.and_then(|c| percent(c.get("used_percentage"))),
        ctx.and_then(|c| non_negative_u64(c.get("context_window_size"))),
        obj.get("exceeds_200k_tokens").and_then(Value::as_bool),
    );

    let rate_limits = obj
        .get("rate_limits")
        .and_then(Value::as_object)
        .map(rate_limits)
        .unwrap_or_default();

    let api_ms = obj
        .get("cost")
        .and_then(Value::as_object)
        .and_then(|c| non_negative_u64(c.get("total_api_duration_ms")));

    let mut rec = CaptureRecord {
        v: CAPTURE_VERSION,
        session_id,
        written_at_ms: now_ms,
        changed_at_ms: now_ms,
        fingerprint: 0,
        transcript_path: bounded_str(obj.get("transcript_path"), MAX_PATH_LEN),
        model,
        context,
        rate_limits,
        api_ms,
        cc_version: bounded_str(obj.get("version"), MAX_LABEL_LEN),
    };
    rec.fingerprint = fingerprint(&rec);
    Some(rec)
}

/// FNV-1a over: rate limits (key, pct, resets_at in key order), context (pct, size, exceeds_200k),
/// `model.id`, and `api_ms`. Excludes timestamps and `transcript_path`.
pub fn fingerprint(rec: &CaptureRecord) -> u64 {
    let mut h = Fnv64::new();
    h.write_u64(rec.rate_limits.len() as u64);
    for (key, window) in &rec.rate_limits {
        h.write_str(key)
            .write_f32(window.used_percentage)
            .write_i64(window.resets_at);
    }
    match &rec.context {
        Some(ctx) => {
            h.write(&[1]);
            hash_opt_f32(&mut h, ctx.used_percentage);
            hash_opt_u64(&mut h, ctx.context_window_size);
            h.write(&[match ctx.exceeds_200k {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            }]);
        }
        None => {
            h.write(&[0]);
        }
    }
    h.write_opt_str(rec.model.as_ref().and_then(|m| m.id.as_deref()));
    hash_opt_u64(&mut h, rec.api_ms);
    h.finish()
}

/// Atomically writes `<dir>/<session_id>.json` (create `dir` if needed; write
/// `.<session_id>.<pid>.<nanos>.tmp` in the same dir, then rename over the target, retrying the
/// rename up to 3 times 15 ms apart on Windows sharing violations). Applies the carry-over/skip
/// rules from the module docs by reading the existing file first (a missing or corrupt existing
/// file counts as "changed").
pub fn write_capture(dir: &Path, mut rec: CaptureRecord) -> io::Result<WriteOutcome> {
    let Some(id) = sanitize_session_id(&rec.session_id) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session id",
        ));
    };
    let target = dir.join(format!("{id}.json"));
    rec.v = CAPTURE_VERSION;
    rec.fingerprint = fingerprint(&rec);

    // Compare against a freshly computed fingerprint of the old record rather than its stored
    // field, so a hand-edited or stale `fingerprint` value cannot suppress a write.
    let unchanged = read_capture_file(&target)
        .filter(|old| old.session_id == id && fingerprint(old) == rec.fingerprint);
    if let Some(old) = unchanged {
        let age = rec.written_at_ms.saturating_sub(old.written_at_ms);
        // A negative age means the clock went backwards: rewrite rather than trust the old stamp.
        if (0..REWRITE_AFTER_MS).contains(&age) {
            return Ok(WriteOutcome::Skipped);
        }
        rec.changed_at_ms = old.changed_at_ms.min(rec.changed_at_ms);
    }

    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(&rec).map_err(io::Error::other)?;
    write_atomic(dir, &id, &target, &bytes)?;
    Ok(WriteOutcome::Written)
}

/// Size check, BOM strip, parse and whitelist extraction, without writing anything. The shim's
/// `--default` mode uses this to render its line from the same record it saves.
pub fn record_from_bytes(bytes: &[u8], now_ms: Ms) -> Result<CaptureRecord, CaptureError> {
    if bytes.len() > MAX_STDIN_BYTES {
        return Err(CaptureError::TooLarge);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let json: Value = serde_json::from_slice(bytes).map_err(|_| CaptureError::NotJson)?;
    if !json.is_object() {
        return Err(CaptureError::NotJson);
    }
    extract_whitelisted(&json, now_ms).ok_or(CaptureError::NoSession)
}

/// The shim's whole job after forwarding stdin: size check, parse, extract, write.
pub fn capture_from_bytes(
    bytes: &[u8],
    dir: &Path,
    now_ms: Ms,
) -> Result<WriteOutcome, CaptureError> {
    let rec = record_from_bytes(bytes, now_ms)?;
    Ok(write_capture(dir, rec)?)
}

/// Parses a capture file written by [`write_capture`]. Returns `None` for other schema versions
/// or malformed content.
pub fn read_capture(bytes: &[u8]) -> Option<CaptureRecord> {
    let rec: CaptureRecord = serde_json::from_slice(bytes).ok()?;
    if rec.v != CAPTURE_VERSION || sanitize_session_id(&rec.session_id).is_none() {
        return None;
    }
    // The file is ours, but it may have been edited by hand: apply the extraction rules again, so
    // a loaded record never holds what `extract_whitelisted` would have dropped.
    let rate_limits = rec
        .rate_limits
        .into_iter()
        .filter(|(key, window)| is_window_key(key) && plausible_resets_at(window.resets_at))
        .take(MAX_WINDOWS)
        .map(|(key, mut window)| {
            window.used_percentage = window.used_percentage.clamp(0.0, 100.0);
            (key, window)
        })
        .collect();
    let model = rec.model.and_then(|m| {
        model_info(
            m.id.filter(|s| within(s, MAX_LABEL_LEN)),
            m.display_name.filter(|s| within(s, MAX_LABEL_LEN)),
        )
    });
    let context = rec.context.and_then(|c| {
        ctx_info(
            c.used_percentage.map(|p| p.clamp(0.0, 100.0)),
            c.context_window_size,
            c.exceeds_200k,
        )
    });
    Some(CaptureRecord {
        transcript_path: rec.transcript_path.filter(|s| within(s, MAX_PATH_LEN)),
        model,
        context,
        rate_limits,
        cc_version: rec.cc_version.filter(|s| within(s, MAX_LABEL_LEN)),
        ..rec
    })
}

/// Reads and parses one capture file with a size cap. Missing, oversized or malformed → `None`.
///
/// Used by the shim (to compare with the file it is about to replace) and by pruning; both only
/// touch sanitised `*.json` names directly inside the widget's own capture dir, so this does not go
/// through [`crate::saferead::SafeReader`] (which needs the full [`crate::paths::Paths`]).
pub(crate) fn read_capture_file(path: &Path) -> Option<CaptureRecord> {
    let file = File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(MAX_CAPTURE_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_CAPTURE_FILE_BYTES {
        return None;
    }
    read_capture(&buf)
}

fn bounded_str(v: Option<&Value>, max_len: usize) -> Option<String> {
    v?.as_str()
        .filter(|s| within(s, max_len))
        .map(str::to_owned)
}

fn within(s: &str, max_len: usize) -> bool {
    !s.is_empty() && s.len() <= max_len
}

fn model_info(id: Option<String>, display_name: Option<String>) -> Option<ModelInfo> {
    (id.is_some() || display_name.is_some()).then_some(ModelInfo { id, display_name })
}

fn ctx_info(
    used_percentage: Option<f32>,
    context_window_size: Option<u64>,
    exceeds_200k: Option<bool>,
) -> Option<CtxInfo> {
    // A zero-sized window is nonsense and would only cause divisions by zero downstream.
    let context_window_size = context_window_size.filter(|&size| size > 0);
    (used_percentage.is_some() || context_window_size.is_some() || exceeds_200k.is_some())
        .then_some(CtxInfo {
            used_percentage,
            context_window_size,
            exceeds_200k,
        })
}

/// A finite number clamped to 0..=100. Clamped as f64 first so huge values do not become `inf`.
fn percent(v: Option<&Value>) -> Option<f32> {
    let f = v?.as_f64()?;
    f.is_finite().then(|| f.clamp(0.0, 100.0) as f32)
}

fn non_negative_u64(v: Option<&Value>) -> Option<u64> {
    let v = v?;
    v.as_u64().or_else(|| {
        let f = v.as_f64()?;
        // `as` saturates, so absurdly large values cannot wrap.
        (f.is_finite() && f >= 0.0).then(|| f.round() as u64)
    })
}

/// `resets_at` as epoch seconds. Claude Code sends integer seconds; epoch milliseconds and
/// RFC 3339 strings are also accepted (see [`crate::time::json_time_to_ms`]). Outside
/// [`plausible_resets_at`] → `None`.
fn epoch_secs(v: &Value) -> Option<i64> {
    // Checked after the conversion: strings are not range-checked by `json_time_to_ms`.
    crate::time::json_time_to_ms(v)
        .map(|ms| ms.div_euclid(1000))
        .filter(|&secs| plausible_resets_at(secs))
}

/// Between 2001 and 2200 (see [`crate::time::MIN_PLAUSIBLE_MS`]); anything else is corrupt.
fn plausible_resets_at(secs: i64) -> bool {
    secs.checked_mul(1000)
        .is_some_and(|ms| (MIN_PLAUSIBLE_MS..=MAX_PLAUSIBLE_MS).contains(&ms))
}

fn valid_window_key(key: &str) -> bool {
    (1..=MAX_WINDOW_KEY_LEN).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A key that may name a plan usage window: well-formed and not the gateway spend limit.
fn is_window_key(key: &str) -> bool {
    key != SPEND_LIMIT_KEY && valid_window_key(key)
}

fn rate_limits(obj: &Map<String, Value>) -> BTreeMap<String, RateLimit> {
    obj.iter()
        .filter(|(key, _)| is_window_key(key))
        .filter_map(|(key, window)| {
            let window = window.as_object()?;
            let used_percentage = percent(window.get("used_percentage"))?;
            let resets_at = epoch_secs(window.get("resets_at")?)?;
            Some((
                key.clone(),
                RateLimit {
                    used_percentage,
                    resets_at,
                },
            ))
        })
        .take(MAX_WINDOWS)
        .collect()
}

fn hash_opt_f32(h: &mut Fnv64, v: Option<f32>) {
    match v {
        Some(v) => h.write(&[1]).write_f32(v),
        None => h.write(&[0]),
    };
}

fn hash_opt_u64(h: &mut Fnv64, v: Option<u64>) {
    match v {
        Some(v) => h.write(&[1]).write_u64(v),
        None => h.write(&[0]),
    };
}

fn write_atomic(dir: &Path, id: &str, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{id}.{}.{nanos}.tmp", std::process::id()));
    // `create_new` guarantees we only ever clean up a temp file that this call created.
    let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let result = write_and_close(file, bytes).and_then(|()| rename_with_retry(&tmp, target));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// No fsync: the shim has a ~30 ms budget, and a torn file after a power cut only costs one
/// capture (readers treat it as corrupt and the next statusline run rewrites it).
fn write_and_close(mut file: File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut retries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if retries < RENAME_RETRIES && is_sharing_violation(&e) => {
                retries += 1;
                std::thread::sleep(RENAME_RETRY_DELAY);
            }
            other => return other,
        }
    }
}

/// Windows reports a target held open by a reader as ERROR_ACCESS_DENIED (5),
/// ERROR_SHARING_VIOLATION (32) or ERROR_LOCK_VIOLATION (33).
fn is_sharing_violation(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{HOUR_MS, SECOND_MS};
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use serde_json::json;

    const FULL: &str = include_str!("../tests/fixtures/statusline/full.json");
    const SID: &str = "00000000-0000-4000-8000-000000000001";
    const NOW: Ms = 1_790_200_000_000;

    fn full() -> Value {
        serde_json::from_str(FULL).unwrap()
    }

    fn rec_at(now_ms: Ms) -> CaptureRecord {
        extract_whitelisted(&full(), now_ms).unwrap()
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn sanitize_accepts_uuids_and_rejects_everything_else() {
        assert_eq!(sanitize_session_id(SID).as_deref(), Some(SID));
        assert_eq!(
            sanitize_session_id("abc-DEF-123").as_deref(),
            Some("abc-DEF-123")
        );
        let max = "a".repeat(64);
        assert_eq!(sanitize_session_id(&max), Some(max.clone()));
        for bad in [
            "", "../x", "a/b", r"a\b", "a.b", "..", "a b", "a:b", "ä", "a\0b", "a_b",
        ] {
            assert_eq!(sanitize_session_id(bad), None, "{bad:?} must be rejected");
        }
        assert_eq!(sanitize_session_id(&"a".repeat(65)), None);
    }

    #[test]
    fn sanitize_rejects_windows_device_names() {
        // `<id>.json` would name the device itself (`NUL.json` is NUL), not a file.
        for device in [
            "CON", "con", "Prn", "aux", "NUL", "nul", "COM0", "com1", "COM9", "LPT0", "lpt5",
            "LpT9",
        ] {
            assert_eq!(
                sanitize_session_id(device),
                None,
                "{device:?} must be rejected"
            );
        }
        for ordinary in [
            "CON1", "CONS", "NULL", "AUXX", "COM", "COM10", "LPT", "LPT-1", "COMA", "XCON", "PRN0",
        ] {
            assert_eq!(
                sanitize_session_id(ordinary).as_deref(),
                Some(ordinary),
                "{ordinary:?} is not a device name"
            );
        }
    }

    #[test]
    fn extracts_every_whitelisted_field() {
        let rec = rec_at(NOW);
        assert_eq!(rec.v, CAPTURE_VERSION);
        assert_eq!(rec.session_id, SID);
        assert_eq!(rec.written_at_ms, NOW);
        assert_eq!(rec.changed_at_ms, NOW);
        assert_eq!(rec.fingerprint, fingerprint(&rec));
        assert_eq!(
            rec.transcript_path.as_deref(),
            Some(
                r"C:\Users\tester\.claude\projects\C--work-demo\00000000-0000-4000-8000-000000000001.jsonl"
            )
        );
        assert_eq!(
            rec.model,
            Some(ModelInfo {
                id: Some("claude-opus-5-5[1m]".into()),
                display_name: Some("Opus 5.5 (1M context)".into()),
            })
        );
        assert_eq!(
            rec.context,
            Some(CtxInfo {
                used_percentage: Some(34.5),
                context_window_size: Some(1_000_000),
                exceeds_200k: Some(true),
            })
        );
        let keys: Vec<&str> = rec.rate_limits.keys().map(String::as_str).collect();
        assert_eq!(keys, ["five_hour", "seven_day", "seven_day_opus"]);
        assert_eq!(
            rec.rate_limits["five_hour"],
            RateLimit {
                used_percentage: 22.4,
                resets_at: 1_790_208_000,
            }
        );
        assert_eq!(rec.rate_limits["seven_day"].used_percentage, 61.0);
        assert_eq!(rec.api_ms, Some(123_456));
        assert_eq!(rec.cc_version.as_deref(), Some("2.3.4"));
    }

    #[test]
    fn output_contains_only_whitelisted_fields() {
        let out = serde_json::to_string(&rec_at(NOW)).unwrap();
        for banned in [
            "sentinel",
            "cwd",
            "workspace",
            "project_dir",
            "output_style",
            "total_cost_usd",
            "12.3456",
            "total_duration_ms",
            "total_lines",
            "remaining_percentage",
            "current_usage",
            "total_input_tokens",
            "hook_event_name",
            "spend_limit",
            "added_dirs",
            "987654321",
            "876543219",
            "7654321987",
        ] {
            assert!(
                !out.contains(banned),
                "capture output leaked {banned:?}: {out}"
            );
        }
        assert!(out.contains("123456"), "api_ms is the only cost field kept");
    }

    #[test]
    fn spend_limit_and_incomplete_windows_are_skipped() {
        let json = json!({
            "session_id": SID,
            "rate_limits": {
                "spend_limit": { "used_percentage": 5, "resets_at": 1_790_000_000 },
                "no_pct": { "resets_at": 1_790_000_000 },
                "no_reset": { "used_percentage": 5 },
                "string_pct": { "used_percentage": "5", "resets_at": 1_790_000_000 },
                "zero_reset": { "used_percentage": 5, "resets_at": 0 },
                "not_an_object": 7,
                "bad key!": { "used_percentage": 5, "resets_at": 1_790_000_000 },
                "five_hour": { "used_percentage": 5, "resets_at": 1_790_000_000 }
            }
        });
        let rec = extract_whitelisted(&json, NOW).unwrap();
        let keys: Vec<&str> = rec.rate_limits.keys().map(String::as_str).collect();
        assert_eq!(keys, ["five_hour"]);
    }

    #[test]
    fn percentages_are_clamped() {
        let json = json!({
            "session_id": SID,
            "context_window": { "used_percentage": 250.0 },
            "rate_limits": {
                "five_hour": { "used_percentage": -5, "resets_at": 1_790_000_000 },
                "seven_day": { "used_percentage": 150.5, "resets_at": 1_790_000_000 },
                "seven_day_opus": { "used_percentage": 1e300, "resets_at": 1_790_000_000 }
            }
        });
        let rec = extract_whitelisted(&json, NOW).unwrap();
        assert_eq!(rec.rate_limits["five_hour"].used_percentage, 0.0);
        assert_eq!(rec.rate_limits["seven_day"].used_percentage, 100.0);
        assert_eq!(rec.rate_limits["seven_day_opus"].used_percentage, 100.0);
        assert_eq!(rec.context.unwrap().used_percentage, Some(100.0));
    }

    #[test]
    fn resets_at_accepts_float_seconds_millis_and_rfc3339() {
        let json = json!({
            "session_id": SID,
            "rate_limits": {
                "a": { "used_percentage": 1, "resets_at": 1_790_000_000.9 },
                "b": { "used_percentage": 1, "resets_at": 1_790_000_000_123_i64 },
                "c": { "used_percentage": 1, "resets_at": "2026-09-24T00:00:00Z" },
                "d": { "used_percentage": 1, "resets_at": -3 },
                "e": { "used_percentage": 1, "resets_at": "soon" }
            }
        });
        let rec = extract_whitelisted(&json, NOW).unwrap();
        assert_eq!(rec.rate_limits["a"].resets_at, 1_790_000_000);
        assert_eq!(rec.rate_limits["b"].resets_at, 1_790_000_000);
        assert_eq!(rec.rate_limits["c"].resets_at, 1_790_208_000);
        assert!(!rec.rate_limits.contains_key("d"));
        assert!(!rec.rate_limits.contains_key("e"));
    }

    #[test]
    fn resets_at_before_2001_or_after_2200_is_skipped() {
        let json = json!({
            "session_id": SID,
            "rate_limits": {
                "half_second": { "used_percentage": 1, "resets_at": 0.5 },
                "subnormal": { "used_percentage": 1, "resets_at": 1e-300 },
                "pre_epoch": { "used_percentage": 1, "resets_at": "1969-12-31T23:59:59Z" },
                "epoch_plus_half": { "used_percentage": 1, "resets_at": "1970-01-01T00:00:00.5Z" },
                "one_second": { "used_percentage": 1, "resets_at": 1 },
                "last_second_of_2000": { "used_percentage": 1, "resets_at": 978_307_199 },
                "string_1990": { "used_percentage": 1, "resets_at": "1990-01-01T00:00:00Z" },
                "after_2200": { "used_percentage": 1, "resets_at": 7_258_118_401_i64 },
                "string_2300": { "used_percentage": 1, "resets_at": "2300-01-01T00:00:00Z" },
                "first_second_of_2001": { "used_percentage": 1, "resets_at": 978_307_200 },
                "start_of_2200": { "used_percentage": 1, "resets_at": "2200-01-01T00:00:00Z" }
            }
        });
        let rec = extract_whitelisted(&json, NOW).unwrap();
        let keys: Vec<&str> = rec.rate_limits.keys().map(String::as_str).collect();
        assert_eq!(keys, ["first_second_of_2001", "start_of_2200"]);
        assert_eq!(
            rec.rate_limits["first_second_of_2001"].resets_at,
            978_307_200
        );
        assert_eq!(rec.rate_limits["start_of_2200"].resets_at, 7_258_118_400);
    }

    #[test]
    fn missing_optional_sections_yield_none() {
        let rec = extract_whitelisted(&json!({ "session_id": SID }), NOW).unwrap();
        assert_eq!(rec.model, None);
        assert_eq!(rec.context, None);
        assert!(rec.rate_limits.is_empty());
        assert_eq!(rec.api_ms, None);
        assert_eq!(rec.transcript_path, None);
        assert_eq!(rec.cc_version, None);

        let odd = json!({
            "session_id": SID,
            "model": { "id": 5, "display_name": "" },
            "context_window": { "used_percentage": null, "context_window_size": 0 },
            "cost": { "total_api_duration_ms": -1 },
            "transcript_path": "x".repeat(MAX_PATH_LEN + 1),
            "version": ["2"]
        });
        let rec = extract_whitelisted(&odd, NOW).unwrap();
        assert_eq!(rec.model, None);
        assert_eq!(rec.context, None);
        assert_eq!(rec.api_ms, None);
        assert_eq!(rec.transcript_path, None);
        assert_eq!(rec.cc_version, None);
    }

    #[test]
    fn invalid_session_or_shape_yields_none() {
        assert_eq!(extract_whitelisted(&json!({}), NOW), None);
        assert_eq!(
            extract_whitelisted(&json!({ "session_id": "../evil" }), NOW),
            None
        );
        assert_eq!(extract_whitelisted(&json!({ "session_id": 5 }), NOW), None);
        assert_eq!(extract_whitelisted(&json!([SID]), NOW), None);
        assert_eq!(extract_whitelisted(&json!(SID), NOW), None);
    }

    #[test]
    fn fingerprint_tracks_values_not_timestamps() {
        let base = rec_at(NOW);
        let fp = fingerprint(&base);

        let mut same = base.clone();
        same.written_at_ms += HOUR_MS;
        same.changed_at_ms += HOUR_MS;
        same.transcript_path = Some("elsewhere.jsonl".into());
        same.cc_version = None;
        same.fingerprint = 0;
        if let Some(m) = same.model.as_mut() {
            m.display_name = Some("Renamed".into());
        }
        assert_eq!(fingerprint(&same), fp);

        let changes: [fn(&mut CaptureRecord); 7] = [
            |r| r.rate_limits.get_mut("five_hour").unwrap().used_percentage += 1.0,
            |r| r.rate_limits.get_mut("five_hour").unwrap().resets_at += 1,
            |r| {
                r.rate_limits.remove("seven_day_opus");
            },
            |r| r.context.as_mut().unwrap().used_percentage = Some(35.0),
            |r| r.context.as_mut().unwrap().exceeds_200k = Some(false),
            |r| r.model.as_mut().unwrap().id = Some("claude-sonnet-5".into()),
            |r| r.api_ms = Some(1),
        ];
        for change in changes {
            let mut r = base.clone();
            change(&mut r);
            assert_ne!(fingerprint(&r), fp);
        }
    }

    #[test]
    fn first_write_creates_dir_and_leaves_no_tmp() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested").join("capture");
        assert_eq!(
            write_capture(&dir, rec_at(NOW)).unwrap(),
            WriteOutcome::Written
        );
        assert_eq!(file_names(&dir), [format!("{SID}.json")]);
        let stored = read_capture(&fs::read(dir.join(format!("{SID}.json"))).unwrap()).unwrap();
        assert_eq!(stored, rec_at(NOW));
    }

    #[test]
    fn unchanged_fingerprint_is_skipped_then_rewritten_with_carried_changed_at() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join(format!("{SID}.json"));
        write_capture(dir, rec_at(NOW)).unwrap();
        let first = fs::read(&path).unwrap();

        let soon = NOW + REWRITE_AFTER_MS - 1;
        assert_eq!(
            write_capture(dir, rec_at(soon)).unwrap(),
            WriteOutcome::Skipped
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            first,
            "skipped write must not touch the file"
        );

        let later = NOW + REWRITE_AFTER_MS;
        assert_eq!(
            write_capture(dir, rec_at(later)).unwrap(),
            WriteOutcome::Written
        );
        let stored = read_capture(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored.written_at_ms, later);
        assert_eq!(
            stored.changed_at_ms, NOW,
            "values did not change, so neither does changed_at"
        );
        assert_eq!(file_names(dir), [format!("{SID}.json")]);
    }

    #[test]
    fn changed_fingerprint_advances_changed_at() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_capture(dir, rec_at(NOW)).unwrap();
        let mut next = rec_at(NOW + SECOND_MS);
        next.rate_limits
            .get_mut("five_hour")
            .unwrap()
            .used_percentage = 23.0;
        assert_eq!(write_capture(dir, next).unwrap(), WriteOutcome::Written);
        let stored = read_capture(&fs::read(dir.join(format!("{SID}.json"))).unwrap()).unwrap();
        assert_eq!(stored.changed_at_ms, NOW + SECOND_MS);
        assert_eq!(stored.written_at_ms, NOW + SECOND_MS);
        assert_eq!(stored.fingerprint, fingerprint(&stored));
    }

    #[test]
    fn clock_going_backwards_rewrites_without_moving_changed_at_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_capture(dir, rec_at(NOW)).unwrap();
        let earlier = NOW - HOUR_MS;
        assert_eq!(
            write_capture(dir, rec_at(earlier)).unwrap(),
            WriteOutcome::Written
        );
        let stored = read_capture(&fs::read(dir.join(format!("{SID}.json"))).unwrap()).unwrap();
        assert_eq!(stored.written_at_ms, earlier);
        assert_eq!(stored.changed_at_ms, earlier);
    }

    #[test]
    fn corrupt_or_foreign_existing_file_counts_as_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join(format!("{SID}.json"));
        for existing in [&b"{ not json"[..], b"", br#"{"v":99}"#] {
            fs::write(&path, existing).unwrap();
            assert_eq!(
                write_capture(dir, rec_at(NOW)).unwrap(),
                WriteOutcome::Written
            );
            assert!(read_capture(&fs::read(&path).unwrap()).is_some());
            assert_eq!(file_names(dir), [format!("{SID}.json")]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn locked_target_retries_the_rename_then_fails_without_leaving_tmp() {
        use std::os::windows::fs::OpenOptionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join(format!("{SID}.json"));
        fs::write(&path, b"held by another process").unwrap();
        // No sharing at all: neither the pre-read nor the rename can touch the file.
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        let start = std::time::Instant::now();
        let err = write_capture(dir, rec_at(NOW)).unwrap_err();
        let elapsed = start.elapsed();
        assert!(is_sharing_violation(&err), "{err:?}");
        assert!(
            elapsed >= RENAME_RETRY_DELAY * RENAME_RETRIES,
            "expected {RENAME_RETRIES} retries, took {elapsed:?}"
        );
        assert_eq!(file_names(dir), [format!("{SID}.json")], "no .tmp left");

        drop(lock);
        assert_eq!(fs::read(&path).unwrap(), b"held by another process");
        assert_eq!(
            write_capture(dir, rec_at(NOW)).unwrap(),
            WriteOutcome::Written
        );
    }

    #[test]
    fn write_rejects_unsafe_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = rec_at(NOW);
        rec.session_id = "../escape".into();
        let err = write_capture(tmp.path(), rec).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(file_names(tmp.path()).is_empty());
    }

    #[test]
    fn capture_from_bytes_reports_error_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let big = vec![b' '; MAX_STDIN_BYTES + 1];
        assert!(matches!(
            capture_from_bytes(&big, dir, NOW),
            Err(CaptureError::TooLarge)
        ));
        for not_json in [
            &b""[..],
            b"not json",
            b"[1,2]",
            b"\"str\"",
            b"{\"a\":1} trailing",
        ] {
            assert!(matches!(
                capture_from_bytes(not_json, dir, NOW),
                Err(CaptureError::NotJson)
            ));
        }
        assert!(matches!(
            capture_from_bytes(b"{}", dir, NOW),
            Err(CaptureError::NoSession)
        ));
        assert!(file_names(dir).is_empty());

        let mut with_bom = b"\xEF\xBB\xBF".to_vec();
        with_bom.extend_from_slice(FULL.as_bytes());
        assert_eq!(
            capture_from_bytes(&with_bom, dir, NOW).unwrap(),
            WriteOutcome::Written
        );
        assert_eq!(
            capture_from_bytes(FULL.as_bytes(), dir, NOW + 1).unwrap(),
            WriteOutcome::Skipped
        );
    }

    #[test]
    fn read_capture_validates() {
        let rec = rec_at(NOW);
        let bytes = serde_json::to_vec(&rec).unwrap();
        assert_eq!(read_capture(&bytes), Some(rec.clone()));

        let mut other_version = rec.clone();
        other_version.v = 2;
        assert_eq!(
            read_capture(&serde_json::to_vec(&other_version).unwrap()),
            None
        );

        let mut bad_id = rec.clone();
        bad_id.session_id = "a/b".into();
        assert_eq!(read_capture(&serde_json::to_vec(&bad_id).unwrap()), None);

        assert_eq!(read_capture(b"garbage"), None);
        assert_eq!(read_capture(&bytes[..bytes.len() / 2]), None);

        let mut edited: Value = serde_json::from_slice(&bytes).unwrap();
        edited["rate_limits"]["five_hour"]["used_percentage"] = json!(400.0);
        let read = read_capture(&serde_json::to_vec(&edited).unwrap()).unwrap();
        assert_eq!(read.rate_limits["five_hour"].used_percentage, 100.0);
    }

    #[test]
    fn read_capture_reapplies_the_extraction_rules_to_edited_files() {
        let window = json!({ "used_percentage": 5.0, "resets_at": 1_790_000_000 });
        let mut edited = serde_json::to_value(rec_at(NOW)).unwrap();
        let limits = edited["rate_limits"].as_object_mut().unwrap();
        let long_key = "k".repeat(MAX_WINDOW_KEY_LEN + 1);
        for key in [SPEND_LIMIT_KEY, "bad key!", "", long_key.as_str()] {
            limits.insert(key.to_owned(), window.clone());
        }
        for (key, resets_at) in [
            ("zero", 0),
            ("negative", -1),
            ("tiny", 5),
            ("far", i64::MAX),
        ] {
            limits.insert(
                key.to_owned(),
                json!({ "used_percentage": 5.0, "resets_at": resets_at }),
            );
        }
        edited["transcript_path"] = json!("p".repeat(MAX_PATH_LEN + 1));
        edited["cc_version"] = json!("v".repeat(MAX_LABEL_LEN + 1));
        edited["model"] = json!({ "id": "m".repeat(MAX_LABEL_LEN + 1), "display_name": "" });
        edited["context"] = json!({ "used_percentage": null, "context_window_size": 0 });

        let read = read_capture(&serde_json::to_vec(&edited).unwrap()).unwrap();
        let keys: Vec<&str> = read.rate_limits.keys().map(String::as_str).collect();
        assert_eq!(keys, ["five_hour", "seven_day", "seven_day_opus"]);
        assert_eq!(read.transcript_path, None);
        assert_eq!(read.cc_version, None);
        assert_eq!(read.model, None);
        assert_eq!(read.context, None);

        let mut label_only = serde_json::to_value(rec_at(NOW)).unwrap();
        label_only["model"]["display_name"] = json!("d".repeat(MAX_LABEL_LEN + 1));
        let read = read_capture(&serde_json::to_vec(&label_only).unwrap()).unwrap();
        assert_eq!(
            read.model,
            Some(ModelInfo {
                id: Some("claude-opus-5-5[1m]".into()),
                display_name: None,
            })
        );
    }

    #[test]
    fn read_capture_keeps_at_most_max_windows() {
        let mut rec = rec_at(NOW);
        for i in 0..MAX_WINDOWS {
            rec.rate_limits.insert(
                format!("extra_{i:02}"),
                RateLimit {
                    used_percentage: 1.0,
                    resets_at: 1_790_000_000,
                },
            );
        }
        let read = read_capture(&serde_json::to_vec(&rec).unwrap()).unwrap();
        assert_eq!(read.rate_limits.len(), MAX_WINDOWS);
    }

    #[test]
    fn oversized_existing_file_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("{SID}.json"));
        let mut padded = serde_json::to_vec(&rec_at(NOW)).unwrap();
        padded.pop();
        padded.extend(std::iter::repeat_n(b' ', MAX_CAPTURE_FILE_BYTES as usize));
        padded.push(b'}');
        fs::write(&path, &padded).unwrap();
        assert_eq!(read_capture_file(&path), None);
        assert_eq!(
            write_capture(tmp.path(), rec_at(NOW)).unwrap(),
            WriteOutcome::Written
        );
    }

    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::from),
            any::<i64>().prop_map(Value::from),
            any::<f64>()
                .prop_map(|f| serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)),
            "[a-z0-9_./-]{0,10}".prop_map(Value::from),
            Just(Value::from(SID)),
        ];
        let key = prop_oneof![
            "[a-z_]{0,10}",
            Just("session_id".to_owned()),
            Just("rate_limits".to_owned()),
            Just("five_hour".to_owned()),
            Just("spend_limit".to_owned()),
            Just("used_percentage".to_owned()),
            Just("resets_at".to_owned()),
            Just("context_window".to_owned()),
            Just("context_window_size".to_owned()),
            Just("model".to_owned()),
            Just("cost".to_owned()),
            Just("total_api_duration_ms".to_owned()),
        ];
        leaf.prop_recursive(4, 48, 6, move |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::vec((key.clone(), inner), 0..6)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        })
    }

    const RESETS_AT_RANGE: std::ops::RangeInclusive<i64> =
        MIN_PLAUSIBLE_MS / 1000..=MAX_PLAUSIBLE_MS / 1000;

    /// A percentage as Claude Code might send it, or a corrupted one.
    fn arb_pct() -> impl Strategy<Value = Value> {
        prop_oneof![
            3 => (0.0..=1.0f64).prop_map(Value::from),
            3 => (-1e3..1e3f64).prop_map(Value::from),
            1 => any::<i64>().prop_map(Value::from),
            1 => prop_oneof![Just(1e300), Just(-1e300), Just(f64::MAX)].prop_map(Value::from),
            1 => prop_oneof![Just("NaN"), Just("inf"), Just("-Infinity"), Just("1e999"), Just("12")]
                .prop_map(Value::from),
            1 => Just(Value::Null),
        ]
    }

    /// A reset time in any accepted unit, or a corrupted one.
    fn arb_resets_at() -> impl Strategy<Value = Value> {
        prop_oneof![
            4 => RESETS_AT_RANGE.prop_map(Value::from),
            2 => (MIN_PLAUSIBLE_MS..=MAX_PLAUSIBLE_MS).prop_map(Value::from),
            1 => (0.0..2e12f64).prop_map(Value::from),
            1 => (-10i64..10).prop_map(Value::from),
            1 => any::<i64>().prop_map(Value::from),
            1 => Just(Value::from(1e300)),
            1 => Just(Value::from("2026-09-24T00:00:00Z")),
            1 => prop_oneof![Just("1990-01-01T00:00:00Z"), Just("soon"), Just("NaN")]
                .prop_map(Value::from),
            1 => Just(Value::Null),
        ]
    }

    /// Statusline-shaped objects whose `rate_limits` hold many windows mixing valid and invalid
    /// keys, percentages and reset times, so the window rules are actually reached ([`arb_json`]
    /// almost never builds a complete window).
    fn arb_statusline() -> impl Strategy<Value = Value> {
        let key = prop_oneof![
            12 => "[A-Za-z0-9_-]{1,64}",
            1 => "[A-Za-z0-9_-]{65,70}",
            1 => Just("bad key!".to_owned()),
            1 => Just(String::new()),
            1 => Just(SPEND_LIMIT_KEY.to_owned()),
        ];
        let window = prop_oneof![
            19 => (arb_pct(), arb_resets_at())
                .prop_map(|(pct, resets_at)| json!({ "used_percentage": pct, "resets_at": resets_at })),
            1 => Just(json!(7)),
        ];
        let windows = prop::collection::vec((key, window), 0..48)
            .prop_map(|kv| Value::Object(kv.into_iter().collect()));
        let session_id = prop_oneof![9 => Just(SID), 1 => Just("../evil")];
        (session_id, windows, arb_pct()).prop_map(|(sid, windows, ctx_pct)| {
            json!({
                "session_id": sid,
                "rate_limits": windows,
                "context_window": { "used_percentage": ctx_pct }
            })
        })
    }

    #[test]
    fn statusline_strategy_reaches_the_window_rules() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        let mut runner = TestRunner::deterministic();
        let strategy = arb_statusline();
        let (mut with_windows, mut capped) = (0, 0);
        for _ in 0..256 {
            let json = strategy.new_tree(&mut runner).unwrap().current();
            if let Some(rec) = extract_whitelisted(&json, NOW) {
                with_windows += usize::from(!rec.rate_limits.is_empty());
                capped += usize::from(rec.rate_limits.len() == MAX_WINDOWS);
            }
        }
        assert!(
            with_windows > 128,
            "only {with_windows}/256 records kept a window"
        );
        assert!(capped > 0, "no record reached the {MAX_WINDOWS}-window cap");
    }

    proptest! {
        #[test]
        fn extraction_never_panics_and_keeps_invariants(json in arb_json()) {
            if let Some(rec) = extract_whitelisted(&json, NOW) {
                prop_assert!(sanitize_session_id(&rec.session_id).is_some());
                prop_assert!(!rec.rate_limits.contains_key(SPEND_LIMIT_KEY));
                for w in rec.rate_limits.values() {
                    prop_assert!((0.0..=100.0).contains(&w.used_percentage));
                    prop_assert!(RESETS_AT_RANGE.contains(&w.resets_at));
                }
                if let Some(pct) = rec.context.as_ref().and_then(|c| c.used_percentage) {
                    prop_assert!((0.0..=100.0).contains(&pct));
                }
                let bytes = serde_json::to_vec(&rec).unwrap();
                prop_assert_eq!(read_capture(&bytes), Some(rec));
            }
        }

        #[test]
        fn shaped_statusline_keeps_window_invariants(json in arb_statusline()) {
            let Some(rec) = extract_whitelisted(&json, NOW) else {
                prop_assert_ne!(json["session_id"].as_str(), Some(SID));
                return Ok(());
            };
            let input = json["rate_limits"].as_object().unwrap();
            prop_assert!(rec.rate_limits.len() <= MAX_WINDOWS);
            for (key, w) in &rec.rate_limits {
                prop_assert!(key != SPEND_LIMIT_KEY, "{:?}", key);
                prop_assert!(
                    (1..=MAX_WINDOW_KEY_LEN).contains(&key.len())
                        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                    "{:?}", key
                );
                prop_assert!(input.contains_key(key));
                prop_assert!((0.0..=100.0).contains(&w.used_percentage), "{}", w.used_percentage);
                prop_assert!(RESETS_AT_RANGE.contains(&w.resets_at), "{}", w.resets_at);
            }
            // Nothing acceptable is dropped below the cap: a window with a whitelisted key, a
            // finite numeric percentage and a plausible reset time is kept.
            let acceptable = input
                .iter()
                .filter(|(key, w)| {
                    key.as_str() != SPEND_LIMIT_KEY
                        && valid_window_key(key)
                        && w["used_percentage"].as_f64().is_some_and(f64::is_finite)
                        && epoch_secs(&w["resets_at"]).is_some_and(|s| RESETS_AT_RANGE.contains(&s))
                })
                .count();
            prop_assert_eq!(rec.rate_limits.len(), acceptable.min(MAX_WINDOWS));
            if let Some(pct) = rec.context.as_ref().and_then(|c| c.used_percentage) {
                prop_assert!((0.0..=100.0).contains(&pct));
            }
            let bytes = serde_json::to_vec(&rec).unwrap();
            prop_assert_eq!(read_capture(&bytes), Some(rec));
        }

        #[test]
        fn unlisted_fields_never_reach_the_output(
            marker in "zq[a-z]{12}",
            extra_key in "[a-z]{1,8}_[a-z]{1,8}",
        ) {
            prop_assume!(extra_key != "session_id");
            let mut json = full();
            json["cwd"] = json!(format!(r"C:\work\{marker}"));
            json["workspace"]["current_dir"] = json!(marker.clone());
            json["workspace"]["project_dir"] = json!(marker.clone());
            json["output_style"]["name"] = json!(marker.clone());
            json["cost"]["note"] = json!(marker.clone());
            json["context_window"]["label"] = json!(marker.clone());
            json["rate_limits"]["five_hour"]["label"] = json!(marker.clone());
            json["model"]["secret"] = json!(marker.clone());
            json[format!("{marker}_{extra_key}")] = json!(1);
            json[extra_key.as_str()] = json!(marker.clone());
            let out = serde_json::to_string(&extract_whitelisted(&json, NOW).unwrap()).unwrap();
            prop_assert!(!out.contains(&marker), "leaked marker in {}", out);
        }
    }
}
