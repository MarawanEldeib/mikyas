//! Weekly recap: one summary when the weekly window resets.
//!
//! Trigger: a weekly (`SevenDay`) window instance ends — its exact reset time passed, or the pct
//! dropped by >= 2 points (the same reset rule as `History::view`). The recap covers the ended
//! window [start, end), where start is the previous window's end (weekly resets are sometimes days
//! apart), at most 7 days before `end`: `used_pct` = the highest weekly % seen in it, `busiest_day` = the local day
//! with the largest weekly `consumed_pct` (from `History::view` day buckets), `five_hour_resets` =
//! five_hour resets inside it, `peak_five_hour_pct` = the highest 5-hour % in it.
//! Fires once per ended window (persisted `last_recapped_end_ms`); if the app was closed at the
//! reset, the next start within `CATCH_UP_MS` (24 h) after it still shows it once; older → skip.
//! No recap for a window with fewer than `MIN_SAMPLES` weekly history rows (not enough data).
//!
//! The pipeline calls [`RecapState::evaluate`] on every tick; the history walk is repeated only
//! when the history changed or [`RECHECK_MS`] passed since the last walk (an exact reset time
//! passing is the only thing that can end a week without a new row, and a recap a minute late is
//! fine).

use serde::{Deserialize, Serialize};

use crate::alerts::WEEKLY_INSTANCE_ALIAS_MS;
use crate::engine::types::{WindowKind, is_reset_drop};
use crate::history::{History, HistoryRow, ViewRange};
use crate::time::{DAY_MS, MINUTE_MS, Ms, SEVEN_DAYS_MS};

pub const CATCH_UP_MS: Ms = DAY_MS;
pub const MIN_SAMPLES: usize = 6;
/// With an unchanged history, the weekly rows are walked again at most this often.
pub const RECHECK_MS: Ms = MINUTE_MS;

#[derive(Debug, Clone, PartialEq)]
pub struct WeeklyRecap {
    pub window_end_ms: Ms,
    pub used_pct: f32,
    /// Local midnight of the busiest day and that day's weekly consumed %.
    pub busiest_day: Option<(Ms, f32)>,
    pub five_hour_resets: u32,
    pub peak_five_hour_pct: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecapState {
    pub last_recapped_end_ms: Option<Ms>,
    /// The last history walk (in memory only; not part of the state's value).
    #[serde(skip)]
    checked: Option<Checked>,
}

/// What the last walk saw: the history's row count and newest row time, and when it ran.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Checked {
    rows: usize,
    last_t: Option<Ms>,
    at_ms: Ms,
}

impl PartialEq for RecapState {
    fn eq(&self, other: &Self) -> bool {
        self.last_recapped_end_ms == other.last_recapped_end_ms
    }
}

