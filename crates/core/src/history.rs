//! Local usage history (`<data_root>/history.jsonl`) for sparklines, burn rate and reset estimation.
//!
//! One JSON object per line: `{"t":<ms>,"w":"5h","p":22.0,"r":<ms>|null,"s":"cli"|"desktop","e":false}`
//! (`w` = [`WindowKind::short`], `r` = reset at_ms if known, `e` = reset was estimated).
//! Malformed lines (e.g. a torn last line after a crash) are skipped on load. Rows are kept sorted
//! by `t`. Retention: [`RETAIN_MS`].
//!
//! [`History::view`] aggregates one window for the History view (see its docs for the rules).

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::types::{
    Phase, RESET_DROP_PCT, ResetInfo, Sample, Source, SparkPoint, WindowKind, WindowState, is_reset_drop,
};
use crate::sources::desktop_usage::DesktopUsage;
use crate::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms};

/// How long rows are kept (whole days). The History view's longest range follows it: the app
/// derives `history_view::MAX_DAYS` from it and sends that to the UI.
pub const RETAIN_MS: Ms = 14 * DAY_MS;
const _: () = assert!(RETAIN_MS > 0 && RETAIN_MS % DAY_MS == 0, "RETAIN_MS must be whole days");
/// A drop of at least this many points between consecutive rows of a window is a reset: the
/// engine-wide [`RESET_DROP_PCT`]. Rows mix CLI values (one decimal) with Desktop integers that
/// can lag a little, so 79.4 then 78 is still one window. The burn fit, which sees the same mix,
/// uses it too.
pub const MIXED_SOURCE_DROP_PCT: f32 = RESET_DROP_PCT;
/// Reset marks closer than this (inclusive) are one reset in [`History::view`].
pub const RESET_DEDUP_MS: Ms = 30 * MINUTE_MS;
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
    /// True if the row belongs to `kind` (no allocation).
    pub fn is_kind(&self, kind: &WindowKind) -> bool {
        kind.matches_short(&self.w)
    }
}

/// Range and local-day boundaries for [`History::view`].
#[derive(Debug, Clone, Copy)]
pub struct ViewRange<'a> {
    pub from_ms: Ms,
    /// Inclusive end; the caller aligns the range to whole hours.
    pub to_ms: Ms,
    /// Exact reset times after this have not happened yet.
    pub now_ms: Ms,
    /// Ascending local midnights. Day `i` covers `[day_starts[i], day_starts[i + 1])`, the last one
    /// ends at `to_ms`. Computed by the caller in the user's zone, so 23 h and 25 h DST days work.
    pub day_starts: &'a [Ms],
}

/// One local calendar day of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayUsage {
    pub day_start_ms: Ms,
    /// Highest % recorded that day (0 without rows).
    pub peak_pct: f32,
    /// Share of the limit used that day: the sum of rises (see [`History::view`]).
    pub consumed_pct: f32,
    /// Rows that day; 0 means no data, unlike a day at 0%.
    pub samples: u32,
}

