//! `get_history` command: aggregates the local history into the History view.
//! Shapes mirror `HistoryData` in `src/lib/types.ts`.
//!
//! - The command runs on a blocking worker (never the main thread) and only reads: it opens its
//!   own copy of `history.jsonl` (the pipeline may append meanwhile; a torn last line is skipped)
//!   and aggregates each window with `History::view`.
//! - Range: `to_ms` is now rounded up to the next local hour, `from_ms = to_ms - days × 24 h`
//!   (`days` clamped to 1..=[`MAX_DAYS`]), so the hourly points line up with the local clock.
//! - Days: every local calendar day touching the range, oldest first, as local midnights from
//!   `chrono::Local` (DST days are 23 h or 25 h long; a midnight inside a DST gap moves to the
//!   first valid time of that day).

use std::sync::Arc;

use chrono::TimeZone;
use mikyas_core::engine::types::{SparkPoint, WindowKind};
use mikyas_core::history::{DayUsage, History, ViewRange, WindowHistory};
use mikyas_core::time::{DAY_MS, Ms, now_ms};
use serde::Serialize;
use tauri::State;

use crate::localtime::{local_day_starts, next_local_hour};
use crate::state::Shared;

/// Longest range the view asks for (the history keeps 14 days).
pub const MAX_DAYS: u32 = 14;