impl RecapState {
    /// `day_starts`: local midnights covering at least the last 8 days (the caller computes them
    /// in local time). An end within [`WEEKLY_INSTANCE_ALIAS_MS`] of the last recapped one is the
    /// same reset seen twice (e.g. the exact time, then the drop), so it does not repeat.
    pub fn evaluate(&mut self, history: &History, day_starts: &[Ms], now_ms: Ms) -> Option<WeeklyRecap> {
        let rows = history.rows();
        let seen = Checked { rows: rows.len(), last_t: rows.last().map(|r| r.t), at_ms: now_ms };
        let unchanged = self.checked.is_some_and(|c| {
            c.rows == seen.rows && c.last_t == seen.last_t && (0..RECHECK_MS).contains(&now_ms.saturating_sub(c.at_ms))
        });
        if unchanged {
            return None;
        }
        self.checked = Some(seen);
        let week = last_ended_week(rows, now_ms)?;
        let end = week.end_ms;
        let new = self.last_recapped_end_ms.is_none_or(|last| end.saturating_sub(last) > WEEKLY_INSTANCE_ALIAS_MS);
        if !new || now_ms.saturating_sub(end) > CATCH_UP_MS {
            return None;
        }
        self.last_recapped_end_ms = Some(end);

        let start = week.start_ms;
        let range = ViewRange { from_ms: start, to_ms: end - 1, now_ms, day_starts: &[] };
        // Days overlapping [start, end); the first one starts at `start`, so the previous week's
        // rows on that day are not counted.
        let first = day_starts.partition_point(|&d| d <= start).saturating_sub(1);
        let midnights: Vec<Ms> = day_starts[first..].iter().copied().take_while(|&d| d < end).collect();
        let mut bounds = midnights.clone();
        if let Some(b) = bounds.first_mut() {
            *b = (*b).max(start);
        }
        let weekly = history.view(&WindowKind::SevenDay, &ViewRange { day_starts: &bounds, ..range });
        let busiest_day = weekly.days.iter().zip(&midnights).filter(|(d, _)| d.consumed_pct > 0.0).fold(
            None::<(Ms, f32)>,
            |best, (d, &midnight)| match best {
                Some((_, pct)) if pct >= d.consumed_pct => best,
                _ => Some((midnight, d.consumed_pct)),
            },
        );
        let five_hour = history.view(&WindowKind::FiveHour, &range);
        let peak_five_hour_pct = history
            .samples(&WindowKind::FiveHour, start)
            .iter()
            .take_while(|s| s.t_ms < end)
            .fold(0.0_f32, |max, s| max.max(s.pct));
        Some(WeeklyRecap {
            window_end_ms: end,
            used_pct: week.used_pct,
            busiest_day,
            five_hour_resets: u32::try_from(five_hour.resets_ms.len()).unwrap_or(u32::MAX),
            peak_five_hour_pct,
        })
    }
}

/// The newest weekly window with at least [`MIN_SAMPLES`] rows that ended by `now_ms`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EndedWeek {
    /// The previous window's end, or 7 days before `end_ms` if that is later (or unknown).
    start_ms: Ms,
    end_ms: Ms,
    /// Highest % of the window's rows within [`start_ms`, `end_ms`).
    used_pct: f32,
    /// Number of those rows.
    samples: usize,
}