/// One window's history over a [`ViewRange`].
#[derive(Debug, Clone, PartialEq)]
pub struct WindowHistory {
    pub kind: WindowKind,
    /// One point per hour (max % in the hour), gaps as in [`History::spark`].
    pub points: Vec<SparkPoint>,
    /// Detected resets within the range, ascending.
    pub resets_ms: Vec<Ms>,
    /// One entry per `day_starts` value.
    pub days: Vec<DayUsage>,
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
        let mut rows: Vec<HistoryRow> = text.split(|&b| b == b'\n').filter_map(parse_line).collect();
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
    /// any whose (window, t) already exists, and keeps rows sorted. When every new row is newer
    /// than the last row, they are appended to the file (like [`Self::record`]); otherwise the file
    /// is rewritten atomically. Returns the new watermark (max sample t seen, or the old one).
    pub fn backfill_desktop(&mut self, usage: &DesktopUsage, watermark_ms: Ms) -> io::Result<Ms> {
        // Only rows after the watermark can collide with a sample that is added.
        let after = self.rows.partition_point(|r| r.t <= watermark_ms);
        let mut existing: HashSet<(WindowKind, Ms)> =
            self.rows[after..].iter().map(|r| (WindowKind::from_short(&r.w), r.t)).collect();
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
        if added.is_empty() {
            return Ok(new_watermark);
        }
        added.sort_by_key(|r| r.t);
        let last_t = self.rows.last().map(|r| r.t);
        if last_t.is_none_or(|last| added[0].t > last) {
            append_lines(&self.path, &added)?;
            self.rows.extend(added);
        } else {
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
        self.rows[start..].iter().filter(|r| r.is_kind(kind)).map(|r| Sample { t_ms: r.t, pct: r.p }).collect()
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
    pub fn spark(&self, kind: &WindowKind, from_ms: Ms, to_ms: Ms, buckets: usize) -> Vec<SparkPoint> {
        let buckets = buckets.min(MAX_SPARK_BUCKETS);
        if buckets == 0 || to_ms <= from_ms {
            return Vec::new();
        }
        // Rows older than this can never be carried into the first bucket.
        let seed_from = from_ms.saturating_sub(SPARK_MAX_CARRY_MS);
        let first = self.rows.partition_point(|r| r.t < seed_from);
        let rows: Vec<&HistoryRow> =
            self.rows[first..].iter().take_while(|r| r.t <= to_ms).filter(|r| r.is_kind(kind)).collect();
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
            let carried = || last.filter(|r| start.saturating_sub(r.t) <= SPARK_MAX_CARRY_MS).map(|r| r.p);
            out.push(SparkPoint { t_ms: start, pct: max.or_else(carried) });
        }
        out
    }

    /// Every window kind with at least one row: FiveHour, SevenDay, then others by key.
    pub fn kinds(&self) -> Vec<WindowKind> {
        let shorts: BTreeSet<&str> = self.rows.iter().map(|r| r.w.as_str()).collect();
        let kinds: BTreeSet<WindowKind> = shorts.into_iter().map(WindowKind::from_short).collect();
        kinds.into_iter().collect()
    }

    /// Aggregates one window for the History view.
    ///
    /// - `points`: [`History::spark`] with one bucket per hour of the range (max per hour, gaps
    ///   after [`SPARK_MAX_CARRY_MS`] without rows).
    /// - Rows are walked in order, rows before `from_ms` seeding the state. A row starts a new
    ///   window instance when its pct is at least [`MIXED_SOURCE_DROP_PCT`] below the previous
    ///   row, when the latest exact reset time known (`r` of a row with `e: false`) passed since
    ///   the previous row, or (windows of a day or less, e.g. five_hour) when it comes more than
    ///   the window's length after the previous row: that window has ended by then. A reset time
    ///   superseded before it passed (re-estimated or moved) is not one.
    /// - `resets_ms`: exact reset times that passed within `[from_ms, min(to_ms, now_ms)]`, plus
    ///   one in-range mark per new instance no exact reset explains: the window's length after
    ///   the previous row for a gap (none if that row was at 0: no window was running), else the
    ///   row showing the drop; sorted, and a mark within [`RESET_DEDUP_MS`] of the previous kept one is
    ///   dropped.
    /// - Days: `peak_pct` is the highest row that day, `samples` the number of rows.
    ///   `consumed_pct` adds, for each row of the day, its rise over the instance's high-water mark
    ///   (smaller dips, e.g. Desktop's integer values, are not counted twice), or its whole value
    ///   when it starts a new instance (the rise from 0 after a reset). The first row ever adds
    ///   nothing.
    pub fn view(&self, kind: &WindowKind, range: &ViewRange<'_>) -> WindowHistory {
        let span = range.to_ms.saturating_sub(range.from_ms).max(0);
        let hours = span / HOUR_MS + i64::from(span % HOUR_MS != 0);
        let points = self.spark(kind, range.from_ms, range.to_ms, usize::try_from(hours).unwrap_or(0));

        let passed = range.to_ms.min(range.now_ms);
        let in_range = |t: Ms| t >= range.from_ms && t <= range.to_ms;
        let passed_in_range = |t: &Ms| *t >= range.from_ms && *t <= passed;
        let mut days: Vec<DayUsage> = range
            .day_starts
            .iter()
            .map(|&day_start_ms| DayUsage { day_start_ms, peak_pct: 0.0, consumed_pct: 0.0, samples: 0 })
            .collect();
        let mut marks: Vec<Ms> = Vec::new();
        let mut known_reset: Option<Ms> = None;
        let mut prev: Option<&HistoryRow> = None;
        let mut high = 0.0_f32;
        // Windows of a day or less (five_hour, ...) start with the first message after the last one
        // ended, so a gap longer than the window ends it; longer windows keep their own schedule.
        let gap_ends = kind.duration_ms().filter(|_| kind.is_short_window());

        for row in self.rows.iter().filter(|r| r.t <= range.to_ms && r.is_kind(kind)) {
            let exact_reset = prev.zip(known_reset).is_some_and(|(p, r)| p.t < r && r <= row.t);
            let dropped = prev.is_some_and(|p| is_reset_drop(p.p, row.p));
            let expired = prev.zip(gap_ends).filter(|(p, d)| row.t.saturating_sub(p.t) > *d);
            if !exact_reset {
                let mark = match expired {
                    Some((p, d)) => (p.p > 0.0).then(|| p.t.saturating_add(d).min(row.t)),
                    None => dropped.then_some(row.t),
                };
                marks.extend(mark.filter(|&t| in_range(t)));
            }
            let new_instance = prev.is_none() || exact_reset || dropped || expired.is_some();
            let consumed = match prev {
                None => 0.0,
                Some(_) if new_instance => row.p,
                Some(_) => (row.p - high).max(0.0),
            };
            high = if new_instance { row.p } else { high.max(row.p) };

            let day = range.day_starts.partition_point(|&d| d <= row.t).checked_sub(1);
            if let Some(day) = day.and_then(|i| days.get_mut(i)) {
                day.consumed_pct += consumed;
                day.peak_pct = day.peak_pct.max(row.p);
                day.samples += 1;
            }

            if let Some(r) = row.r.filter(|_| !row.e) {
                // The previous reset time is replaced: it was a reset if it had already passed.
                if let Some(old) = known_reset.filter(|&old| old != r && old <= row.t) {
                    marks.extend(Some(old).filter(passed_in_range));
                }
                known_reset = Some(r);
            }
            prev = Some(row);
        }
        marks.extend(known_reset.filter(passed_in_range));
        marks.sort_unstable();

        let mut resets_ms: Vec<Ms> = Vec::with_capacity(marks.len());
        for t in marks {
            if resets_ms.last().is_none_or(|&last| t.saturating_sub(last) > RESET_DEDUP_MS) {
                resets_ms.push(t);
            }
        }
        WindowHistory { kind: kind.clone(), points, resets_ms, days }
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
    append_lines(path, std::slice::from_ref(row))
}

/// [`append_line`] for several rows, in one write.
fn append_lines(path: &Path, rows: &[HistoryRow]) -> io::Result<()> {
    ensure_parent(path)?;
    let mut file = OpenOptions::new().read(true).append(true).create(true).open(path)?;
    let mut line = Vec::with_capacity(96 * rows.len());
    if !ends_with_newline(&mut file)? {
        line.push(b'\n');
    }
    for row in rows {
        serde_json::to_writer(&mut line, row).map_err(io::Error::other)?;
        line.push(b'\n');
    }
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
    let mut name = path.file_name().map(OsString::from).unwrap_or_else(|| OsString::from("history.jsonl"));
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
        state(WindowKind::FiveHour, pct, ResetInfo::Exact { at_ms: T0 + 5 * HOUR_MS }, t)
    }

    fn row(t: Ms, w: &str, p: f32) -> HistoryRow {
        HistoryRow { t, w: w.to_owned(), p, r: None, s: Source::Cli, e: false }
    }

    /// A history whose rows are set directly (for the pure query functions). Its path lives in a
    /// tempdir so an accidental rewrite can never land in the crate directory.
    fn with_rows(rows: Vec<HistoryRow>) -> (tempfile::TempDir, History) {
        let (dir, path) = tmp_history();
        (dir, History { path, rows })
    }

    fn file_lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect()
    }

