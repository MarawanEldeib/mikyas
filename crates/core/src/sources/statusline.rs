//! Loads statusline captures written by the shim and turns them into observations.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::capture::{CaptureRecord, MAX_CAPTURE_FILE_BYTES, parse_capture, read_capture_file};
use crate::engine::types::{Observation, Source, WindowKind};
use crate::saferead::SafeReader;
use crate::sources::fsutil::has_extension;
use crate::time::{Ms, system_time_ms};

/// Capture files older than this (by `written_at_ms`, falling back to mtime) are deleted by [`prune`].
pub const CAPTURE_RETAIN_MS: Ms = 7 * crate::time::DAY_MS;
/// Stray `.tmp` files older than this are deleted by [`prune`].
pub const TMP_RETAIN_MS: Ms = crate::time::MINUTE_MS;
/// Captures stamped further ahead of `now_ms` than this were written while the clock ran fast:
/// [`load_captures`] ignores them (they would sort first and never go stale).
pub const FUTURE_TOLERANCE_MS: Ms = 5 * crate::time::MINUTE_MS;
/// [`prune`] deletes files stamped further ahead of `now_ms` than this.
pub const FUTURE_RETAIN_MS: Ms = crate::time::DAY_MS;

/// Reads every `<capture_dir>/*.json` whose name does not start with `.` or `_`, through the
/// reader, skipping files larger than [`crate::capture::MAX_CAPTURE_FILE_BYTES`] and files that
/// fail [`crate::capture::parse_capture`], and records whose `changed_at_ms` or `written_at_ms` is
/// more than [`FUTURE_TOLERANCE_MS`] after `now_ms`. A missing dir yields an empty vec. Sorted by
/// `changed_at_ms` descending.
pub fn load_captures(reader: &SafeReader, capture_dir: &Path, now_ms: Ms) -> Vec<CaptureRecord> {
    let mut records = Vec::new();
    for_each_capture_file(reader, capture_dir, &mut |entry| {
        records.extend(read_record(reader, entry));
    });
    finish(records, now_ms)
}

/// [`load_captures`] across calls: a capture file is read again only when its listed size or
/// modification time changed (the shim replaces a capture by renaming a new file over it), so a
/// capture event does not re-read and re-validate every session's file.
#[derive(Debug, Default)]
pub struct CaptureCache {
    files: HashMap<PathBuf, CachedCapture>,
}

#[derive(Debug)]
struct CachedCapture {
    stamp: (Option<SystemTime>, u64),
    record: Option<CaptureRecord>,
}

impl CaptureCache {
    /// What [`load_captures`] returns now, reading only new or changed files.
    pub fn load(&mut self, reader: &SafeReader, capture_dir: &Path, now_ms: Ms) -> Vec<CaptureRecord> {
        let mut next = HashMap::with_capacity(self.files.len());
        for_each_capture_file(reader, capture_dir, &mut |entry| {
            let Ok(meta) = entry.metadata() else { return };
            let stamp = (meta.modified().ok(), meta.len());
            let path = entry.path();
            let cached = self.files.remove(&path).filter(|c| c.stamp == stamp);
            let slot = cached.unwrap_or_else(|| CachedCapture { stamp, record: read_record(reader, entry) });
            next.insert(path, slot);
        });
        self.files = next;
        finish(self.files.values().filter_map(|c| c.record.clone()).collect(), now_ms)
    }
}

/// Calls `on_file` for every loadable capture file directly in `capture_dir`. Missing, unlistable
/// or denied dirs all mean "no captures".
fn for_each_capture_file(reader: &SafeReader, capture_dir: &Path, on_file: &mut dyn FnMut(&DirEntry)) {
    let Ok(entries) = reader.read_dir(capture_dir) else { return };
    for entry in entries.flatten() {
        // `DirEntry::file_type` does not follow links, so symlinks and junctions are skipped too.
        if entry.file_type().is_ok_and(|t| t.is_file()) && is_loadable_name(&entry.file_name()) {
            on_file(&entry);
        }
    }
}

fn read_record(reader: &SafeReader, entry: &DirEntry) -> Option<CaptureRecord> {
    let bytes = reader.read(&entry.path(), MAX_CAPTURE_FILE_BYTES).ok()?;
    parse_capture(&bytes)
}

/// Drops records stamped too far ahead of `now_ms` and sorts newest first (ties by session id).
fn finish(mut records: Vec<CaptureRecord>, now_ms: Ms) -> Vec<CaptureRecord> {
    let latest = now_ms.saturating_add(FUTURE_TOLERANCE_MS);
    records.retain(|r| r.changed_at_ms <= latest && r.written_at_ms <= latest);
    records.sort_by(|a, b| b.changed_at_ms.cmp(&a.changed_at_ms).then_with(|| a.session_id.cmp(&b.session_id)));
    records
}