/// One local calendar day of one window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryDay {
    pub day_start_ms: i64,
    pub peak_pct: f32,
    pub consumed_pct: f32,
    pub samples: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryWindow {
    pub kind: WindowKind,
    /// `WindowKind::label` / `short_label`, so the chart titles match the card.
    pub label: String,
    pub short: String,
    pub points: Vec<SparkPoint>,
    pub resets_ms: Vec<i64>,
    pub days: Vec<HistoryDay>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryData {
    pub from_ms: i64,
    pub to_ms: i64,
    pub windows: Vec<HistoryWindow>,
}

impl From<DayUsage> for HistoryDay {
    fn from(d: DayUsage) -> Self {
        Self { day_start_ms: d.day_start_ms, peak_pct: d.peak_pct, consumed_pct: d.consumed_pct, samples: d.samples }
    }
}

impl From<WindowHistory> for HistoryWindow {
    fn from(w: WindowHistory) -> Self {
        Self {
            label: w.kind.label(),
            short: w.kind.short_label(),
            kind: w.kind,
            points: w.points,
            resets_ms: w.resets_ms,
            days: w.days.into_iter().map(HistoryDay::from).collect(),
        }
    }
}

#[tauri::command]
pub async fn get_history(shared: State<'_, Arc<Shared>>, days: u32) -> Result<HistoryData, String> {
    let path = shared.paths.history_file();
    tauri::async_runtime::spawn_blocking(move || {
        let history = History::open(path).map_err(|e| format!("Couldn't read the usage history: {e}"))?;
        Ok(build(&history, days, now_ms(), &chrono::Local))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The view data for the last `days` days as seen at `now_ms` in time zone `tz`.
pub fn build<Tz: TimeZone>(history: &History, days: u32, now_ms: Ms, tz: &Tz) -> HistoryData {
    let days = days.clamp(1, MAX_DAYS);
    let to_ms = next_local_hour(now_ms, tz);
    let from_ms = to_ms - i64::from(days) * DAY_MS;
    let day_starts = local_day_starts(from_ms, to_ms - 1, MAX_DAYS as usize, tz);
    let range = ViewRange { from_ms, to_ms, now_ms, day_starts: &day_starts };
    let windows = history.kinds().iter().map(|kind| HistoryWindow::from(history.view(kind, &range))).collect();
    HistoryData { from_ms, to_ms, windows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use mikyas_core::engine::types::{Phase, ResetInfo, Source, WindowState};
    use mikyas_core::time::{HOUR_MS, MINUTE_MS};

    /// 2026-09-24T10:17:00Z.
    const NOW: Ms = 1_790_208_000_000 + 10 * HOUR_MS + 17 * MINUTE_MS;

    fn history_with(rows: &[(Ms, WindowKind, f32)]) -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::open(dir.path().join("history.jsonl")).unwrap();
        for (t, kind, pct) in rows {
            let state = WindowState {
                kind: kind.clone(),
                pct: *pct,
                reset: ResetInfo::Unknown,
                source: Source::Cli,
                observed_at_ms: *t,
                stale: false,
                limit_reached: false,
                phase: Phase::Active,
            };
            h.record(&state).unwrap();
        }
        (dir, h)
    }

    #[test]
    fn range_is_hour_aligned_and_days_are_clamped() {
        let (_dir, h) = history_with(&[]);
        let d = build(&h, 7, NOW, &Utc);
        assert_eq!(d.to_ms, 1_790_208_000_000 + 11 * HOUR_MS);
        assert_eq!(d.to_ms - d.from_ms, 7 * DAY_MS);
        assert!(d.windows.is_empty());
        let one = build(&h, 0, NOW, &Utc);
        assert_eq!(one.to_ms - one.from_ms, DAY_MS);
        let wide = build(&h, 400, NOW, &Utc);
        assert_eq!(wide.to_ms - wide.from_ms, 14 * DAY_MS);
        // Exactly on the hour still rounds up to the next one.
        assert_eq!(build(&h, 1, 1_790_208_000_000, &Utc).to_ms, 1_790_208_000_000 + HOUR_MS);
    }

    #[test]
    fn days_cover_every_local_date_in_the_range() {
        let (_dir, h) = history_with(&[
            (NOW - 30 * HOUR_MS, WindowKind::SevenDay, 40.0),
            (NOW - 2 * HOUR_MS, WindowKind::SevenDay, 44.0),
            (NOW - HOUR_MS, WindowKind::FiveHour, 12.0),
        ]);
        let d = build(&h, 2, NOW, &Utc);
        let kinds: Vec<&WindowKind> = d.windows.iter().map(|w| &w.kind).collect();
        assert_eq!(kinds, vec![&WindowKind::FiveHour, &WindowKind::SevenDay]);
        let weekly = &d.windows[1];
        assert_eq!(weekly.points.len(), 48);
        // 22 Sep 11:00Z .. 24 Sep 11:00Z touches three UTC dates.
        let starts: Vec<Ms> = weekly.days.iter().map(|d| d.day_start_ms).collect();
        let midnight = 1_790_208_000_000;
        assert_eq!(starts, vec![midnight - 2 * DAY_MS, midnight - DAY_MS, midnight]);
        assert_eq!(weekly.days[2].consumed_pct, 4.0);
        assert_eq!(weekly.days[2].peak_pct, 44.0);
        let samples: Vec<u32> = weekly.days.iter().map(|d| d.samples).collect();
        assert_eq!(samples, vec![0, 1, 1], "a day without rows says so");
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["windows"][1]["kind"], "seven_day");
        assert!(json["windows"][0]["points"][0].get("t_ms").is_some());
        assert!(json["windows"][0]["days"][0].get("consumed_pct").is_some());
        assert_eq!(json["windows"][1]["days"][1]["samples"], 1);
    }

    #[test]
    fn windows_carry_the_core_names() {
        let (_dir, h) = history_with(&[
            (NOW - HOUR_MS, WindowKind::SevenDay, 40.0),
            (NOW - HOUR_MS, WindowKind::from_key("thirty_day"), 12.0),
        ]);
        let json = serde_json::to_value(build(&h, 2, NOW, &Utc)).unwrap();
        assert_eq!(json["windows"][0]["label"], "weekly");
        assert_eq!(json["windows"][0]["short"], "7d");
        assert_eq!(json["windows"][1]["kind"], "thirty_day");
        assert_eq!(json["windows"][1]["label"], "30-day");
        assert_eq!(json["windows"][1]["short"], "30d");
    }

    #[test]
    fn get_history_reads_many_rows() {
        // About two weeks of busy use.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut text = String::new();
        for i in 0..20_000_i64 {
            let t = NOW - 14 * DAY_MS + i * MINUTE_MS;
            let (w, p) = if i % 2 == 0 { ("5h", (i % 600) as f32 / 6.0) } else { ("7d", (i / 300) as f32) };
            text.push_str(&format!("{{\"t\":{t},\"w\":\"{w}\",\"p\":{p:.1},\"r\":null,\"s\":\"cli\",\"e\":false}}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let start = std::time::Instant::now();
        let h = History::open(path).unwrap();
        let d = build(&h, 14, NOW, &Utc);
        let elapsed = start.elapsed();
        assert_eq!(h.rows().len(), 20_000);
        assert_eq!(d.windows.len(), 2);
        assert_eq!(d.windows[0].points.len(), 14 * 24);
        assert_eq!(d.windows[0].days.len(), 15);
        // Timing only means something in optimised builds (and even then only as a smoke check:
        // a busy CI machine can be slow).
        if !cfg!(debug_assertions) {
            assert!(elapsed.as_secs() < 10, "{elapsed:?}");
        }
    }
}
