//! Loads statusline captures written by the shim and turns them into observations.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::capture::{CaptureRecord, MAX_CAPTURE_FILE_BYTES, read_capture, read_capture_file};
use crate::engine::types::{Observation, Source, WindowKind};
use crate::saferead::SafeReader;
use crate::time::Ms;

/// Capture files older than this (by `written_at_ms`, falling back to mtime) are deleted by [`prune`].
pub const CAPTURE_RETAIN_MS: Ms = 7 * crate::time::DAY_MS;
/// Stray `.tmp` files older than this are deleted by [`prune`].
pub const TMP_RETAIN_MS: Ms = crate::time::MINUTE_MS;

/// Reads every `<capture_dir>/*.json` whose name does not start with `.` or `_`, through the
/// reader, skipping files larger than [`crate::capture::MAX_CAPTURE_FILE_BYTES`] and files that
/// fail [`crate::capture::read_capture`]. A missing dir yields an empty vec. Sorted by
/// `changed_at_ms` descending.
pub fn load_captures(reader: &SafeReader, capture_dir: &Path) -> Vec<CaptureRecord> {
    // Missing, unlistable or denied dirs all mean "no captures".
    let Ok(entries) = reader.read_dir(capture_dir) else {
        return Vec::new();
    };
    let mut records: Vec<CaptureRecord> = entries
        .flatten()
        // `DirEntry::file_type` does not follow links, so symlinks and junctions are skipped too.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| is_loadable_name(&e.file_name()))
        .filter_map(|e| reader.read(&e.path(), MAX_CAPTURE_FILE_BYTES).ok())
        .filter_map(|bytes| read_capture(&bytes))
        .collect();
    records.sort_by(|a, b| {
        b.changed_at_ms
            .cmp(&a.changed_at_ms)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    records
}

/// Deletes expired captures and stale temp files. Returns how many files were removed.
/// Never touches anything that is not `*.json` / `*.tmp` directly inside `capture_dir`.
pub fn prune(capture_dir: &Path, now_ms: Ms) -> io::Result<usize> {
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
        let mtime_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(system_time_ms);
        let (stamp, retain) = match kind {
            Prunable::Capture => (
                read_capture_file(&path)
                    .map(|r| r.written_at_ms)
                    .or(mtime_ms),
                CAPTURE_RETAIN_MS,
            ),
            Prunable::Tmp => (mtime_ms, TMP_RETAIN_MS),
        };
        // Unknown age → keep. A file that vanished or is locked is simply left for next time.
        let expired = stamp.is_some_and(|t| now_ms.saturating_sub(t) > retain);
        if expired && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// One observation per rate-limit window per record: `kind = WindowKind::from_key(key)`,
/// `pct = used_percentage`, `resets_at_ms = resets_at * 1000`, `observed_at_ms = changed_at_ms`,
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
                    resets_at_ms: Some(w.resets_at.saturating_mul(1000)),
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

fn has_extension(name: &OsStr, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn is_loadable_name(name: &OsStr) -> bool {
    let Some(text) = name.to_str() else {
        return false;
    };
    !text.starts_with('.') && !text.starts_with('_') && has_extension(name, "json")
}

fn prunable_kind(path: &Path) -> Option<Prunable> {
    let name = path.file_name()?;
    if has_extension(name, "json") {
        Some(Prunable::Capture)
    } else if has_extension(name, "tmp") {
        Some(Prunable::Tmp)
    } else {
        None
    }
}

fn system_time_ms(t: SystemTime) -> Option<Ms> {
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Some(Ms::try_from(d.as_millis()).unwrap_or(Ms::MAX))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

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
            .map(|&(k, pct, resets_at)| {
                (
                    k.to_owned(),
                    RateLimit {
                        used_percentage: pct,
                        resets_at,
                    },
                )
            })
            .collect();
        let mut rec = CaptureRecord {
            v: CAPTURE_VERSION,
            session_id: session_id.to_owned(),
            written_at_ms: changed_at_ms,
            changed_at_ms,
            fingerprint: 0,
            transcript_path: None,
            model: None,
            context: None,
            rate_limits,
            api_ms: None,
            cc_version: None,
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
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn load_returns_valid_captures_newest_first() {
        let (_tmp, reader, dir) = setup();
        let older = record(SID1, NOW - HOUR_MS, &[("five_hour", 10.0, 1_790_210_000)]);
        let newer = record(SID2, NOW, &[("seven_day", 50.0, 1_790_500_000)]);
        write_json(&dir.join(format!("{SID1}.json")), &older);
        write_json(&dir.join(format!("{SID2}.JSON")), &newer);
        assert_eq!(load_captures(&reader, &dir), vec![newer, older]);
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
        fs::write(
            dir.join("_errors.log"),
            b"2026-09-24T00:00:00.000Z tee not_json\n",
        )
        .unwrap();
        fs::write(dir.join("corrupt.json"), b"{\"v\":1,\"session_id\":").unwrap();
        fs::write(dir.join("array.json"), b"[]").unwrap();
        let mut v2 = other.clone();
        v2.v = CAPTURE_VERSION + 1;
        write_json(&dir.join("future.json"), &v2);

        let mut big = serde_json::to_vec(&other).unwrap();
        big.pop();
        big.extend(std::iter::repeat_n(b' ', MAX_CAPTURE_FILE_BYTES as usize));
        big.push(b'}');
        assert!(
            read_capture(&big).is_some(),
            "only the size makes this file unacceptable"
        );
        fs::write(dir.join("oversize.json"), &big).unwrap();

        fs::create_dir(dir.join("nested.json")).unwrap();
        write_json(
            &dir.join("nested.json").join(format!("{SID2}.json")),
            &other,
        );

        assert_eq!(load_captures(&reader, &dir), vec![good]);
    }

    #[test]
    fn load_of_missing_or_disallowed_dir_is_empty() {
        let (tmp, reader, dir) = setup();
        assert!(load_captures(&reader, &dir.join("missing")).is_empty());

        let outside = tmp.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        write_json(
            &outside.join(format!("{SID1}.json")),
            &record(SID1, NOW, &[]),
        );
        assert!(
            load_captures(&reader, &outside).is_empty(),
            "the reader's allowlist applies"
        );
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
        assert!(
            inner.exists(),
            "files below subdirectories are never touched"
        );
        assert_eq!(prune(&dir, NOW).unwrap(), 0);
    }

    #[test]
    fn prune_of_missing_dir_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(prune(&tmp.path().join("missing"), NOW).unwrap(), 0);
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