/// Deletes expired captures and stale temp files, and files stamped more than
/// [`FUTURE_RETAIN_MS`] after `now_ms`. Returns how many files were removed.
/// Never touches anything that is not `*.json` / `*.tmp` directly inside `capture_dir`.
pub fn prune(capture_dir: &Path, now_ms: Ms) -> io::Result<usize> {
    prune_with(capture_dir, now_ms, |_| {})
}

/// [`prune`], calling `before_remove` between judging a file expired and removing it (tests use
/// it to replace the file there, as the shim can).
fn prune_with(capture_dir: &Path, now_ms: Ms, mut before_remove: impl FnMut(&Path)) -> io::Result<usize> {
    let entries = match fs::read_dir(capture_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(kind) = prunable_kind(&path) else {
            continue;
        };
        // Taken before the read, so a replacement made after it can be detected before deleting.
        // A file that vanished or is locked is simply left for next time.
        let Ok(before) = fs::metadata(&path) else {
            continue;
        };
        let mtime_ms = before.modified().ok().and_then(system_time_ms);
        let (stamp, retain) = match kind {
            Prunable::Capture => (read_capture_file(&path).map(|r| r.written_at_ms).or(mtime_ms), CAPTURE_RETAIN_MS),
            Prunable::Tmp => (mtime_ms, TMP_RETAIN_MS),
        };
        // Unknown age → keep.
        let expired =
            stamp.is_some_and(|t| now_ms.saturating_sub(t) > retain || t.saturating_sub(now_ms) > FUTURE_RETAIN_MS);
        if expired {
            before_remove(&path);
            if remove_if_unchanged(&path, &before) {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// The shim may have renamed a fresh capture over `path` since it was read: delete it only if its
/// size and modification time still match `before`. This narrows the race to the gap between
/// this check and the delete; closing it fully would need a delete through the handle we read.
fn remove_if_unchanged(path: &Path, before: &fs::Metadata) -> bool {
    let unchanged =
        fs::metadata(path).is_ok_and(|now| now.len() == before.len() && now.modified().ok() == before.modified().ok());
    unchanged && fs::remove_file(path).is_ok()
}

/// One observation per rate-limit window per record: `kind = WindowKind::from_key(key)`,
/// `pct = used_percentage`, `resets_at_ms = resets_at * 1000` (`None` without one), `observed_at_ms = changed_at_ms`,
/// `source = Cli`. Windows whose reset time is `<= now_ms` are still returned (the merge step
/// uses expired ones to detect "reset awaiting data").
pub fn observations(records: &[CaptureRecord]) -> Vec<Observation> {
    records
        .iter()
        .flat_map(|rec| {
            rec.rate_limits
                .iter()
                // Only an in-memory record can hold NaN (JSON cannot); it has no meaningful value.
                .filter(|(_, w)| !w.used_percentage.is_nan())
                .map(move |(key, w)| Observation {
                    kind: WindowKind::from_key(key),
                    pct: w.used_percentage.clamp(0.0, 100.0),
                    resets_at_ms: w.resets_at.map(|s| s.saturating_mul(1000)),
                    observed_at_ms: rec.changed_at_ms,
                    source: Source::Cli,
                })
        })
        .collect()
}

/// Newest `changed_at_ms` across all records, for source health.
pub fn last_capture_ms(records: &[CaptureRecord]) -> Option<Ms> {
    records.iter().map(|r| r.changed_at_ms).max()
}

enum Prunable {
    Capture,
    Tmp,
}

fn is_loadable_name(name: &OsStr) -> bool {
    let Some(text) = name.to_str() else {
        return false;
    };
    !text.starts_with('.') && !text.starts_with('_') && has_extension(Path::new(name), "json")
}

fn prunable_kind(path: &Path) -> Option<Prunable> {
    path.file_name()?;
    if has_extension(path, "json") {
        Some(Prunable::Capture)
    } else if has_extension(path, "tmp") {
        Some(Prunable::Tmp)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::capture::{CAPTURE_VERSION, RateLimit};
    use crate::paths::Paths;
    use crate::time::{DAY_MS, HOUR_MS, MINUTE_MS, SECOND_MS};

    const NOW: Ms = 1_790_200_000_000;
    const SID1: &str = "00000000-0000-4000-8000-000000000001";
    const SID2: &str = "00000000-0000-4000-8000-000000000002";

    fn record(session_id: &str, changed_at_ms: Ms, windows: &[(&str, f32, i64)]) -> CaptureRecord {
        let rate_limits: BTreeMap<String, RateLimit> = windows
            .iter()
            .map(|&(k, pct, resets_at)| (k.to_owned(), RateLimit { used_percentage: pct, resets_at: Some(resets_at) }))
            .collect();
        let mut rec = CaptureRecord {
            v: CAPTURE_VERSION,
            session_id: session_id.to_owned(),
            written_at_ms: changed_at_ms,
            changed_at_ms,
            fingerprint: 0,
            model: None,
            context: None,
            rate_limits,
            api_ms: None,
        };
        rec.fingerprint = crate::capture::fingerprint(&rec);
        rec
    }

    fn setup() -> (tempfile::TempDir, SafeReader, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_roots(tmp.path().join(".claude"), vec![], tmp.path().join("data"));
        let dir = paths.capture_dir();
        fs::create_dir_all(&dir).unwrap();
        (tmp, SafeReader::new(&paths), dir)
    }

    fn write_json(path: &Path, rec: &CaptureRecord) {
        fs::write(path, serde_json::to_vec(rec).unwrap()).unwrap();
    }

    fn set_mtime(path: &Path, ms: Ms) {
        let t = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap());
        fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn capture_cache_reads_only_changed_files() {
        let (_tmp, reader, dir) = setup();
        let (p1, p2) = (dir.join(format!("{SID1}.json")), dir.join(format!("{SID2}.json")));
        write_json(&p1, &record(SID1, NOW - 2_000, &[("five_hour", 10.0, 1_790_210_000)]));
        write_json(&p2, &record(SID2, NOW - 1_000, &[("five_hour", 11.0, 1_790_210_000)]));
        set_mtime(&p1, NOW - 2_000);
        let mut cache = CaptureCache::default();
        let first = cache.load(&reader, &dir, NOW);
        assert_eq!(first, load_captures(&reader, &dir, NOW));
        assert_eq!(first.len(), 2);

        // Same size and modification time: the cached record is kept.
        write_json(&p1, &record(SID1, NOW - 2_000, &[("five_hour", 20.0, 1_790_210_000)]));
        set_mtime(&p1, NOW - 2_000);
        assert_eq!(cache.load(&reader, &dir, NOW), first);
        // A replaced file is read again; a removed one is dropped.
        set_mtime(&p1, NOW - 500);
        fs::remove_file(&p2).unwrap();
        let now = cache.load(&reader, &dir, NOW);
        assert_eq!(now, load_captures(&reader, &dir, NOW));
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].rate_limits["five_hour"].used_percentage, 20.0);
    }

    #[test]
    fn load_returns_valid_captures_newest_first() {
        let (_tmp, reader, dir) = setup();
        let older = record(SID1, NOW - HOUR_MS, &[("five_hour", 10.0, 1_790_210_000)]);
        let newer = record(SID2, NOW, &[("seven_day", 50.0, 1_790_500_000)]);
        write_json(&dir.join(format!("{SID1}.json")), &older);
        write_json(&dir.join(format!("{SID2}.JSON")), &newer);
        assert_eq!(load_captures(&reader, &dir, NOW), vec![newer, older]);
    }

    #[test]
    fn load_skips_hidden_meta_oversize_corrupt_and_foreign_files() {
        let (_tmp, reader, dir) = setup();
        let good = record(SID1, NOW, &[("five_hour", 10.0, 1_790_210_000)]);
        let other = record(SID2, NOW, &[]);
        write_json(&dir.join(format!("{SID1}.json")), &good);
        write_json(&dir.join(".hidden.json"), &other);
        write_json(&dir.join(format!(".{SID2}.123.456.tmp")), &other);
        write_json(&dir.join("_meta.json"), &other);
        write_json(&dir.join("notes.txt"), &other);
        fs::write(dir.join("_errors.log"), b"2026-09-24T00:00:00.000Z tee not_json\n").unwrap();
        fs::write(dir.join("corrupt.json"), b"{\"v\":1,\"session_id\":").unwrap();
        fs::write(dir.join("array.json"), b"[]").unwrap();
        let mut v2 = other.clone();
        v2.v = CAPTURE_VERSION + 1;
        write_json(&dir.join("future.json"), &v2);

        let mut big = serde_json::to_vec(&other).unwrap();
        big.pop();
        big.extend(std::iter::repeat_n(b' ', MAX_CAPTURE_FILE_BYTES as usize));
        big.push(b'}');
        assert!(parse_capture(&big).is_some(), "only the size makes this file unacceptable");
        fs::write(dir.join("oversize.json"), &big).unwrap();

        fs::create_dir(dir.join("nested.json")).unwrap();
        write_json(&dir.join("nested.json").join(format!("{SID2}.json")), &other);

        assert_eq!(load_captures(&reader, &dir, NOW), vec![good]);
    }

    #[test]
    fn load_of_missing_or_disallowed_dir_is_empty() {
        let (tmp, reader, dir) = setup();
        assert!(load_captures(&reader, &dir.join("missing"), NOW).is_empty());

        let outside = tmp.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        write_json(&outside.join(format!("{SID1}.json")), &record(SID1, NOW, &[]));
        assert!(load_captures(&reader, &outside, NOW).is_empty(), "the reader's allowlist applies");
    }

    #[test]
    fn prune_applies_retention_rules() {
        let (_tmp, _reader, dir) = setup();
        let expired_by_stamp = dir.join(format!("{SID1}.json"));
        write_json(&expired_by_stamp, &record(SID1, NOW - 8 * DAY_MS, &[]));
        set_mtime(&expired_by_stamp, NOW);

        let fresh_by_stamp = dir.join(format!("{SID2}.json"));
        write_json(&fresh_by_stamp, &record(SID2, NOW - HOUR_MS, &[]));
        set_mtime(&fresh_by_stamp, NOW - 30 * DAY_MS);

        let corrupt_old = dir.join("corrupt-old.json");
        fs::write(&corrupt_old, b"nope").unwrap();
        set_mtime(&corrupt_old, NOW - 8 * DAY_MS);

        let corrupt_new = dir.join("corrupt-new.json");
        fs::write(&corrupt_new, b"nope").unwrap();
        set_mtime(&corrupt_new, NOW - DAY_MS);

        let stale_tmp = dir.join(format!(".{SID1}.1.2.tmp"));
        fs::write(&stale_tmp, b"{").unwrap();
        set_mtime(&stale_tmp, NOW - 2 * MINUTE_MS);

        let live_tmp = dir.join(format!(".{SID2}.1.2.tmp"));
        fs::write(&live_tmp, b"{").unwrap();
        set_mtime(&live_tmp, NOW - 10 * SECOND_MS);

        let log = dir.join("_errors.log");
        fs::write(&log, b"x\n").unwrap();
        set_mtime(&log, NOW - 30 * DAY_MS);

        let notes = dir.join("notes.txt");
        fs::write(&notes, b"x").unwrap();
        set_mtime(&notes, NOW - 30 * DAY_MS);

        let subdir = dir.join("old.json");
        fs::create_dir(&subdir).unwrap();
        let inner = subdir.join("inner.json");
        fs::write(&inner, b"nope").unwrap();
        set_mtime(&inner, NOW - 30 * DAY_MS);

        assert_eq!(prune(&dir, NOW).unwrap(), 3);
        assert_eq!(
            names(&dir),
            [
                format!(".{SID2}.1.2.tmp"),
                format!("{SID2}.json"),
                "_errors.log".to_owned(),
                "corrupt-new.json".to_owned(),
                "notes.txt".to_owned(),
                "old.json".to_owned(),
            ]
        );
        assert!(inner.exists(), "files below subdirectories are never touched");
        assert_eq!(prune(&dir, NOW).unwrap(), 0);
    }

    #[test]
    fn prune_of_missing_dir_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(prune(&tmp.path().join("missing"), NOW).unwrap(), 0);
    }

    #[test]
    fn load_ignores_captures_stamped_in_the_future() {
        let (_tmp, reader, dir) = setup();
        let sid3 = "00000000-0000-4000-8000-000000000003";
        let slightly_ahead = record(SID1, NOW + FUTURE_TOLERANCE_MS, &[]);
        let changed_ahead = record(SID2, NOW + FUTURE_TOLERANCE_MS + 1, &[]);
        let mut written_ahead = record(sid3, NOW, &[]);
        written_ahead.written_at_ms = NOW + HOUR_MS;
        write_json(&dir.join(format!("{SID1}.json")), &slightly_ahead);
        write_json(&dir.join(format!("{SID2}.json")), &changed_ahead);
        write_json(&dir.join(format!("{sid3}.json")), &written_ahead);
        assert_eq!(load_captures(&reader, &dir, NOW), vec![slightly_ahead]);
    }

    #[test]
    fn prune_deletes_files_stamped_more_than_a_day_ahead() {
        let (_tmp, _reader, dir) = setup();
        let far_ahead = dir.join(format!("{SID1}.json"));
        write_json(&far_ahead, &record(SID1, NOW + DAY_MS + 1, &[]));
        let hours_ahead = dir.join(format!("{SID2}.json"));
        write_json(&hours_ahead, &record(SID2, NOW + 12 * HOUR_MS, &[]));

        // Files without a readable stamp are judged by mtime, with the same limits.
        let corrupt_far_ahead = dir.join("corrupt.json");
        fs::write(&corrupt_far_ahead, b"nope").unwrap();
        set_mtime(&corrupt_far_ahead, NOW + DAY_MS + 1);
        let tmp_far_ahead = dir.join(format!(".{SID1}.1.2.tmp"));
        fs::write(&tmp_far_ahead, b"{").unwrap();
        set_mtime(&tmp_far_ahead, NOW + DAY_MS + 1);
        let tmp_hours_ahead = dir.join(format!(".{SID2}.1.2.tmp"));
        fs::write(&tmp_hours_ahead, b"{").unwrap();
        set_mtime(&tmp_hours_ahead, NOW + 12 * HOUR_MS);

        assert_eq!(prune(&dir, NOW).unwrap(), 3);
        assert_eq!(names(&dir), [format!(".{SID2}.1.2.tmp"), format!("{SID2}.json")]);
    }

    #[test]
    fn prune_keeps_a_capture_replaced_after_it_was_judged_expired() {
        let (_tmp, _reader, dir) = setup();
        let path = dir.join(format!("{SID1}.json"));
        let expired = record(SID1, NOW - 8 * DAY_MS, &[]);
        let larger = record(SID1, NOW, &[("five_hour", 10.0, 1_790_210_000)]);
        let same_size = record(SID1, NOW, &[]);
        assert_eq!(
            serde_json::to_vec(&same_size).unwrap().len(),
            serde_json::to_vec(&expired).unwrap().len(),
            "only the modification time tells this replacement apart"
        );

        for fresh in [larger, same_size] {
            write_json(&path, &expired);
            set_mtime(&path, NOW - 8 * DAY_MS);
            // The shim renames a fresh capture over the file between prune's read and its delete.
            let removed = prune_with(&dir, NOW, |p| {
                let tmp = dir.join(format!(".{SID1}.1.2.tmp"));
                write_json(&tmp, &fresh);
                fs::rename(&tmp, p).unwrap();
            })
            .unwrap();
            assert_eq!(removed, 0);
            assert_eq!(read_capture_file(&path), Some(fresh));
        }

        // Untouched, the same expired capture is removed.
        write_json(&path, &expired);
        assert_eq!(prune_with(&dir, NOW, |_| {}).unwrap(), 1);
        assert!(names(&dir).is_empty());
    }

    #[test]
    fn observations_convert_units_and_keep_expired_windows() {
        let a = record(
            SID1,
            NOW - MINUTE_MS,
            &[
                ("five_hour", 22.5, 1_790_210_000),
                ("seven_day", 61.0, 1_790_100_000),
                ("seven_day_opus", 12.0, 1_790_500_000),
            ],
        );
        let b = record(SID2, NOW, &[("five_hour", 150.0, i64::MAX)]);
        let obs = observations(&[a, b]);
        assert_eq!(
            obs,
            vec![
                Observation {
                    kind: WindowKind::FiveHour,
                    pct: 22.5,
                    resets_at_ms: Some(1_790_210_000_000),
                    observed_at_ms: NOW - MINUTE_MS,
                    source: Source::Cli,
                },
                Observation {
                    kind: WindowKind::SevenDay,
                    pct: 61.0,
                    // Already past `NOW`: still reported.
                    resets_at_ms: Some(1_790_100_000_000),
                    observed_at_ms: NOW - MINUTE_MS,
                    source: Source::Cli,
                },
                Observation {
                    kind: WindowKind::Other("seven_day_opus".into()),
                    pct: 12.0,
                    resets_at_ms: Some(1_790_500_000_000),
                    observed_at_ms: NOW - MINUTE_MS,
                    source: Source::Cli,
                },
                Observation {
                    kind: WindowKind::FiveHour,
                    pct: 100.0,
                    resets_at_ms: Some(i64::MAX),
                    observed_at_ms: NOW,
                    source: Source::Cli,
                },
            ]
        );
        assert_eq!(observations(&[]), vec![]);
    }

    #[test]
    fn last_capture_is_newest_changed_at() {
        let recs = [record(SID1, NOW - HOUR_MS, &[]), record(SID2, NOW, &[])];
        assert_eq!(last_capture_ms(&recs), Some(NOW));
        assert_eq!(last_capture_ms(&[]), None);
    }
}
