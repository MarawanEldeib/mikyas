//! Loads statusline captures written by the shim and turns them into observations.

use std::io;
use std::path::Path;

use crate::capture::CaptureRecord;
use crate::engine::types::Observation;
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
    let _ = (reader, capture_dir);
    todo!("statusline::load_captures")
}

/// Deletes expired captures and stale temp files. Returns how many files were removed.
/// Never touches anything that is not `*.json` / `*.tmp` directly inside `capture_dir`.
pub fn prune(capture_dir: &Path, now_ms: Ms) -> io::Result<usize> {
    let _ = (capture_dir, now_ms);
    todo!("statusline::prune")
}

/// One observation per rate-limit window per record: `kind = WindowKind::from_key(key)`,
/// `pct = used_percentage`, `resets_at_ms = resets_at * 1000`, `observed_at_ms = changed_at_ms`,
/// `source = Cli`. Windows whose reset time is `<= now_ms` are still returned (the merge step
/// uses expired ones to detect "reset awaiting data").
pub fn observations(records: &[CaptureRecord]) -> Vec<Observation> {
    let _ = records;
    todo!("statusline::observations")
}

/// Newest `changed_at_ms` across all records, for source health.
pub fn last_capture_ms(records: &[CaptureRecord]) -> Option<Ms> {
    records.iter().map(|r| r.changed_at_ms).max()
}
