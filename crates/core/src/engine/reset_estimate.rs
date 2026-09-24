//! Estimates reset times when no exact CLI `resets_at` is available.
//!
//! Real data (this user's Desktop file) shows why this is heuristic:
//! - Samples are ~15 min apart but gaps of hours/days happen.
//! - Many 5h resets are never observed at 0 (e.g. 73 → 13, 58 → 48): a reset plus fresh usage
//!   inside one sampling gap. So ANY decrease greater than 1 point is a reset boundary, as is a
//!   gap longer than the window, as is a rise from exactly 0.
//! - Weekly resets were observed 2–3 days apart several times (not a clean 7-day cycle), and one
//!   weekly drop was 84 → 41. So weekly estimates are always `Confidence::Low`.
//!
//! Algorithm for `estimate_reset(kind, samples, last_exact_reset_ms, now_ms)`:
//! - If `last_exact_reset_ms > now_ms` → `Exact { at_ms: last_exact_reset_ms }`.
//! - Only FiveHour / SevenDay (or `kind.duration_ms()`-known kinds) are estimated; others → Unknown.
//! - Find the start of the current window by scanning `samples` (sorted ascending, all of this
//!   kind) backwards from the newest sample `s[n]`: the first index `i` where `s[i]` begins a new
//!   window — `s[i].pct < s[i-1].pct - 1.0`, or `s[i].t - s[i-1].t > duration`, or
//!   (`s[i-1].pct == 0.0 && s[i].pct > 0.0`). A past `last_exact_reset_ms` greater than `s[i-1].t`
//!   also bounds the start from below.
//!   The window started in `(lo, hi]` with `hi = s[i].t` and
//!   `lo = max(s[i-1].t, s[i].t - duration, last_exact_reset_ms if past)`.
//!   Estimate `at = (lo + hi) / 2 + duration`, `plus_minus = (hi - lo) / 2`.
//!   Confidence: FiveHour → High if plus_minus ≤ 10 min, Medium if ≤ 45 min, else Low;
//!   weekly → Low always. For weekly, add 1 day to `plus_minus`.
//! - No boundary found within the samples (monotone since the first sample): if the first sample
//!   is older than `now - duration`, the window cannot be older than `duration`… use the first
//!   sample as `hi` with `lo = hi - duration` only when `s[0].pct > 0`; otherwise Unknown.
//! - If the last sample's pct is 0.0 → the current window hasn't started yet → Unknown.
//! - Weekly fallback: if no boundary is found but `last_exact_reset_ms` is in the past, the next
//!   reset is `last_exact_reset_ms + 7d` (Low, ±1 day) — rolled forward by whole weeks until > now.
//! - Never return an estimate earlier than `now_ms - duration` (a stale estimate is still useful:
//!   an estimate in the past signals "probably reset already" to the merge step).

use crate::engine::types::{ResetInfo, Sample, WindowKind};
use crate::time::Ms;

pub fn estimate_reset(
    kind: &WindowKind,
    samples: &[Sample],
    last_exact_reset_ms: Option<Ms>,
    now_ms: Ms,
) -> ResetInfo {
    let _ = (kind, samples, last_exact_reset_ms, now_ms);
    todo!("reset_estimate::estimate_reset")
}
