//! Burn-rate forecast: "at this pace you hit 100% at 15:40 — before the reset".
//!
//! `compute(kind, samples, state, now_ms)`:
//! - Only FiveHour and SevenDay. Returns `None` if `state.stale`, `state.phase` is
//!   ResetAwaitingData, or `state.limit_reached`.
//! - Window start = `reset.at_ms - duration` when the reset is known, else unknown (then only
//!   the lookback bound applies). Lookback = FiveHour: 60 min; SevenDay: 24 h — never reaching
//!   before the window start (samples from a previous window would fake a negative slope).
//! - Build a step function from `samples` (sorted asc; value holds until the next sample) plus
//!   the current point (`now_ms`, `state.pct`), resample every 1 min (FiveHour) / 10 min
//!   (SevenDay) over the lookback span, and fit ordinary least squares pct = a + b·t.
//! - Require: span ≥ 15 min (FiveHour) / 6 h (SevenDay), ≥ 2 distinct sample values, and
//!   slope `b` > [`MIN_SLOPE_PCT_PER_H`]. Otherwise `None`.
//! - `t100_ms = now + (100 - pct) / slope` (hours → ms).
//! - `pct_at_reset = pct + slope * (reset - now)` when the reset is known (not clamped).
//! - `hits_limit_before_reset = reset known && t100 < reset`.

use crate::engine::types::{Burn, Sample, WindowKind, WindowState};
use crate::time::Ms;

pub const MIN_SLOPE_PCT_PER_H: f32 = 0.05;

pub fn compute(kind: &WindowKind, samples: &[Sample], state: &WindowState, now_ms: Ms) -> Option<Burn> {
    let _ = (kind, samples, state, now_ms);
    todo!("burn::compute")
}
