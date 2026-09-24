//! Local usage history (`<data_root>/history.jsonl`) for sparklines, burn rate and reset estimation.
//!
//! One JSON object per line: `{"t":<ms>,"w":"5h","p":22.0,"r":<ms>|null,"s":"cli"|"desktop","e":false}`
//! (`w` = [`WindowKind::short`], `r` = reset at_ms if known, `e` = reset was estimated).
//! Malformed lines (e.g. a torn last line after a crash) are skipped on load. Rows are kept sorted
//! by `t`. Retention: [`RETAIN_MS`].

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::types::{Phase, ResetInfo, Sample, Source, SparkPoint, WindowKind, WindowState};
use crate::sources::desktop_usage::DesktopUsage;
use crate::time::{DAY_MS, HOUR_MS, Ms};

pub const RETAIN_MS: Ms = 14 * DAY_MS;
/// A sparkline bucket with no sample carries the previous value forward only if that value is
/// at most this old; otherwise the bucket is a gap (`pct: None`).
pub const SPARK_MAX_CARRY_MS: Ms = 2 * HOUR_MS;
/// Values closer than this are "unchanged" for [`History::record`].
pub const CHANGE_EPSILON: f32 = 0.05;
/// [`History::spark`] never returns more points than this (the bucket count comes from the UI).
pub const MAX_SPARK_BUCKETS: usize = 4096;

/// How often an atomic rewrite retries the final rename (Windows reports sharing violations while
/// another process, e.g. a virus scanner, briefly holds the target open).
const RENAME_ATTEMPTS: u32 = 5;
const RENAME_BACKOFF: Duration = Duration::from_millis(20);

/// One history line. Field order is the on-disk key order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryRow {
    pub t: Ms,
    pub w: String,
    pub p: f32,
    #[serde(default)]
    pub r: Option<Ms>,
    pub s: Source,
    #[serde(default)]
    pub e: bool,
}

impl HistoryRow {
    fn is_kind(&self, kind: &WindowKind) -> bool {
        self.w == kind.short() || WindowKind::from_short(&self.w) == *kind
    }
}

/// The in-memory copy of `history.jsonl`, sorted by `t`.
#[derive(Debug)]
pub struct History {
    path: PathBuf,
    rows: Vec<HistoryRow>,
}

