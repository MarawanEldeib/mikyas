//! Weekly recap: one summary when the weekly window resets.
//!
//! Trigger: a weekly (`SevenDay`) window instance ends — its exact reset time passed, or the pct
//! dropped by >= 2 points (the same reset rule as `History::view`). The recap covers the ended
//! window [start, end): `used_pct` = the highest weekly % seen in it, `busiest_day` = the local day
//! with the largest weekly `consumed_pct` (from `History::view` day buckets), `five_hour_resets` =
//! five_hour resets inside it, `peak_five_hour_pct` = the highest 5-hour % in it.
//! Fires once per ended window (persisted `last_recapped_end_ms`); if the app was closed at the
//! reset, the next start within `CATCH_UP_MS` (24 h) after it still shows it once; older → skip.
//! No recap for a window with fewer than `MIN_SAMPLES` weekly history rows (not enough data).

use serde::{Deserialize, Serialize};

use crate::history::History;
use crate::time::{DAY_MS, Ms};

pub const CATCH_UP_MS: Ms = DAY_MS;
pub const MIN_SAMPLES: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct WeeklyRecap {
    pub window_end_ms: Ms,
    pub used_pct: f32,
    /// Local midnight of the busiest day and that day's weekly consumed %.
    pub busiest_day: Option<(Ms, f32)>,
    pub five_hour_resets: u32,
    pub peak_five_hour_pct: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecapState {
    pub last_recapped_end_ms: Option<Ms>,
}

impl RecapState {
    /// `day_starts`: local midnights covering at least the last 8 days (the caller computes them
    /// in local time). TODO(stream A1): implement per the module docs.
    pub fn evaluate(&mut self, history: &History, day_starts: &[Ms], now_ms: Ms) -> Option<WeeklyRecap> {
        let _ = (history, day_starts, now_ms);
        None
    }
}
