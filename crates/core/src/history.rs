//! Local usage history (`<data_root>/history.jsonl`) for sparklines, burn rate and reset estimation.
//!
//! One JSON object per line: `{"t":<ms>,"w":"5h","p":22.0,"r":<ms>|null,"s":"cli"|"desktop","e":false}`
//! (`w` = [`WindowKind::short`], `r` = reset at_ms if known, `e` = reset was estimated).
//! Malformed lines (e.g. a torn last line after a crash) are skipped on load. Rows are kept sorted
//! by `t`. Retention: [`RETAIN_MS`].

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::types::{Sample, Source, SparkPoint, WindowKind, WindowState};
use crate::sources::desktop_usage::DesktopUsage;
use crate::time::{DAY_MS, HOUR_MS, Ms};

pub const RETAIN_MS: Ms = 14 * DAY_MS;
/// A sparkline bucket with no sample carries the previous value forward only if that value is
/// at most this old; otherwise the bucket is a gap (`pct: None`).
pub const SPARK_MAX_CARRY_MS: Ms = 2 * HOUR_MS;
/// Values closer than this are "unchanged" for [`History::record`].
pub const CHANGE_EPSILON: f32 = 0.05;

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

#[derive(Debug)]
pub struct History {
    path: PathBuf,
    rows: Vec<HistoryRow>,
}

impl History {
    /// Loads `path` if it exists (missing file → empty history; parent dirs created on first write).
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let _ = path;
        todo!("History::open")
    }

    pub fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }

    /// Appends a row for `state` (t = `state.observed_at_ms`) if, compared with the newest row of
    /// the same window, the pct changed by more than [`CHANGE_EPSILON`] or the reset `at_ms`
    /// changed — and `t` is newer than that row. ResetAwaitingData states are not recorded.
    /// Appends to the file (open in append mode, one line + `\n`). Returns whether it appended.
    pub fn record(&mut self, state: &WindowState) -> io::Result<bool> {
        let _ = state;
        todo!("History::record")
    }

    /// Adds Desktop samples with `t > watermark_ms` as rows (`s = desktop`, `r = None`), skipping
    /// any whose (window, t) already exists, keeps rows sorted, and rewrites the file atomically if
    /// anything was added. Returns the new watermark (max sample t seen, or the old one).
    pub fn backfill_desktop(&mut self, usage: &DesktopUsage, watermark_ms: Ms) -> io::Result<Ms> {
        let _ = (usage, watermark_ms);
        todo!("History::backfill_desktop")
    }

    /// Drops rows older than `now_ms - RETAIN_MS`, rewriting the file atomically (temp + rename)
    /// only if something was dropped.
    pub fn compact(&mut self, now_ms: Ms) -> io::Result<()> {
        let _ = now_ms;
        todo!("History::compact")
    }

    /// Rows of `kind` with `t >= since_ms`, as samples sorted ascending.
    pub fn samples(&self, kind: &WindowKind, since_ms: Ms) -> Vec<Sample> {
        let _ = (kind, since_ms);
        todo!("History::samples")
    }

    /// `buckets` evenly spaced points over `[from_ms, to_ms]`. Each bucket's value is the MAX pct of
    /// rows inside it (peaks and resets stay visible); an empty bucket carries the previous value
    /// forward if the last row is ≤ [`SPARK_MAX_CARRY_MS`] old at the bucket start, otherwise
    /// `pct: None` (gap). `t_ms` is the bucket start. Returns an empty vec if `buckets == 0` or the
    /// range is empty.
    pub fn spark(&self, kind: &WindowKind, from_ms: Ms, to_ms: Ms, buckets: usize) -> Vec<SparkPoint> {
        let _ = (kind, from_ms, to_ms, buckets);
        todo!("History::spark")
    }
}