impl History {
    /// Loads `path` if it exists (missing file → empty history; parent dirs created on first write).
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        // Windows editors may have saved the file with a UTF-8 BOM.
        let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let mut rows: Vec<HistoryRow> =
            text.split(|&b| b == b'\n').filter_map(parse_line).collect();
        rows.sort_by_key(|r| r.t);
        Ok(Self { path, rows })
    }

    /// All rows, sorted by `t` ascending.
    pub fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }

    /// Appends a row for `state` (t = `state.observed_at_ms`) if, compared with the newest row of
    /// the same window, the pct changed by more than [`CHANGE_EPSILON`] or the reset `at_ms`
    /// changed — and `t` is newer than that row. ResetAwaitingData states are not recorded.
    /// Appends to the file (open in append mode, one line + `\n`). Returns whether it appended.
    pub fn record(&mut self, state: &WindowState) -> io::Result<bool> {
        if state.phase == Phase::ResetAwaitingData || !state.pct.is_finite() {
            return Ok(false);
        }
        let row = HistoryRow {
            t: state.observed_at_ms,
            w: state.kind.short().to_owned(),
            p: state.pct,
            r: state.reset.at_ms(),
            s: state.source,
            e: matches!(state.reset, ResetInfo::Estimated { .. }),
        };
        if let Some(last) = self.rows.iter().rev().find(|r| r.is_kind(&state.kind)) {
            let changed = (row.p - last.p).abs() > CHANGE_EPSILON || row.r != last.r;
            if !changed || row.t <= last.t {
                return Ok(false);
            }
        }
        append_line(&self.path, &row)?;
        // Newer than its own window's last row, but not necessarily newer than other windows' rows.
        let at = self.rows.partition_point(|r| r.t <= row.t);
        self.rows.insert(at, row);
        Ok(true)
    }

    /// Adds Desktop samples with `t > watermark_ms` as rows (`s = desktop`, `r = None`), skipping
    /// any whose (window, t) already exists, keeps rows sorted, and rewrites the file atomically if
    /// anything was added. Returns the new watermark (max sample t seen, or the old one).
    pub fn backfill_desktop(&mut self, usage: &DesktopUsage, watermark_ms: Ms) -> io::Result<Ms> {
        let mut existing: HashSet<(WindowKind, Ms)> = self
            .rows
            .iter()
            .map(|r| (WindowKind::from_short(&r.w), r.t))
            .collect();
        let mut new_watermark = watermark_ms;
        let mut added = Vec::new();
        for (kind, samples) in &usage.series {
            for sample in samples {
                new_watermark = new_watermark.max(sample.t_ms);
                if sample.t_ms <= watermark_ms || sample.pct.is_nan() {
                    continue;
                }
                if existing.insert((kind.clone(), sample.t_ms)) {
                    added.push(HistoryRow {
                        t: sample.t_ms,
                        w: kind.short().to_owned(),
                        p: sample.pct.clamp(0.0, 100.0),
                        r: None,
                        s: Source::Desktop,
                        e: false,
                    });
                }
            }
        }
        if !added.is_empty() {
            let mut rows = self.rows.clone();
            rows.extend(added);
            rows.sort_by_key(|r| r.t);
            self.replace_rows(rows)?;
        }
        Ok(new_watermark)
    }

    /// Drops rows older than `now_ms - RETAIN_MS`, rewriting the file atomically (temp + rename)
    /// only if something was dropped.
    pub fn compact(&mut self, now_ms: Ms) -> io::Result<()> {
        let cutoff = now_ms.saturating_sub(RETAIN_MS);
        let dropped = self.rows.partition_point(|r| r.t < cutoff);
        if dropped == 0 {
            return Ok(());
        }
        let kept = self.rows[dropped..].to_vec();
        self.replace_rows(kept)
    }

    /// Rows of `kind` with `t >= since_ms`, as samples sorted ascending.
    pub fn samples(&self, kind: &WindowKind, since_ms: Ms) -> Vec<Sample> {
        let start = self.rows.partition_point(|r| r.t < since_ms);
        self.rows[start..]
            .iter()
            .filter(|r| r.is_kind(kind))
            .map(|r| Sample {
                t_ms: r.t,
                pct: r.p,
            })
            .collect()
    }

    /// `buckets` evenly spaced points over `[from_ms, to_ms]`. Each bucket's value is the MAX pct of
    /// rows inside it (peaks and resets stay visible); an empty bucket carries the previous value
    /// forward if the last row is ≤ [`SPARK_MAX_CARRY_MS`] old at the bucket start, otherwise
    /// `pct: None` (gap). `t_ms` is the bucket start. Returns an empty vec if `buckets == 0` or the
    /// range is empty.
    ///
    /// Bucket `i` covers `[start_i, start_{i+1})`; the last bucket also includes `to_ms`. The
    /// carried value is the pct of the newest row before the bucket (the current usage at that
    /// point, which after a reset is lower than the previous bucket's max). Rows before `from_ms`
    /// seed the carry for the first buckets. `buckets` is capped at [`MAX_SPARK_BUCKETS`].
    pub fn spark(
        &self,
        kind: &WindowKind,
        from_ms: Ms,
        to_ms: Ms,
        buckets: usize,
    ) -> Vec<SparkPoint> {
        let buckets = buckets.min(MAX_SPARK_BUCKETS);
        if buckets == 0 || to_ms <= from_ms {
            return Vec::new();
        }
        let rows: Vec<&HistoryRow> = self
            .rows
            .iter()
            .filter(|r| r.t <= to_ms && r.is_kind(kind))
            .collect();
        let span = i128::from(to_ms) - i128::from(from_ms);
        let count = buckets as i128;
        // Always within [from_ms, to_ms], so the narrowing cannot truncate.
        let bucket_start = |i: usize| (i128::from(from_ms) + span * i as i128 / count) as Ms;

        let mut next = rows.partition_point(|r| r.t < from_ms);
        let mut last: Option<&HistoryRow> = next.checked_sub(1).and_then(|i| rows.get(i).copied());
        let mut out = Vec::with_capacity(buckets);
        for i in 0..buckets {
            let start = bucket_start(i);
            let end = (i + 1 < buckets).then(|| bucket_start(i + 1));
            let mut max: Option<f32> = None;
            while let Some(&row) = rows.get(next).filter(|r| end.is_none_or(|e| r.t < e)) {
                max = Some(max.map_or(row.p, |m| m.max(row.p)));
                last = Some(row);
                next += 1;
            }
            let carried = || {
                last.filter(|r| start.saturating_sub(r.t) <= SPARK_MAX_CARRY_MS)
                    .map(|r| r.p)
            };
            out.push(SparkPoint {
                t_ms: start,
                pct: max.or_else(carried),
            });
        }
        out
    }

    /// Atomically rewrites the file with `rows`, then adopts them (memory is untouched on failure).
    fn replace_rows(&mut self, rows: Vec<HistoryRow>) -> io::Result<()> {
        let mut buf = Vec::with_capacity(rows.len() * 80);
        for row in &rows {
            serde_json::to_writer(&mut buf, row).map_err(io::Error::other)?;
            buf.push(b'\n');
        }
        write_atomic(&self.path, &buf)?;
        self.rows = rows;
        Ok(())
    }
}