    fn usage(series: Vec<(WindowKind, Vec<(Ms, f32)>)>) -> DesktopUsage {
        let series: BTreeMap<WindowKind, Vec<Sample>> = series
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(|(t_ms, pct)| Sample { t_ms, pct }).collect()))
            .collect();
        let last_sample_ms = series.values().flatten().map(|s| s.t_ms).max();
        DesktopUsage { version: 2, series, last_sample_ms }
    }

    // ---- open ----

    #[test]
    fn open_missing_file_is_empty_and_creates_nothing() {
        let (dir, path) = tmp_history();
        let h = History::open(path.clone()).unwrap();
        assert!(h.rows().is_empty());
        assert!(!path.exists());
        assert!(!dir.path().join("data").exists(), "parent dirs are created on first write only");
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
        assert_eq!((defaulted.r, defaulted.e, defaulted.s), (None, false, Source::Desktop));
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
            ResetInfo::Estimated { at_ms: T0 + 3 * DAY_MS, plus_minus_ms: DAY_MS, confidence: Confidence::Low },
            T0 + 1,
        );
        est.source = Source::Desktop;
        assert!(h.record(&est).unwrap());
        let unknown = state(WindowKind::Other("seven_day_opus".into()), 7.0, ResetInfo::Unknown, T0 + 2);
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
        assert!(!h.record(&fh(22.04, T0 + 2 * MINUTE_MS)).unwrap(), "within epsilon");
        assert!(!h.record(&fh(21.96, T0 + 2 * MINUTE_MS)).unwrap(), "within epsilon, downwards");
        assert!(h.record(&fh(22.1, T0 + 3 * MINUTE_MS)).unwrap(), "beyond epsilon");
        let mut moved = fh(22.1, T0 + 4 * MINUTE_MS);
        moved.reset = ResetInfo::Exact { at_ms: T0 + 6 * HOUR_MS };
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
        assert!(h.record(&fh(40.0, T0)).unwrap(), "other window's newer row does not block");
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
            (WindowKind::FiveHour, vec![(T0 + 1_000, 10.0), (T0 + 2_000, 12.0), (T0 + 3_000, 15.0)]),
            (WindowKind::SevenDay, vec![(T0 + 1_500, 40.0), (T0 + 3_000, 41.0)]),
        ]);
        let wm = h.backfill_desktop(&u, T0 + 1_000).unwrap();
        assert_eq!(wm, T0 + 3_000);
        let got: Vec<(Ms, &str, Source)> = h.rows().iter().map(|r| (r.t, r.w.as_str(), r.s)).collect();
        assert_eq!(
            got,
            vec![
                (T0 + 1_500, "7d", Source::Desktop),
                (T0 + 2_000, "5h", Source::Cli),
                (T0 + 3_000, "5h", Source::Desktop),
                (T0 + 3_000, "7d", Source::Desktop),
            ]
        );
        assert!(h.rows().iter().filter(|r| r.s == Source::Desktop).all(|r| r.r.is_none() && !r.e));
        assert_eq!(History::open(path.clone()).unwrap().rows(), h.rows(), "file rewritten sorted");
        assert_eq!(file_lines(&path).len(), 4);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("data")).unwrap().flatten().collect();
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
    fn backfill_of_newer_samples_appends() {
        let (_dir, path) = tmp_history();
        let mut h = History::open(path.clone()).unwrap();
        assert!(h.record(&fh(12.0, T0)).unwrap());
        // A torn last line must not swallow the appended rows.
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"t\":1").unwrap();
        let u = usage(vec![
            (WindowKind::FiveHour, vec![(T0 + 2_000, 14.0), (T0 + 1_000, 13.0)]),
            (WindowKind::SevenDay, vec![(T0 + 1_000, 40.0)]),
        ]);
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(h.backfill_desktop(&u, T0).unwrap(), T0 + 2_000);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(&before), "appended, not rewritten");
        let got: Vec<(Ms, &str)> = h.rows().iter().map(|r| (r.t, r.w.as_str())).collect();
        assert_eq!(got, vec![(T0, "5h"), (T0 + 1_000, "5h"), (T0 + 1_000, "7d"), (T0 + 2_000, "5h")]);
        assert_eq!(History::open(path).unwrap().rows(), h.rows());
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
        let u = usage(vec![(WindowKind::FiveHour, vec![(T0, f32::NAN), (T0 + 1, 150.0), (T0 + 2, -3.0)])]);
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
        let entries: Vec<_> = fs::read_dir(dir.path().join("data")).unwrap().flatten().collect();
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
            vec![(now - 15 * DAY_MS, 1.0), (now - RETAIN_MS, 2.0), (now - DAY_MS, 3.0)],
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
            vec![Sample { t_ms: 3, pct: 3.0 }, Sample { t_ms: 4, pct: 4.0 }, Sample { t_ms: 5, pct: 5.0 },]
        );
        assert_eq!(h.samples(&WindowKind::SevenDay, 0), vec![Sample { t_ms: 2, pct: 50.0 }]);
        assert!(h.samples(&WindowKind::SevenDay, 3).is_empty());
        assert!(h.samples(&WindowKind::Other("seven_day_opus".into()), 0).is_empty());
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
        assert_eq!(starts, vec![T0, T0 + HOUR_MS, T0 + 2 * HOUR_MS, T0 + 3 * HOUR_MS]);
        // Bucket 0 shows the peak; later buckets carry the post-reset value, not the peak, until
        // the last row is more than 2 h old at the bucket start (3h - 50m > 2h).
        assert_eq!(pcts(&points), vec![Some(80.0), Some(5.0), Some(5.0), None]);
    }

    #[test]
    fn spark_carry_limit_is_inclusive() {
        let (_dir, h) = with_rows(vec![row(T0 + HOUR_MS, "5h", 42.0)]);
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + 4 * HOUR_MS, 4);
        // Bucket 3 starts exactly 2 h after the row.
        assert_eq!(pcts(&points), vec![None, Some(42.0), Some(42.0), Some(42.0)]);
    }

    #[test]
    fn spark_seeds_carry_from_rows_before_range() {
        let (_dir, h) = with_rows(vec![row(T0 - 3 * HOUR_MS, "5h", 99.0), row(T0 - 90 * MINUTE_MS, "5h", 17.0)]);
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
        assert!(h.spark(&WindowKind::FiveHour, T0, T0 + HOUR_MS, 0).is_empty());
        assert!(h.spark(&WindowKind::FiveHour, T0, T0, 4).is_empty());
        assert!(h.spark(&WindowKind::FiveHour, T0 + 1, T0, 4).is_empty());
        // More buckets than milliseconds: starts are T0, T0, T0+1, T0+1, T0+2, T0+2. The first
        // bucket is zero-width (the row lands in the second); later ones carry it forward.
        let points = h.spark(&WindowKind::FiveHour, T0, T0 + 3, 6);
        let starts: Vec<Ms> = points.iter().map(|p| p.t_ms - T0).collect();
        assert_eq!(starts, vec![0, 0, 1, 1, 2, 2]);
        assert_eq!(pcts(&points), vec![None, Some(1.0), Some(1.0), Some(1.0), Some(1.0), Some(1.0)]);
        let wide = h.spark(&WindowKind::FiveHour, Ms::MIN, Ms::MAX, 3);
        assert_eq!(wide.first().map(|p| p.t_ms), Some(Ms::MIN));
        assert_eq!(wide.len(), 3);
    }

    // ---- kinds / view ----

    /// An hour-aligned base for the view tests.
    const H0: Ms = T0 / HOUR_MS * HOUR_MS;

    /// A 5h row with an exact reset time.
    fn exact(t: Ms, p: f32, r: Ms) -> HistoryRow {
        HistoryRow { r: Some(r), ..row(t, "5h", p) }
    }

    fn range(from_ms: Ms, to_ms: Ms, now_ms: Ms, day_starts: &[Ms]) -> ViewRange<'_> {
        ViewRange { from_ms, to_ms, now_ms, day_starts }
    }

    fn day_values(v: &WindowHistory) -> Vec<(f32, f32)> {
        v.days.iter().map(|d| ((d.peak_pct * 10.0).round() / 10.0, (d.consumed_pct * 10.0).round() / 10.0)).collect()
    }

    #[test]
    fn kinds_are_ordered_and_unique() {
        let (_dir, h) = with_rows(vec![
            row(1, "seven_day_opus", 1.0),
            row(2, "7d", 1.0),
            row(3, "5h", 1.0),
            row(4, "five_hour", 1.0),
            row(5, "7d", 2.0),
        ]);
        assert_eq!(
            h.kinds(),
            vec![WindowKind::FiveHour, WindowKind::SevenDay, WindowKind::Other("seven_day_opus".into())]
        );
        assert!(with_rows(vec![]).1.kinds().is_empty());
    }

    #[test]
    fn view_points_are_hourly_maxima_with_gaps() {
        let m = MINUTE_MS;
        let (_dir, h) = with_rows(vec![
            row(H0 + 10 * m, "5h", 10.0),
            row(H0 + 50 * m, "5h", 30.0),
            row(H0 + 70 * m, "5h", 20.0),
            row(H0 + 80 * m, "7d", 90.0),
        ]);
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + 5 * HOUR_MS, H0 + 5 * HOUR_MS, &[]));
        let starts: Vec<Ms> = v.points.iter().map(|p| p.t_ms - H0).collect();
        assert_eq!(starts, (0..5).map(|i| i * HOUR_MS).collect::<Vec<_>>());
        // Hours 2 and 3 carry the 20% (≤ 2 h old); hour 4 starts 170 min after it: a gap.
        assert_eq!(pcts(&v.points), vec![Some(30.0), Some(20.0), Some(20.0), Some(20.0), None]);
        assert!(v.days.is_empty());
        // A partial last hour still gets its own point.
        let partial = h.view(&WindowKind::FiveHour, &range(H0, H0 + 90 * m, H0, &[]));
        assert_eq!(partial.points.len(), 2);
    }

    #[test]
    fn view_of_empty_history() {
        let (_dir, h) = with_rows(vec![]);
        let days = [H0, H0 + DAY_MS];
        let v = h.view(&WindowKind::SevenDay, &range(H0, H0 + 2 * DAY_MS, H0 + 2 * DAY_MS, &days));
        assert_eq!(v.kind, WindowKind::SevenDay);
        assert_eq!(v.points.len(), 48);
        assert!(v.points.iter().all(|p| p.pct.is_none()));
        assert!(v.resets_ms.is_empty());
        assert_eq!(day_values(&v), vec![(0.0, 0.0), (0.0, 0.0)]);
        assert_eq!(v.days[1].day_start_ms, H0 + DAY_MS);
        // A degenerate range yields no points and never panics.
        assert!(h.view(&WindowKind::FiveHour, &range(H0, H0, H0, &[])).points.is_empty());
        let wide = h.view(&WindowKind::FiveHour, &range(Ms::MIN, Ms::MAX, Ms::MAX, &[Ms::MIN]));
        assert_eq!(wide.points.len(), MAX_SPARK_BUCKETS);
    }

    #[test]
    fn view_resets_from_exact_times_and_drops() {
        let h_ = HOUR_MS;
        let (_dir, h) = with_rows(vec![
            // Instance A resets exactly at +2h (the next row is a drop the exact time explains).
            exact(H0, 10.0, H0 + 2 * h_),
            exact(H0 + h_, 40.0, H0 + 2 * h_),
            exact(H0 + 3 * h_, 15.0, H0 + 8 * h_),
            // Desktop rows without reset times: an unexplained drop at +5h, noise 20 minutes later.
            row(H0 + 4 * h_, "5h", 60.0),
            row(H0 + 5 * h_, "5h", 20.0),
            row(H0 + 5 * h_ + 20 * MINUTE_MS, "5h", 5.0),
            // The +8h reset is re-estimated to +9h before it passes: not a reset.
            exact(H0 + 6 * h_, 25.0, H0 + 9 * h_),
        ]);
        let now = H0 + 7 * h_;
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + 10 * h_, now, &[]));
        assert_eq!(v.resets_ms, vec![H0 + 2 * h_, H0 + 5 * h_]);
        // Once +9h has passed it is a reset too (no row needs to follow).
        let later = h.view(&WindowKind::FiveHour, &range(H0, H0 + 10 * h_, H0 + 9 * h_, &[]));
        assert_eq!(later.resets_ms, vec![H0 + 2 * h_, H0 + 5 * h_, H0 + 9 * h_]);
        // Only resets inside the range, and never after to_ms.
        let tail = h.view(&WindowKind::FiveHour, &range(H0 + 3 * h_, H0 + 8 * h_, H0 + 10 * h_, &[]));
        assert_eq!(tail.resets_ms, vec![H0 + 5 * h_]);
    }

    #[test]
    fn view_reset_marks_are_deduplicated_within_30_minutes() {
        let m = MINUTE_MS;
        let (_dir, h) = with_rows(vec![
            row(H0, "5h", 50.0),
            row(H0 + 10 * m, "5h", 30.0),
            row(H0 + 40 * m, "5h", 10.0), // exactly 30 min later: same reset
            row(H0 + 50 * m, "5h", 40.0),
            row(H0 + 71 * m, "5h", 5.0), // 61 min after the kept mark: a new reset
        ]);
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + 2 * HOUR_MS, H0 + 2 * HOUR_MS, &[]));
        assert_eq!(v.resets_ms, vec![H0 + 10 * m, H0 + 71 * m]);
    }

    #[test]
    fn view_consumed_sums_rises_across_resets() {
        let d0 = H0;
        let d1 = d0 + DAY_MS;
        let (_dir, h) = with_rows(vec![
            row(d0 - HOUR_MS, "5h", 40.0), // before the first day: seeds the walk
            row(d0 + HOUR_MS, "5h", 45.0),
            row(d0 + 2 * HOUR_MS, "5h", 50.0),
            row(d0 + 3 * HOUR_MS, "5h", 49.5), // noise below one point
            row(d0 + 4 * HOUR_MS, "5h", 50.3), // only 0.3 above the high-water mark
            row(d1 + HOUR_MS, "5h", 5.0),      // reset: the whole 5% counts
            row(d1 + 2 * HOUR_MS, "5h", 30.0),
            row(d1 + 3 * HOUR_MS, "7d", 99.0), // other windows are ignored
        ]);
        let days = [d0, d1];
        let v = h.view(&WindowKind::FiveHour, &range(d0, d1 + DAY_MS, d1 + DAY_MS, &days));
        assert_eq!(day_values(&v), vec![(50.3, 10.3), (30.0, 30.0)]);
        // 21 h without rows: the window seen last at d0 + 4h ended five hours later at the latest.
        assert_eq!(v.resets_ms, vec![d0 + 9 * HOUR_MS]);
    }

    #[test]
    fn view_counts_the_new_window_after_an_exact_reset() {
        // 20% before a reset at +2h, 25% an hour after it: 25 points were used in the new window,
        // although no drop is visible.
        let (_dir, h) = with_rows(vec![
            exact(H0, 10.0, H0 + 2 * HOUR_MS),
            exact(H0 + HOUR_MS, 20.0, H0 + 2 * HOUR_MS),
            exact(H0 + 3 * HOUR_MS, 25.0, H0 + 7 * HOUR_MS),
        ]);
        let days = [H0];
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + DAY_MS, H0 + 4 * HOUR_MS, &days));
        assert_eq!(day_values(&v), vec![(25.0, 35.0)]);
        assert_eq!(v.resets_ms, vec![H0 + 2 * HOUR_MS]);
        // Estimated reset times (`e`) never count.
        let (_dir, est) = with_rows(vec![
            HistoryRow { e: true, ..exact(H0, 10.0, H0 + 2 * HOUR_MS) },
            HistoryRow { e: true, ..exact(H0 + 3 * HOUR_MS, 25.0, H0 + 7 * HOUR_MS) },
        ]);
        let v = est.view(&WindowKind::FiveHour, &range(H0, H0 + DAY_MS, H0 + 4 * HOUR_MS, &days));
        assert_eq!(day_values(&v), vec![(25.0, 15.0)]);
        assert!(v.resets_ms.is_empty());
    }

    #[test]
    fn view_ignores_small_dips_between_cli_and_desktop_rows() {
        // CLI rows carry one decimal, Desktop rows integers that may lag a little: 79.4 then 78 is
        // one window, not a reset followed by 78 points of fresh usage.
        let m = MINUTE_MS;
        let desktop = |t: Ms, p: f32| HistoryRow { s: Source::Desktop, ..row(t, "5h", p) };
        let (_dir, h) = with_rows(vec![
            row(H0, "5h", 75.0),
            row(H0 + 10 * m, "5h", 79.4),
            desktop(H0 + 15 * m, 78.0),
            row(H0 + 20 * m, "5h", 80.2),
            desktop(H0 + 30 * m, 80.0),
            row(H0 + 40 * m, "5h", 84.0),
            desktop(H0 + 45 * m, 41.0), // a real reset
            row(H0 + 50 * m, "5h", 43.5),
        ]);
        let days = [H0];
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + DAY_MS, H0 + HOUR_MS, &days));
        assert_eq!(v.resets_ms, vec![H0 + 45 * m]);
        // 75 → 84 is 9 points, then 43.5 in the new window.
        assert_eq!(day_values(&v), vec![(84.0, 52.5)]);
        // A dip smaller than two points still starts a new window when an exact reset explains it.
        let (_dir, h) = with_rows(vec![exact(H0, 3.0, H0 + HOUR_MS), exact(H0 + 2 * HOUR_MS, 1.5, H0 + 6 * HOUR_MS)]);
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + DAY_MS, H0 + 3 * HOUR_MS, &days));
        assert_eq!(v.resets_ms, vec![H0 + HOUR_MS]);
        assert_eq!(day_values(&v), vec![(3.0, 1.5)]);
    }

    #[test]
    fn view_infers_gap_resets_of_other_short_windows_from_their_length() {
        let days = [H0];
        let r = range(H0, H0 + DAY_MS, H0 + DAY_MS, &days);
        let kind = WindowKind::from_key("five_hour_opus");
        let (_dir, h) = with_rows(vec![row(H0, "five_hour_opus", 40.0), row(H0 + 8 * HOUR_MS, "five_hour_opus", 60.0)]);
        assert_eq!(h.view(&kind, &r).resets_ms, vec![H0 + 5 * HOUR_MS]);
        let kind = WindowKind::from_key("2_hour");
        let (_dir, h) = with_rows(vec![row(H0, "2_hour", 40.0), row(H0 + 3 * HOUR_MS, "2_hour", 60.0)]);
        assert_eq!(h.view(&kind, &r).resets_ms, vec![H0 + 2 * HOUR_MS]);
        // Longer windows keep their own schedule: a gap alone is no reset.
        let kind = WindowKind::from_key("seven_day_opus");
        let (_dir, h) = with_rows(vec![row(H0, "seven_day_opus", 40.0), row(H0 + 8 * HOUR_MS, "seven_day_opus", 60.0)]);
        assert!(h.view(&kind, &r).resets_ms.is_empty());
    }

    #[test]
    fn view_infers_a_five_hour_reset_from_a_long_gap() {
        // 40% at H0 and 60% eight hours later: that window ended by H0 + 5h, so the 60% is new.
        let days = [H0];
        let r = |from: Ms| range(from, H0 + DAY_MS, H0 + DAY_MS, &days);
        let (_dir, h) = with_rows(vec![row(H0, "5h", 40.0), row(H0 + 8 * HOUR_MS, "5h", 60.0)]);
        let v = h.view(&WindowKind::FiveHour, &r(H0));
        assert_eq!(v.resets_ms, vec![H0 + 5 * HOUR_MS]);
        assert_eq!(day_values(&v), vec![(60.0, 60.0)]);
        // Exactly five hours apart is still one window.
        let (_dir, h) = with_rows(vec![row(H0, "5h", 40.0), row(H0 + 5 * HOUR_MS, "5h", 60.0)]);
        let v = h.view(&WindowKind::FiveHour, &r(H0));
        assert!(v.resets_ms.is_empty());
        assert_eq!(day_values(&v), vec![(60.0, 20.0)]);
        // A drop across the gap is one reset, marked when the old window ended at the latest.
        let (_dir, h) = with_rows(vec![row(H0, "5h", 70.0), row(H0 + 8 * HOUR_MS, "5h", 20.0)]);
        assert_eq!(h.view(&WindowKind::FiveHour, &r(H0)).resets_ms, vec![H0 + 5 * HOUR_MS]);
        // An exact reset inside the gap is marked at its own time only.
        let (_dir, h) =
            with_rows(vec![exact(H0, 30.0, H0 + 2 * HOUR_MS), exact(H0 + 8 * HOUR_MS, 50.0, H0 + 13 * HOUR_MS)]);
        let v = h.view(&WindowKind::FiveHour, &range(H0, H0 + DAY_MS, H0 + 9 * HOUR_MS, &days));
        assert_eq!(v.resets_ms, vec![H0 + 2 * HOUR_MS]);
        assert_eq!(day_values(&v), vec![(50.0, 50.0)]);
        // No window was running at 0%: nothing to mark, the new one counts in full.
        let (_dir, h) = with_rows(vec![row(H0, "5h", 0.0), row(H0 + 8 * HOUR_MS, "5h", 30.0)]);
        let v = h.view(&WindowKind::FiveHour, &r(H0));
        assert!(v.resets_ms.is_empty());
        assert_eq!(day_values(&v), vec![(30.0, 30.0)]);
        // Only in range: the gap mark of a window that ended before `from_ms` is dropped.
        let (_dir, h) = with_rows(vec![row(H0, "5h", 40.0), row(H0 + 8 * HOUR_MS, "5h", 60.0)]);
        assert!(h.view(&WindowKind::FiveHour, &r(H0 + 6 * HOUR_MS)).resets_ms.is_empty());
        // Weekly windows follow their own schedule: a long gap alone is no reset.
        let (_dir, h) = with_rows(vec![row(H0, "7d", 40.0), row(H0 + 8 * HOUR_MS, "7d", 60.0)]);
        let v = h.view(&WindowKind::SevenDay, &r(H0));
        assert!(v.resets_ms.is_empty());
        assert_eq!(day_values(&v), vec![(60.0, 20.0)]);
    }

    #[test]
    fn view_counts_rows_per_day() {
        let d0 = H0;
        let d1 = d0 + DAY_MS;
        let d2 = d1 + DAY_MS;
        let (_dir, h) = with_rows(vec![
            row(d0 - HOUR_MS, "7d", 10.0), // before the range: seeds the walk only
            row(d0 + HOUR_MS, "7d", 10.0),
            row(d0 + 2 * HOUR_MS, "7d", 10.0), // unchanged value: a real 0% day so far
            row(d0 + 3 * HOUR_MS, "5h", 50.0), // other windows are not counted
            row(d2 + HOUR_MS, "7d", 12.0),
        ]);
        let days = [d0, d1, d2];
        let v = h.view(&WindowKind::SevenDay, &range(d0, d2 + DAY_MS, d2 + DAY_MS, &days));
        let samples: Vec<u32> = v.days.iter().map(|d| d.samples).collect();
        assert_eq!(samples, vec![2, 0, 1]);
        assert_eq!(day_values(&v), vec![(10.0, 0.0), (0.0, 0.0), (12.0, 2.0)]);
    }

    #[test]
    fn view_uses_the_given_day_boundaries() {
        // A 23 h day (spring forward) followed by a 25 h day (fall back).
        let d0 = H0;
        let d1 = d0 + 23 * HOUR_MS;
        let d2 = d1 + 25 * HOUR_MS;
        let (_dir, h) = with_rows(vec![
            row(d0 + HOUR_MS, "7d", 10.0),
            row(d1 - 30 * MINUTE_MS, "7d", 12.0), // last half hour of the short day
            row(d1, "7d", 15.0),                  // midnight belongs to the new day
            row(d2 - 30 * MINUTE_MS, "7d", 19.0), // 24.5 h into the long day
            row(d2 + HOUR_MS, "7d", 20.0),
            row(d2 + 2 * HOUR_MS, "7d", 60.0), // after to_ms: ignored
        ]);
        let days = [d0, d1, d2];
        let to = d2 + HOUR_MS;
        let v = h.view(&WindowKind::SevenDay, &range(d0, to, to, &days));
        assert_eq!(day_values(&v), vec![(12.0, 2.0), (19.0, 7.0), (20.0, 1.0)]);
        let starts: Vec<Ms> = v.days.iter().map(|d| d.day_start_ms).collect();
        assert_eq!(starts, days);
    }

    #[test]
    fn view_handles_many_rows() {
        // Two weeks of 1-minute rows for two windows (about the size of a busy history file).
        let rows: Vec<HistoryRow> = (0..20_160)
            .flat_map(|i| {
                let t = H0 + i * MINUTE_MS;
                let five = (i % 300) as f32 / 3.0; // 0 → 99.7 every 5 h
                [row(t, "5h", five), row(t, "7d", (i / 200) as f32 / 1.1)]
            })
            .collect();
        let (_dir, h) = with_rows(rows);
        let to = H0 + 14 * DAY_MS;
        let days: Vec<Ms> = (0..14).map(|d| H0 + d * DAY_MS).collect();
        let v = h.view(&WindowKind::FiveHour, &range(H0, to, to, &days));
        assert_eq!(v.points.len(), 14 * 24);
        assert_eq!(v.resets_ms.len(), 67, "a drop every 5 h after the first window");
        let total: f32 = v.days.iter().map(|d| d.consumed_pct).sum();
        // 67 full windows rising 0 → 99.67, then a partial one up to 19.67.
        let expected = 67.0 * (299.0 / 3.0) + 59.0 / 3.0;
        assert!((total - expected).abs() < 1.0, "{total} vs {expected}");
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