/// Walks the weekly rows with the reset rule of `History::view`: a row starts a new window when
/// its pct is at least [`crate::history::MIXED_SOURCE_DROP_PCT`] below the previous row (the window ended at that
/// row), or when the latest exact reset time known passed since the previous row (it ended then).
/// An exact reset time that passed with no row after it ends the current window too. A window
/// with too few rows is passed over, so a Desktop drop just before the exact reset time (a
/// one-row "window" between them) does not hide the week that ended with the drop.
fn last_ended_week(rows: &[HistoryRow], now_ms: Ms) -> Option<EndedWeek> {
    let weekly: Vec<&HistoryRow> =
        rows.iter().take_while(|r| r.t <= now_ms).filter(|r| r.is_kind(&WindowKind::SevenDay)).collect();
    let summarize = |window: &[&HistoryRow], prev_end: Option<Ms>, end_ms: Ms| {
        let start_ms = end_ms.saturating_sub(SEVEN_DAYS_MS).max(prev_end.unwrap_or(Ms::MIN));
        let inside = window.iter().filter(|r| r.t >= start_ms && r.t < end_ms);
        let (samples, used_pct) = inside.fold((0, 0.0_f32), |(n, max), r| (n + 1, max.max(r.p)));
        EndedWeek { start_ms, end_ms, used_pct, samples }
    };

    let mut ended = None;
    let mut window_start = 0;
    let mut prev_end: Option<Ms> = None;
    let mut known_reset: Option<Ms> = None;
    for (i, row) in weekly.iter().enumerate() {
        if let Some(prev) = i.checked_sub(1).map(|p| weekly[p]) {
            let exact = known_reset.filter(|&r| prev.t < r && r <= row.t);
            let dropped = is_reset_drop(prev.p, row.p);
            if let Some(end_ms) = exact.or(dropped.then_some(row.t)) {
                let week = summarize(&weekly[window_start..i], prev_end, end_ms);
                ended = Some(week).filter(|w| w.samples >= MIN_SAMPLES).or(ended);
                window_start = i;
                prev_end = Some(end_ms);
            }
        }
        // A newer exact time replaces the known one (one that had not passed was not a reset).
        if let Some(r) = row.r.filter(|_| !row.e) {
            known_reset = Some(r);
        }
    }
    let last_t = weekly.last().map(|r| r.t);
    if let Some(r) = known_reset.filter(|&r| last_t.is_some_and(|t| t < r) && r <= now_ms) {
        let week = summarize(&weekly[window_start..], prev_end, r);
        ended = Some(week).filter(|w| w.samples >= MIN_SAMPLES).or(ended);
    }
    ended
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{HOUR_MS, MINUTE_MS, SECOND_MS};
    use pretty_assertions::assert_eq;

    /// 2026-09-24T00:00Z. The tests use UTC midnights as the "local" days.
    const T0: Ms = 1_790_208_000_000;
    /// Midnight of day `i` of the recapped week (day 0 holds its start).
    const fn day(i: i64) -> Ms {
        T0 - 7 * DAY_MS + i * DAY_MS
    }
    const END: Ms = T0 + 10 * HOUR_MS;
    const START: Ms = END - SEVEN_DAYS_MS;

    /// `(t, window, pct, exact reset)`; rows without a reset are Desktop rows.
    type Row = (Ms, &'static str, f32, Option<Ms>);

    fn history(rows: &[Row]) -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let text: String = rows
            .iter()
            .map(|&(t, w, p, r)| {
                let (r, s) = r.map_or(("null".to_owned(), "desktop"), |r| (r.to_string(), "cli"));
                format!("{{\"t\":{t},\"w\":\"{w}\",\"p\":{p},\"r\":{r},\"s\":\"{s}\",\"e\":false}}\n")
            })
            .collect();
        std::fs::write(&path, text).unwrap();
        let history = History::open(path).unwrap();
        (dir, history)
    }

    /// Local midnights of the 8 days before `T0` and `T0` itself.
    fn day_starts() -> Vec<Ms> {
        (-1..=7).map(day).collect()
    }

    /// A week that ends at its exact reset time `END`, after the previous one ended at `START`,
    /// with a busy Tuesday (day 1) and two five-hour resets inside it.
    fn week() -> Vec<Row> {
        let prev = Some(START);
        let this = Some(END);
        vec![
            // The previous week, on the same calendar day as the start.
            (day(0) + 2 * HOUR_MS, "7d", 50.0, prev),
            (day(0) + 3 * HOUR_MS, "5h", 99.0, None),
            (day(0) + 4 * HOUR_MS, "7d", 90.0, prev),
            // This week: 10 on day 0, 35 on day 1, then 15, 10 and 12.
            (day(0) + 12 * HOUR_MS, "7d", 5.0, this),
            (day(0) + 13 * HOUR_MS, "7d", 10.0, this),
            (day(1) + 9 * HOUR_MS, "7d", 20.0, this),
            (day(1) + 9 * HOUR_MS, "5h", 40.0, None),
            (day(1) + 11 * HOUR_MS, "5h", 100.0, None),
            (day(1) + 14 * HOUR_MS, "5h", 3.0, None),
            (day(1) + 15 * HOUR_MS, "7d", 45.0, this),
            (day(1) + 15 * HOUR_MS, "5h", 30.0, None),
            (day(2) + 10 * HOUR_MS, "7d", 60.0, this),
            (day(2) + 10 * HOUR_MS, "5h", 20.0, None),
            (day(2) + 11 * HOUR_MS, "5h", 60.0, None),
            (day(3) + 10 * HOUR_MS, "7d", 70.0, this),
            (day(5) + 10 * HOUR_MS, "7d", 82.0, this),
            (day(6) + 20 * HOUR_MS, "7d", 81.5, this),
        ]
    }

    #[test]
    fn exact_reset_ends_the_week_without_a_later_row() {
        let (_d, h) = history(&week());
        let mut s = RecapState::default();
        assert_eq!(s.evaluate(&h, &day_starts(), END - MINUTE_MS), None, "not over yet");
        let recap = s.evaluate(&h, &day_starts(), END + HOUR_MS);
        assert_eq!(
            recap,
            Some(WeeklyRecap {
                window_end_ms: END,
                used_pct: 82.0,
                busiest_day: Some((day(1), 35.0)),
                // A drop on day 1, a five-hour gap into day 2; the previous week's window does
                // not count.
                five_hour_resets: 2,
                peak_five_hour_pct: 100.0,
            })
        );
        assert_eq!(s.last_recapped_end_ms, Some(END));
        assert_eq!(s.evaluate(&h, &day_starts(), END + 2 * HOUR_MS), None, "once");
        // The week before was recapped already: this one still is.
        let mut s = RecapState { last_recapped_end_ms: Some(START), ..RecapState::default() };
        assert_eq!(s.evaluate(&h, &day_starts(), END + HOUR_MS).map(|r| r.window_end_ms), Some(END));
    }

    #[test]
    fn a_drop_just_before_the_exact_reset_keeps_its_week() {
        // Desktop shows the drop first; the CLI row after the exact time ends a one-row window.
        let mut rows: Vec<Row> = week().into_iter().filter(|r| r.1 == "7d" && r.0 >= START).collect();
        rows.push((END - 20 * MINUTE_MS, "7d", 0.0, None));
        rows.push((END + 10 * MINUTE_MS, "7d", 1.0, Some(END + SEVEN_DAYS_MS)));
        let (_d, h) = history(&rows);
        let mut s = RecapState::default();
        let recap = s.evaluate(&h, &day_starts(), END + HOUR_MS);
        assert_eq!(recap.map(|r| (r.window_end_ms, r.used_pct)), Some((END - 20 * MINUTE_MS, 82.0)));
        assert_eq!(s.evaluate(&h, &day_starts(), END + 2 * HOUR_MS), None);
    }

    #[test]
    fn a_new_row_after_the_reset_is_the_same_end() {
        let mut rows = week();
        rows.push((END + 30 * MINUTE_MS, "7d", 1.0, Some(END + SEVEN_DAYS_MS)));
        let (_d, h) = history(&rows);
        let mut s = RecapState::default();
        assert_eq!(s.evaluate(&h, &day_starts(), END + HOUR_MS).map(|r| r.window_end_ms), Some(END));
        assert_eq!(s.evaluate(&h, &day_starts(), END + 2 * HOUR_MS), None);
    }

    #[test]
    fn catch_up_within_a_day_after_the_reset() {
        let (_d, h) = history(&week());
        let at = |now: Ms| RecapState::default().evaluate(&h, &day_starts(), now).map(|r| r.window_end_ms);
        assert_eq!(at(END), Some(END));
        assert_eq!(at(END + CATCH_UP_MS), Some(END));
        assert_eq!(at(END + CATCH_UP_MS + 1), None, "too old: skipped");
    }

    #[test]
    fn a_drop_ends_a_week_without_reset_times() {
        // Desktop only: no reset times, the week ends at the row showing the drop.
        let drop_at = day(6) + 9 * HOUR_MS;
        let mut rows: Vec<Row> = [5.0, 10.0, 20.0, 45.0, 60.0, 70.0]
            .iter()
            .enumerate()
            .map(|(i, &p)| (day(i as i64) + 12 * HOUR_MS, "7d", p, None))
            .collect();
        rows.push((drop_at, "7d", 1.0, None));
        let (_d, h) = history(&rows);
        let mut s = RecapState::default();
        let recap = s.evaluate(&h, &day_starts(), drop_at + 5 * MINUTE_MS).expect("recap");
        assert_eq!((recap.window_end_ms, recap.used_pct), (drop_at, 70.0));
        assert_eq!(recap.busiest_day, Some((day(3), 25.0)));
        assert_eq!((recap.five_hour_resets, recap.peak_five_hour_pct), (0, 0.0));
        // The same reset seen again a little later (an exact time, a second drop) is not a new week.
        let mut s = RecapState { last_recapped_end_ms: Some(drop_at - 2 * HOUR_MS), ..RecapState::default() };
        assert_eq!(s.evaluate(&h, &day_starts(), drop_at + 5 * MINUTE_MS), None);
        assert_eq!(s.last_recapped_end_ms, Some(drop_at - 2 * HOUR_MS));
    }

    #[test]
    fn an_early_reset_limits_the_recap_to_its_own_window() {
        // Weekly resets are sometimes days apart: the previous window ended with a drop on day 2,
        // after a busy day 1 with a five-hour reset. The recap of the window after it (ended by a
        // drop on day 6) must not count them.
        let mut rows: Vec<Row> = vec![
            (day(0) + 12 * HOUR_MS, "7d", 10.0, None),
            (day(1) + 9 * HOUR_MS, "7d", 20.0, None),
            (day(1) + 9 * HOUR_MS, "5h", 90.0, None),
            (day(1) + 12 * HOUR_MS, "5h", 5.0, None),
            (day(1) + 20 * HOUR_MS, "7d", 70.0, None),
        ];
        let early = day(2) + 8 * HOUR_MS;
        for (i, p) in [2.0, 5.0, 9.0, 12.0, 15.0, 18.0].into_iter().enumerate() {
            rows.push((early + i as i64 * 12 * HOUR_MS, "7d", p, None));
        }
        let end = day(6) + 9 * HOUR_MS;
        rows.push((end, "7d", 1.0, None));
        let (_d, h) = history(&rows);
        let recap = RecapState::default().evaluate(&h, &day_starts(), end + MINUTE_MS).expect("recap");
        assert_eq!((recap.window_end_ms, recap.used_pct), (end, 18.0));
        assert_eq!(recap.busiest_day, Some((day(3), 7.0)), "day 1 (60%) was the previous window");
        assert_eq!((recap.five_hour_resets, recap.peak_five_hour_pct), (0, 0.0));
    }

    #[test]
    fn busiest_day_follows_uneven_local_days() {
        // A 25 h day (daylight time ends) and a 23 h day: rows are bucketed by the given midnights.
        let starts: Vec<Ms> = vec![day(-1), day(0), day(1) + HOUR_MS, day(2), day(3), day(4), day(5), day(6), day(7)];
        let rows: Vec<Row> = vec![
            (day(0) + 12 * HOUR_MS, "7d", 5.0, Some(END)),
            (day(1) + 30 * MINUTE_MS, "7d", 30.0, Some(END)),
            (day(1) + 2 * HOUR_MS, "7d", 40.0, Some(END)),
            (day(2) + 30 * MINUTE_MS, "7d", 50.0, Some(END)),
            (day(3) + HOUR_MS, "7d", 55.0, Some(END)),
            (day(4) + HOUR_MS, "7d", 60.0, Some(END)),
        ];
        let (_d, h) = history(&rows);
        let recap = RecapState::default().evaluate(&h, &starts, END).expect("recap");
        // 00:30 on day 1 still belongs to the long day 0: 25 there, 10 on day 1.
        assert_eq!(recap.busiest_day, Some((day(0), 25.0)));
    }

    #[test]
    fn small_dips_are_not_a_reset() {
        // Desktop integers after a CLI value: 79.4 then 78 is still one week.
        let rows: Vec<Row> = [10.0, 30.0, 50.0, 79.4, 78.0, 79.0, 80.0]
            .iter()
            .enumerate()
            .map(|(i, &p)| (day(0) + 12 * HOUR_MS + i as i64 * 10 * HOUR_MS, "7d", p, None))
            .collect();
        let (_d, h) = history(&rows);
        assert_eq!(RecapState::default().evaluate(&h, &day_starts(), day(4)), None);
    }

    #[test]
    fn a_superseded_reset_time_is_not_an_end() {
        // Six rows expecting a reset on day 1, then a newer exact time a week later.
        let first = day(1);
        let mut rows: Vec<Row> =
            (0..6).map(|i| (day(0) + (12 + i) * HOUR_MS, "7d", 10.0 + i as f32, Some(first))).collect();
        rows.push((day(0) + 20 * HOUR_MS, "7d", 16.0, Some(first + SEVEN_DAYS_MS)));
        let (_d, h) = history(&rows);
        assert_eq!(RecapState::default().evaluate(&h, &day_starts(), first + 2 * HOUR_MS), None);
        // Without the move, the first time passing ends the week.
        rows.pop();
        let (_d, h) = history(&rows);
        let recap = RecapState::default().evaluate(&h, &day_starts(), first + 2 * HOUR_MS);
        assert_eq!(recap.map(|r| (r.window_end_ms, r.used_pct)), Some((first, 15.0)));
    }

    #[test]
    fn too_few_samples_skip_the_recap() {
        let rows: Vec<Row> = (0..MIN_SAMPLES as i64 - 1)
            .map(|i| (day(i) + 12 * HOUR_MS, "7d", 10.0 * (i + 1) as f32, Some(END)))
            .collect();
        let (_d, h) = history(&rows);
        let mut s = RecapState::default();
        assert_eq!(s.evaluate(&h, &day_starts(), END + HOUR_MS), None);
        assert_eq!(s, RecapState::default());
        let (_d, empty) = history(&[]);
        assert_eq!(s.evaluate(&empty, &day_starts(), END), None);
    }

    #[test]
    fn a_week_without_rises_has_no_busiest_day() {
        // History began at the week's high: the first row adds nothing, the rest are flat.
        let rows: Vec<Row> = (0..6).map(|i| (day(i) + 12 * HOUR_MS, "7d", 40.0, Some(END))).collect();
        let (_d, h) = history(&rows);
        let recap = RecapState::default().evaluate(&h, &day_starts(), END).expect("recap");
        assert_eq!((recap.used_pct, recap.busiest_day), (40.0, None));
        // Missing day boundaries only lose the busiest day.
        let (_d, h) = history(&week());
        let recap = RecapState::default().evaluate(&h, &[], END).expect("recap");
        assert_eq!((recap.used_pct, recap.busiest_day), (82.0, None));
    }

    #[test]
    fn an_unchanged_history_is_walked_again_only_after_a_minute() {
        let (_d, h) = history(&week());
        let mut s = RecapState::default();
        assert_eq!(s.evaluate(&h, &day_starts(), END - 10 * SECOND_MS), None, "not over yet");
        // The reset passed, but the same history was walked moments ago.
        assert_eq!(s.evaluate(&h, &day_starts(), END + 30 * SECOND_MS), None);
        let recap = s.evaluate(&h, &day_starts(), END - 10 * SECOND_MS + RECHECK_MS);
        assert_eq!(recap.map(|r| r.window_end_ms), Some(END));
        // A changed history is walked at once.
        let mut s = RecapState::default();
        assert_eq!(s.evaluate(&h, &day_starts(), END - 10 * SECOND_MS), None);
        let mut rows = week();
        rows.push((END + MINUTE_MS, "7d", 1.0, None));
        let (_d2, h2) = history(&rows);
        assert_eq!(
            s.evaluate(&h2, &day_starts(), END + 2 * MINUTE_MS - RECHECK_MS / 2).map(|r| r.window_end_ms),
            Some(END)
        );
        assert_eq!(
            s,
            RecapState { last_recapped_end_ms: Some(END), ..RecapState::default() },
            "the walk is not part of the value"
        );
    }

    #[test]
    fn state_round_trips() {
        let s = RecapState { last_recapped_end_ms: Some(END), ..RecapState::default() };
        let back: RecapState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(serde_json::from_str::<RecapState>("{}").unwrap(), RecapState::default());
    }
}
