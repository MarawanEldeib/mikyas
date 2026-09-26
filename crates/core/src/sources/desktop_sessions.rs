//! Claude Desktop Code-tab session metadata:
//! `<desktop_root>/claude-code-sessions/<account>/<org>/local_<id>.json`.
//!
//! Only these fields are read (serde must ignore every other field):
//! `cliSessionId` (string; equals the transcript `sessionId`), `model` (string, may end with
//! `[1m]`), `lastFocusedAt`, `lastActivityAt` (epoch ms/s number or RFC 3339 string — use
//! [`crate::time::json_time_to_ms`]). PRIVACY: the `<account>` and `<org>` directory names are
//! never stored.
//!
//! [`load_all`] reads every file; [`DesktopSessionCache`] re-reads only the files that changed
//! since its last update and skips files not modified for [`MAX_AGE_MS`].

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::DirEntry;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::saferead::SafeReader;
use crate::sources::fsutil::{has_extension, lenient, walk_files};
use crate::time::{DAY_MS, Ms, json_time_to_ms, system_time_ms};

pub const MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_WALK_DEPTH: usize = 4;
/// [`DesktopSessionCache`] skips session files not modified for this long: their sessions have no
/// transcript the widget still tracks, so they are never shown.
pub const MAX_AGE_MS: Ms = 7 * DAY_MS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopSession {
    pub cli_session_id: Option<String>,
    pub model: Option<String>,
    pub last_focused_ms: Option<Ms>,
    pub last_activity_ms: Option<Ms>,
}

/// Loads every `local_*.json` below `dirs` (depth ≤ [`MAX_WALK_DEPTH`]) through the reader.
/// Files that fail to read or parse are skipped. Sorted by `last_activity_ms` descending.
///
/// Files directly inside a dir are at depth 1; sessions without `last_activity_ms` sort last.
/// Empty strings count as missing. Symlinks are not followed.
pub fn load_all(reader: &SafeReader, dirs: &[PathBuf]) -> Vec<DesktopSession> {
    let mut sessions = Vec::new();
    for_each_session_file(reader, dirs, &mut |entry| {
        if let Some(session) = read_session(reader, entry) {
            sessions.push(session);
        }
    });
    sorted(sessions)
}

/// [`load_all`] across calls, re-reading only files whose size or modification time changed.
#[derive(Debug, Default)]
pub struct DesktopSessionCache {
    files: HashMap<PathBuf, CachedSession>,
}

#[derive(Debug)]
struct CachedSession {
    stamp: (Option<SystemTime>, u64),
    session: Option<DesktopSession>,
}

impl DesktopSessionCache {
    /// The sessions [`load_all`] would return, leaving out files not modified within
    /// [`MAX_AGE_MS`] of `now_ms`. Unchanged files (same listed size and modification time) are
    /// not read again; files that disappeared are dropped from the cache.
    pub fn update(&mut self, reader: &SafeReader, dirs: &[PathBuf], now_ms: Ms) -> Vec<DesktopSession> {
        let floor = now_ms.saturating_sub(MAX_AGE_MS);
        let mut next = HashMap::with_capacity(self.files.len());
        for_each_session_file(reader, dirs, &mut |entry| {
            let Ok(meta) = entry.metadata() else { return };
            let modified = meta.modified().ok();
            if modified.and_then(system_time_ms).is_some_and(|m| m < floor) {
                return;
            }
            let stamp = (modified, meta.len());
            let path = entry.path();
            let cached = self.files.remove(&path).filter(|c| c.stamp == stamp);
            let slot = cached.unwrap_or_else(|| CachedSession {
                stamp,
                session: read_session(reader, entry),
            });
            next.insert(path, slot);
        });
        self.files = next;
        sorted(self.files.values().filter_map(|c| c.session.clone()).collect())
    }
}

/// Calls `on_file` for every `local_*.json` file below `dirs`.
fn for_each_session_file(reader: &SafeReader, dirs: &[PathBuf], on_file: &mut dyn FnMut(&DirEntry)) {
    for dir in dirs {
        walk_files(reader, dir, MAX_WALK_DEPTH, &|_| false, &mut |entry| {
            let name = entry.file_name();
            let is_session_file = name.to_str().is_some_and(|n| n.starts_with("local_"));
            if is_session_file && has_extension(&entry.path(), "json") {
                on_file(entry);
            }
        });
    }
}

fn read_session(reader: &SafeReader, entry: &DirEntry) -> Option<DesktopSession> {
    let bytes = reader.read(&entry.path(), MAX_FILE_BYTES).ok()?;
    parse(&bytes)
}