/// Parses one line; blank or malformed lines yield `None`. `p` is clamped to `0..=100` like every
/// producer does.
fn parse_line(line: &[u8]) -> Option<HistoryRow> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let mut row = serde_json::from_slice::<HistoryRow>(line).ok()?;
    if !row.p.is_finite() {
        return None;
    }
    row.p = row.p.clamp(0.0, 100.0);
    Some(row)
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// Appends one JSON line. If the file does not end in `\n` (torn last line), a newline is written
/// first so the new row does not get glued onto the fragment.
fn append_line(path: &Path, row: &HistoryRow) -> io::Result<()> {
    ensure_parent(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)?;
    let mut line = Vec::with_capacity(96);
    if !ends_with_newline(&mut file)? {
        line.push(b'\n');
    }
    serde_json::to_writer(&mut line, row).map_err(io::Error::other)?;
    line.push(b'\n');
    file.write_all(&line)
}

/// True for an empty file or one whose last byte is `\n`.
fn ends_with_newline(file: &mut File) -> io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(true);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

/// Writes `bytes` to a sibling temp file, flushes it to disk and renames it over `path`,
/// retrying the rename a few times. The temp file is removed on failure.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    ensure_parent(path)?;
    let tmp = tmp_path(path);
    if let Err(e) = write_synced(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    let mut attempt = 1;
    loop {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < RENAME_ATTEMPTS => {
                std::thread::sleep(RENAME_BACKOFF * attempt);
                attempt += 1;
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

/// Writes and fsyncs; the handle is closed before returning (an open handle blocks the rename on
/// Windows).
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("history.jsonl"));
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::engine::types::Confidence;
    use crate::time::MINUTE_MS;
    use pretty_assertions::assert_eq;

    const T0: Ms = 1_790_000_000_000;
    const TORN_FIXTURE: &str = include_str!("../tests/fixtures/history/torn.jsonl");

    fn tmp_history() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("history.jsonl");
        (dir, path)
    }

    fn state(kind: WindowKind, pct: f32, reset: ResetInfo, t: Ms) -> WindowState {
        WindowState {
            kind,
            pct,
            reset,
            source: Source::Cli,
            observed_at_ms: t,
            stale: false,
            limit_reached: false,
            phase: Phase::Active,
        }
    }

    fn fh(pct: f32, t: Ms) -> WindowState {
        state(
            WindowKind::FiveHour,
            pct,
            ResetInfo::Exact {
                at_ms: T0 + 5 * HOUR_MS,
            },
            t,
        )
    }

    fn row(t: Ms, w: &str, p: f32) -> HistoryRow {
        HistoryRow {
            t,
            w: w.to_owned(),
            p,
            r: None,
            s: Source::Cli,
            e: false,
        }
    }

    /// A history whose rows are set directly (for the pure query functions). Its path lives in a
    /// tempdir so an accidental rewrite can never land in the crate directory.
    fn with_rows(rows: Vec<HistoryRow>) -> (tempfile::TempDir, History) {
        let (dir, path) = tmp_history();
        (dir, History { path, rows })
    }

    fn file_lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn usage(series: Vec<(WindowKind, Vec<(Ms, f32)>)>) -> DesktopUsage {
        let series: BTreeMap<WindowKind, Vec<Sample>> = series
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_iter()
                        .map(|(t_ms, pct)| Sample { t_ms, pct })
                        .collect(),
                )
            })
            .collect();
        let last_sample_ms = series.values().flatten().map(|s| s.t_ms).max();
        DesktopUsage {
            version: 2,
            series,
            last_sample_ms,
        }
    }

    // ---- open ----

    #[test]
    fn open_missing_file_is_empty_and_creates_nothing() {
        let (dir, path) = tmp_history();
        let h = History::open(path.clone()).unwrap();
        assert!(h.rows().is_empty());
        assert!(!path.exists());
        assert!(
            !dir.path().join("data").exists(),
            "parent dirs are created on first write only"
        );
    }

    #[test]
    fn open_skips_malformed_and_torn_lines_and_sorts() {
        let (_dir, path) = tmp_history();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, TORN_FIXTURE).unwrap();
        let h = History::open(path).unwrap();
        let got: Vec<(Ms, &str, f32)> = h.rows().iter().map(|r| (r.t, r.w.as_str(), r.p)).collect();
        assert_eq!(
            got,
            vec![
                (1_789_990_000_000, "7d", 40.0),
                (1_790_000_000_000, "5h", 10.0),
                (1_790_000_600_000, "5h", 12.5),
                (1_790_001_800_000, "seven_day_opus", 3.0),
            ]
        );
        let defaulted = &h.rows()[3];
        assert_eq!(
            (defaulted.r, defaulted.e, defaulted.s),
            (None, false, Source::Desktop)
        );
    }

    #[test]
    fn open_accepts_crlf_and_invalid_utf8() {
        let (_dir, path) = tmp_history();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = br#"{"t":1,"w":"5h","p":1.0,"r":null,"s":"cli","e":false}"#.to_vec();
        bytes.extend_from_slice(b"\r\n\xff\xfe garbage\n");
        bytes.extend_from_slice(br#"{"t":2,"w":"5h","p":2.0,"r":null,"s":"unknown","e":false}"#);
        fs::write(&path, bytes).unwrap();
        let h = History::open(path).unwrap();
        assert_eq!(h.rows(), &[row(1, "5h", 1.0)]);
    }

    #[test]
    fn open_clamps_out_of_range_pct() {
        // Hand-edited or foreign rows: every producer clamps, so load must too (spark/burn/record
        // would otherwise see 500%).
        let (_dir, path) = tmp_history();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"t\":1,\"w\":\"5h\",\"p\":500,\"s\":\"cli\"}\n{\"t\":2,\"w\":\"5h\",\"p\":-3.5,\"s\":\"cli\"}\n",
        )
        .unwrap();
        let h = History::open(path).unwrap();
        let ps: Vec<f32> = h.rows().iter().map(|r| r.p).collect();
        assert_eq!(ps, vec![100.0, 0.0]);
    }

    #[test]
    fn open_skips_utf8_bom() {
        // Windows editors (Notepad "UTF-8 with BOM") prepend EF BB BF; the first row must survive.
        let (_dir, path) = tmp_history();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(br#"{"t":1,"w":"5h","p":1.0,"r":null,"s":"cli","e":false}"#);
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(br#"{"t":2,"w":"5h","p":2.0,"r":null,"s":"cli","e":false}"#);
        bytes.extend_from_slice(b"\r\n");
        fs::write(&path, bytes).unwrap();
        let h = History::open(path).unwrap();
        assert_eq!(h.rows(), &[row(1, "5h", 1.0), row(2, "5h", 2.0)]);
    }

    #[test]
    fn open_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(History::open(dir.path().to_path_buf()).is_err());
    }

    // ---- record ----

    #[test]
    fn record_writes_exact_json_lines() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        assert!(h.record(&fh(22.0, T0)).unwrap());
        let mut est = state(
            WindowKind::SevenDay,
            41.5,
            ResetInfo::Estimated {
                at_ms: T0 + 3 * DAY_MS,
                plus_minus_ms: DAY_MS,
                confidence: Confidence::Low,
            },
            T0 + 1,
        );
        est.source = Source::Desktop;
        assert!(h.record(&est).unwrap());
        let unknown = state(
            WindowKind::Other("seven_day_opus".into()),
            7.0,
            ResetInfo::Unknown,
            T0 + 2,
        );
        assert!(h.record(&unknown).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{{\"t\":{T0},\"w\":\"5h\",\"p\":22.0,\"r\":{},\"s\":\"cli\",\"e\":false}}\n\
                 {{\"t\":{},\"w\":\"7d\",\"p\":41.5,\"r\":{},\"s\":\"desktop\",\"e\":true}}\n\
                 {{\"t\":{},\"w\":\"seven_day_opus\",\"p\":7.0,\"r\":null,\"s\":\"cli\",\"e\":false}}\n",
                T0 + 5 * HOUR_MS,
                T0 + 1,
                T0 + 3 * DAY_MS,
                T0 + 2,
            )
        );
        let reopened = History::open(path).unwrap();
        assert_eq!(reopened.rows(), h.rows());
    }

    #[test]
    fn record_only_on_change() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        assert!(h.record(&fh(22.0, T0)).unwrap(), "first row of a window");
        assert!(!h.record(&fh(22.0, T0 + MINUTE_MS)).unwrap(), "unchanged");
        assert!(
            !h.record(&fh(22.04, T0 + 2 * MINUTE_MS)).unwrap(),
            "within epsilon"
        );
        assert!(
            !h.record(&fh(21.96, T0 + 2 * MINUTE_MS)).unwrap(),
            "within epsilon, downwards"
        );
        assert!(
            h.record(&fh(22.1, T0 + 3 * MINUTE_MS)).unwrap(),
            "beyond epsilon"
        );
        let mut moved = fh(22.1, T0 + 4 * MINUTE_MS);
        moved.reset = ResetInfo::Exact {
            at_ms: T0 + 6 * HOUR_MS,
        };
        assert!(h.record(&moved).unwrap(), "reset changed");
        let mut unknown = fh(22.1, T0 + 5 * MINUTE_MS);
        unknown.reset = ResetInfo::Unknown;
        assert!(h.record(&unknown).unwrap(), "reset became unknown");
        assert_eq!(file_lines(&path).len(), 4);
        assert_eq!(h.rows().len(), 4);
    }

    #[test]
    fn record_requires_newer_t() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path).unwrap();
        assert!(h.record(&fh(10.0, T0)).unwrap());
        assert!(!h.record(&fh(50.0, T0)).unwrap(), "same t");
        assert!(!h.record(&fh(50.0, T0 - 1)).unwrap(), "older t");
        assert!(h.record(&fh(50.0, T0 + 1)).unwrap());
    }

    #[test]
    fn record_skips_reset_awaiting_data() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        let mut s = fh(0.0, T0);
        s.phase = Phase::ResetAwaitingData;
        assert!(!h.record(&s).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn record_compares_per_window_and_keeps_rows_sorted() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path).unwrap();
        let weekly = state(WindowKind::SevenDay, 40.0, ResetInfo::Unknown, T0 + HOUR_MS);
        assert!(h.record(&weekly).unwrap());
        assert!(
            h.record(&fh(40.0, T0)).unwrap(),
            "other window's newer row does not block"
        );
        let ts: Vec<Ms> = h.rows().iter().map(|r| r.t).collect();
        assert_eq!(ts, vec![T0, T0 + HOUR_MS]);
    }

    #[test]
    fn record_after_torn_line_starts_a_new_line() {
        let (_dir, path) = tmp_history();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, TORN_FIXTURE).unwrap();
        let mut h = History::open(path.clone()).unwrap();
        let before = h.rows().len();
        assert!(h.record(&fh(33.0, T0 + DAY_MS)).unwrap());
        let reopened = History::open(path).unwrap();
        assert_eq!(reopened.rows().len(), before + 1);
        assert_eq!(reopened.rows(), h.rows());
    }

    // ---- backfill_desktop ----

    #[test]
    fn backfill_adds_new_samples_dedups_and_rewrites_sorted() {
        let (dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        assert!(h.record(&fh(12.0, T0 + 2_000)).unwrap());
        let u = usage(vec![
            (
                WindowKind::FiveHour,
                vec![(T0 + 1_000, 10.0), (T0 + 2_000, 12.0), (T0 + 3_000, 15.0)],
            ),
            (
                WindowKind::SevenDay,
                vec![(T0 + 1_500, 40.0), (T0 + 3_000, 41.0)],
            ),
        ]);
        let wm = h.backfill_desktop(&u, T0 + 1_000).unwrap();
        assert_eq!(wm, T0 + 3_000);
        let got: Vec<(Ms, &str, Source)> =
            h.rows().iter().map(|r| (r.t, r.w.as_str(), r.s)).collect();
        assert_eq!(
            got,
            vec![
                (T0 + 1_500, "7d", Source::Desktop),
                (T0 + 2_000, "5h", Source::Cli),
                (T0 + 3_000, "5h", Source::Desktop),
                (T0 + 3_000, "7d", Source::Desktop),
            ]
        );
        assert!(
            h.rows()
                .iter()
                .filter(|r| r.s == Source::Desktop)
                .all(|r| r.r.is_none() && !r.e)
        );
        assert_eq!(
            History::open(path.clone()).unwrap().rows(),
            h.rows(),
            "file rewritten sorted"
        );
        assert_eq!(file_lines(&path).len(), 4);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("data"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(leftovers.len(), 1, "no temp file left behind");

        // Same input again: nothing new, same watermark, no rewrite.
        fs::remove_file(&path).unwrap();
        assert_eq!(h.backfill_desktop(&u, wm).unwrap(), wm);
        assert!(!path.exists(), "no rewrite when nothing was added");
        // Old watermark again: every sample above it is already present, dedup adds nothing.
        assert_eq!(h.backfill_desktop(&u, T0 + 1_000).unwrap(), wm);
        assert!(!path.exists());
        assert_eq!(h.rows().len(), 4);
    }

    #[test]
    fn backfill_watermark_never_moves_back() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        let u = usage(vec![(WindowKind::FiveHour, vec![(T0, 10.0)])]);
        assert_eq!(h.backfill_desktop(&u, T0 + DAY_MS).unwrap(), T0 + DAY_MS);
        assert!(h.rows().is_empty());
        let empty = usage(vec![]);
        assert_eq!(h.backfill_desktop(&empty, 7).unwrap(), 7);
        assert!(!path.exists());
    }

    #[test]
    fn backfill_clamps_and_skips_nan() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path).unwrap();
        let u = usage(vec![(
            WindowKind::FiveHour,
            vec![(T0, f32::NAN), (T0 + 1, 150.0), (T0 + 2, -3.0)],
        )]);
        assert_eq!(h.backfill_desktop(&u, 0).unwrap(), T0 + 2);
        let ps: Vec<f32> = h.rows().iter().map(|r| r.p).collect();
        assert_eq!(ps, vec![100.0, 0.0]);
    }

    #[test]
    fn failed_rewrite_leaves_memory_untouched_and_cleans_temp() {
        let (dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        // A directory where the file should be makes the final rename fail on every OS.
        fs::create_dir_all(&path).unwrap();
        let u = usage(vec![(WindowKind::FiveHour, vec![(T0, 10.0)])]);
        assert!(h.backfill_desktop(&u, 0).is_err());
        assert!(h.rows().is_empty());
        let entries: Vec<_> = fs::read_dir(dir.path().join("data"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(entries.len(), 1, "temp file removed");
    }

    // ---- compact ----

    #[test]
    fn compact_drops_rows_past_retention() {
        let (_dir, path) = tmp_history();
        let now = T0 + 30 * DAY_MS;
        let mut h = History::open(path.clone()).unwrap();
        let u = usage(vec![(
            WindowKind::FiveHour,
            vec![
                (now - 15 * DAY_MS, 1.0),
                (now - RETAIN_MS, 2.0),
                (now - DAY_MS, 3.0),
            ],
        )]);
        h.backfill_desktop(&u, 0).unwrap();
        h.compact(now).unwrap();
        let ps: Vec<f32> = h.rows().iter().map(|r| r.p).collect();
        assert_eq!(ps, vec![2.0, 3.0], "a row exactly at the cutoff is kept");
        assert_eq!(History::open(path.clone()).unwrap().rows(), h.rows());

        fs::remove_file(&path).unwrap();
        h.compact(now).unwrap();
        assert!(!path.exists(), "no rewrite when nothing was dropped");
    }

    #[test]
    fn compact_handles_extreme_now() {
        let (_dir, mut h) = with_rows(vec![row(T0, "5h", 1.0)]);
        h.compact(Ms::MIN).unwrap();
        assert_eq!(h.rows().len(), 1);
    }

    // ---- samples ----

    #[test]
    fn samples_filter_kind_and_time() {
        let (_dir, h) = with_rows(vec![
            row(1, "5h", 1.0),
            row(2, "7d", 50.0),
            row(3, "5h", 3.0),
            row(4, "five_hour", 4.0),
            row(5, "5h", 5.0),
        ]);
        let got = h.samples(&WindowKind::FiveHour, 3);
        assert_eq!(
            got,
            vec![
                Sample { t_ms: 3, pct: 3.0 },
                Sample { t_ms: 4, pct: 4.0 },
                Sample { t_ms: 5, pct: 5.0 },
            ]
        );
        assert_eq!(
            h.samples(&WindowKind::SevenDay, 0),
            vec![Sample { t_ms: 2, pct: 50.0 }]
        );
        assert!(h.samples(&WindowKind::SevenDay, 3).is_empty());
        assert!(
            h.samples(&WindowKind::Other("seven_day_opus".into()), 0)
                .is_empty()
        );
    }

    // ---- spark ----

    fn pcts(points: &[SparkPoint]) -> Vec<Option<f32>> {
        points.iter().map(|p| p.pct).collect()
    }

    #[test]
    fn spark_bucket_is_max_and_carries_last_row() {
        let m = MINUTE_MS;
        let (_dir, h) = with_rows(vec![
            row(T0 + 5 * m, "5h", 10.0),
            row(T0 + 20 * m, "5h", 80.0),
            row(T0 + 30 * m, "7d", 99.0),
            row(T0 + 50 * m, "5h", 5.0), // reset inside bucket 0
        ]);
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + 4 * HOUR_MS, 4);
        let starts: Vec<Ms> = points.iter().map(|p| p.t_ms).collect();
        assert_eq!(
            starts,
            vec![T0, T0 + HOUR_MS, T0 + 2 * HOUR_MS, T0 + 3 * HOUR_MS]
        );
        // Bucket 0 shows the peak; later buckets carry the post-reset value, not the peak, until
        // the last row is more than 2 h old at the bucket start (3h - 50m > 2h).
        assert_eq!(pcts(&points), vec![Some(80.0), Some(5.0), Some(5.0), None]);
    }

    #[test]
    fn spark_carry_limit_is_inclusive() {
        let (_dir, h) = with_rows(vec![row(T0 + HOUR_MS, "5h", 42.0)]);
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + 4 * HOUR_MS, 4);
        // Bucket 3 starts exactly 2 h after the row.
        assert_eq!(
            pcts(&points),
            vec![None, Some(42.0), Some(42.0), Some(42.0)]
        );
    }

    #[test]
    fn spark_seeds_carry_from_rows_before_range() {
        let (_dir, h) = with_rows(vec![
            row(T0 - 3 * HOUR_MS, "5h", 99.0),
            row(T0 - 90 * MINUTE_MS, "5h", 17.0),
        ]);
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + 2 * HOUR_MS, 4);
        // Buckets start at +0, +30, +60, +90 min, when the newest row is 90, 120, 150, 180 min old.
        assert_eq!(pcts(&points), vec![Some(17.0), Some(17.0), None, None]);
    }

    #[test]
    fn spark_last_bucket_includes_end_and_ignores_later_rows() {
        let end = T0 + 4 * HOUR_MS;
        let (_dir, h) = with_rows(vec![row(end, "5h", 60.0), row(end + 1, "5h", 90.0)]);
        let points = h.spark(&WindowKind::FiveHour, T0, end, 4);
        assert_eq!(pcts(&points), vec![None, None, None, Some(60.0)]);
    }

    #[test]
    fn spark_empty_history_is_all_gaps() {
        let (_dir, h) = with_rows(vec![]);
        let points = h.spark(&WindowKind::SevenDay, T0, T0 + DAY_MS, 3);
        assert_eq!(pcts(&points), vec![None, None, None]);
    }

    #[test]
    fn spark_degenerate_ranges() {
        let (_dir, h) = with_rows(vec![row(T0, "5h", 1.0)]);
        assert!(
            h.spark(&WindowKind::FiveHour, T0, T0 + HOUR_MS, 0)
                .is_empty()
        );
        assert!(h.spark(&WindowKind::FiveHour, T0, T0, 4).is_empty());
        assert!(h.spark(&WindowKind::FiveHour, T0 + 1, T0, 4).is_empty());
        // More buckets than milliseconds: starts are T0, T0, T0+1, T0+1, T0+2, T0+2. The first
        // bucket is zero-width (the row lands in the second); later ones carry it forward.
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + 3, 6);
        let starts: Vec<Ms> = points.iter().map(|p| p.t_ms - T0).collect();
        assert_eq!(starts, vec![0, 0, 1, 1, 2, 2]);
        assert_eq!(
            pcts(&points),
            vec![None, Some(1.0), Some(1.0), Some(1.0), Some(1.0), Some(1.0)]
        );
        let wide = h.spark(&WindowKind::FiveHour, Ms::MIN, Ms::MAX, 3);
        assert_eq!(wide.first().map(|p| p.t_ms), Some(Ms::MIN));
        assert_eq!(wide.len(), 3);
    }

    #[test]
    fn spark_bucket_count_is_capped() {
        // `buckets` comes from the UI; an absurd value must not allocate/loop without bound
        // (usize::MAX would push until the process aborts on OOM).
        let (_dir, h) = with_rows(vec![row(T0, "5h", 1.0)]);
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + DAY_MS, 1_000_000);
        assert_eq!(points.len(), MAX_SPARK_BUCKETS);
        assert_eq!(points[0].t_ms, T0);
        assert!(points.windows(2).all(|w| w[0].t_ms < w[1].t_ms));
        let huge = h.spark(&WindowKind::FiveHour, T0, T0 + DAY_MS, usize::MAX);
        assert_eq!(huge.len(), MAX_SPARK_BUCKETS);
        assert_eq!(huge.first().and_then(|p| p.pct), Some(1.0));
    }
}