/// Newest activity first; `None < Some(_)`, so a descending sort puts sessions without activity
/// last. Equal activity is ordered by the remaining fields, so the result does not depend on the
/// listing order.
fn sorted(mut sessions: Vec<DesktopSession>) -> Vec<DesktopSession> {
    sessions.sort_by(|a, b| {
        Reverse(a.last_activity_ms)
            .cmp(&Reverse(b.last_activity_ms))
            .then_with(|| a.cli_session_id.cmp(&b.cli_session_id))
            .then_with(|| a.last_focused_ms.cmp(&b.last_focused_ms))
            .then_with(|| a.model.cmp(&b.model))
    });
    sessions
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSession {
    #[serde(default, deserialize_with = "lenient")]
    cli_session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    model: Option<String>,
    #[serde(default)]
    last_focused_at: Option<serde_json::Value>,
    #[serde(default)]
    last_activity_at: Option<serde_json::Value>,
}

/// Parses one metadata file; `None` unless it is a JSON object.
fn parse(bytes: &[u8]) -> Option<DesktopSession> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes); // UTF-8 BOM
    if bytes.trim_ascii_start().first() != Some(&b'{') {
        return None;
    }
    let raw: RawSession = serde_json::from_slice(bytes).ok()?;
    let non_empty = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Some(DesktopSession {
        cli_session_id: non_empty(raw.cli_session_id),
        model: non_empty(raw.model),
        last_focused_ms: raw.last_focused_at.as_ref().and_then(json_time_to_ms),
        last_activity_ms: raw.last_activity_at.as_ref().and_then(json_time_to_ms),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::paths::Paths;

    const FIXTURE: &str = include_str!("../../tests/fixtures/desktop_sessions/local_basic.json");
    const SID: &str = "00000000-0000-4000-8000-000000000001";
    const T_NOON: Ms = 1_790_251_200_000; // 2026-09-24T12:00:00Z

    struct Env {
        tmp: tempfile::TempDir,
        dir: PathBuf,
        reader: SafeReader,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Roaming").join("Claude");
        let paths = Paths::with_roots(tmp.path().join(".claude"), vec![root.clone()], tmp.path().join("data"));
        let reader = SafeReader::new(&paths);
        Env {
            dir: root.join("claude-code-sessions"),
            tmp,
            reader,
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn session_json(id: &str, activity: serde_json::Value) -> Vec<u8> {
        json!({"cliSessionId": id, "model": "claude-opus-5-5", "lastActivityAt": activity})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn fixture_parses_and_ignores_extra_fields() {
        let e = env();
        write(&e.dir.join("acct-dir").join("org-dir").join("local_basic.json"), FIXTURE.as_bytes());
        let sessions = load_all(&e.reader, std::slice::from_ref(&e.dir));
        assert_eq!(
            sessions,
            vec![DesktopSession {
                cli_session_id: Some(SID.into()),
                model: Some("claude-opus-5-5[1m]".into()),
                last_focused_ms: Some(T_NOON + 60_000),
                last_activity_ms: Some(T_NOON + 5 * 60_000),
            }]
        );
    }

    #[test]
    fn times_accept_ms_seconds_and_strings() {
        let parse_time = |v: serde_json::Value| {
            let bytes = json!({"lastFocusedAt": v.clone(), "lastActivityAt": v}).to_string();
            let s = parse(bytes.as_bytes()).unwrap();
            assert_eq!(s.last_focused_ms, s.last_activity_ms);
            s.last_activity_ms
        };
        assert_eq!(parse_time(json!(T_NOON)), Some(T_NOON));
        assert_eq!(parse_time(json!(T_NOON / 1000)), Some(T_NOON));
        assert_eq!(parse_time(json!(1_790_251_200.5)), Some(T_NOON + 500));
        assert_eq!(parse_time(json!("2026-09-24T12:00:00Z")), Some(T_NOON));
        assert_eq!(parse_time(json!("2026-09-24T14:00:00+02:00")), Some(T_NOON));
        assert_eq!(parse_time(json!("yesterday")), None);
        assert_eq!(parse_time(json!(null)), None);
        assert_eq!(parse_time(json!({"nested": 1})), None);
        assert_eq!(parse_time(json!(-1)), None);
    }

    #[test]
    fn odd_field_types_become_none() {
        let s = parse(br#"{"cliSessionId": 42, "model": ["x"], "lastFocusedAt": true, "extra": {"deep": [1, 2]}}"#)
            .unwrap();
        assert_eq!(
            s,
            DesktopSession {
                cli_session_id: None,
                model: None,
                last_focused_ms: None,
                last_activity_ms: None,
            }
        );
        let s = parse(b"\xEF\xBB\xBF {\"cliSessionId\": \"  \", \"model\": \" claude-sonnet-5 \"}").unwrap();
        assert_eq!(s.cli_session_id, None, "blank id is missing");
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(parse(br#"["cliSessionId", "model"]"#), None, "arrays are not sessions");
        assert_eq!(parse(b"{ not json"), None);
        assert_eq!(parse(b""), None);
    }

    #[test]
    fn walks_filters_and_sorts() {
        let e = env();
        let org = e.dir.join("acct-dir").join("org-dir");
        write(&org.join("local_a.json"), &session_json("a", json!(T_NOON)));
        write(&org.join("local_b.json"), &session_json("b", json!(T_NOON + 1_000)));
        write(&org.join("local_none.json"), &json!({"cliSessionId": "none"}).to_string().into_bytes());
        write(&org.join("local_c.JSON"), &session_json("c", json!("2026-09-24T12:00:00.500Z")));
        write(&org.join("local_bad.json"), b"{ truncated");
        write(&org.join("remote_x.json"), &session_json("remote", json!(T_NOON + 9_000)));
        write(&org.join("local_x.txt"), &session_json("txt", json!(T_NOON + 9_000)));
        write(&e.dir.join("local_top.json"), &session_json("top", json!(T_NOON - 1)));
        // Depth: dir/1/2/3/local (depth 4) is found, dir/1/2/3/4/local (depth 5) is not.
        let d3 = e.dir.join("l1").join("l2").join("l3");
        write(&d3.join("local_depth4.json"), &session_json("depth4", json!(T_NOON - 2)));
        write(&d3.join("l4").join("local_depth5.json"), &session_json("depth5", json!(T_NOON + 9_000)));
        // Oversized files are skipped.
        let mut big = session_json("big", json!(T_NOON + 9_000));
        big.pop();
        big.extend_from_slice(format!(",\"pad\":\"{}\"}}", "p".repeat(MAX_FILE_BYTES as usize)).as_bytes());
        write(&org.join("local_big.json"), &big);

        let ids: Vec<_> = load_all(&e.reader, std::slice::from_ref(&e.dir))
            .into_iter()
            .map(|s| s.cli_session_id.unwrap_or_default())
            .collect();
        assert_eq!(ids, ["b", "c", "a", "top", "depth4", "none"]);
    }

    #[test]
    fn missing_and_disallowed_dirs_are_skipped() {
        let e = env();
        let outside = e.tmp.path().join("elsewhere");
        write(&outside.join("local_x.json"), &session_json("x", json!(T_NOON)));
        let sessions = load_all(&e.reader, &[outside, e.dir.join("missing")]);
        assert!(sessions.is_empty());
    }

    #[test]
    fn cache_rereads_only_changed_files_and_skips_old_ones() {
        let e = env();
        let org = e.dir.join("acct-dir").join("org-dir");
        let (a, b) = (org.join("local_a.json"), org.join("local_b.json"));
        write(&a, &session_json("a", json!(T_NOON)));
        write(&b, &session_json("b", json!(T_NOON + 1_000)));
        let now = crate::time::now_ms();
        let dirs = std::slice::from_ref(&e.dir);
        let mut cache = DesktopSessionCache::default();
        let first = cache.update(&e.reader, dirs, now);
        assert_eq!(first, load_all(&e.reader, dirs));

        // Replace `a` behind the cache's back with the same size and time: not read again.
        let stamp = std::fs::metadata(&a).unwrap().modified().unwrap();
        write(&a, &session_json("z", json!(T_NOON)));
        std::fs::File::options().write(true).open(&a).unwrap().set_modified(stamp).unwrap();
        assert_eq!(cache.update(&e.reader, dirs, now), first, "unchanged stamp: cached");

        // A real change (new size) is picked up; a removed file is dropped.
        write(&a, &session_json("a2", json!(T_NOON + 2_000)));
        std::fs::remove_file(&b).unwrap();
        let ids: Vec<_> = cache.update(&e.reader, dirs, now).into_iter().filter_map(|s| s.cli_session_id).collect();
        assert_eq!(ids, ["a2"]);

        // Files not modified for a week are left out.
        let later = now + MAX_AGE_MS + crate::time::DAY_MS;
        assert!(cache.update(&e.reader, dirs, later).is_empty());
    }

    #[test]
    fn account_and_org_names_are_not_kept() {
        let e = env();
        let account = "acct-name-00000000";
        let org = "org-name-00000000";
        write(&e.dir.join(account).join(org).join("local_basic.json"), FIXTURE.as_bytes());
        let sessions = load_all(&e.reader, std::slice::from_ref(&e.dir));
        assert_eq!(sessions.len(), 1);
        let dump = format!("{sessions:?}");
        assert!(!dump.contains(account) && !dump.contains(org), "{dump}");
    }
}
