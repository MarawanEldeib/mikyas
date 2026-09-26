//! Estimates reset times when no exact CLI `resets_at` is available.
//!
//! Real data (this user's Desktop file) shows why this is heuristic:
//! - Samples are ~15 min apart but gaps of hours/days happen.
//! - Many 5h resets are never observed at 0 (e.g. 73 → 13, 58 → 48): a reset plus fresh usage
//!   inside one sampling gap. So ANY decrease of [`RESET_DROP_PCT`] (2) points or more is a reset
//!   boundary (Desktop's samples are integers, so this is every decrease beyond a 1-point dip), as
//!   is a gap longer than the window, as is a rise from exactly 0.
//! - Weekly resets were observed 2–3 days apart several times (not a clean 7-day cycle), and one
//!   weekly drop was 84 → 41. So weekly estimates are always `Confidence::Low`.
//!
//! Algorithm for `estimate_reset(kind, samples, last_exact_reset_ms, now_ms)`:
//! - If `last_exact_reset_ms > now_ms` → `Exact { at_ms: last_exact_reset_ms }`.
//! - Only FiveHour / SevenDay (or `kind.duration_ms()`-known kinds) are estimated; others → Unknown.
//! - Find the start of the current window by scanning `samples` (sorted ascending, all of this
//!   kind) backwards from the newest sample `s[n]`: the first index `i` where `s[i]` begins a new
//!   window — `s[i-1].pct - s[i].pct >= RESET_DROP_PCT`, or `s[i].t - s[i-1].t > duration`, or
//!   (`s[i-1].pct == 0.0 && s[i].pct > 0.0`). A past `last_exact_reset_ms` greater than `s[i-1].t`
//!   also bounds the start from below.
//!   The window started in `(lo, hi]` with `hi = s[i].t` and
//!   `lo = max(s[i-1].t, s[i].t - duration, last_exact_reset_ms if past)`.
//!   Estimate `at = (lo + hi) / 2 + duration`, `plus_minus = (hi - lo) / 2`.
//!   Confidence: FiveHour → High if plus_minus ≤ 10 min, Medium if ≤ 45 min, else Low;
//!   weekly → Low always. For weekly, add 1 day to `plus_minus`.
//!   A start bounded below only by a sample at 0 is at most Medium: Desktop's integer 0 hides
//!   usage below one point, so at light usage the window may already have been running then.
//! - No boundary found within the samples (monotone since the first sample): if the first sample
//!   is older than `now - duration`, the window cannot be older than `duration`… use the first
//!   sample as `hi` with `lo = hi - duration` only when `s[0].pct > 0`; otherwise Unknown.
//! - If the last sample's pct is 0.0 → the current window hasn't started yet → Unknown.
//! - Weekly fallback: if no boundary is found but `last_exact_reset_ms` is in the past, the next
//!   reset is `last_exact_reset_ms + 7d` (Low, ±1 day) — rolled forward by whole weeks until > now.
//! - An estimate in the past is returned as is, however old: it signals "probably reset already"
//!   to the merge step, and it does not move while `now_ms` advances, so repeated snapshots of
//!   unchanged data stay identical (and so do the alert keys derived from it).
//!
//! Implementation notes on the points above:
//! - A past exact reset lying in `(s[i-1].t, s[i].t]` is itself a boundary: the older sample
//!   belongs to the window that ended there.
//! - If every sample predates a past exact reset, the samples describe a window that is over:
//!   FiveHour → Unknown (the next window starts with the next message), weekly → the fallback.
//! - `plus_minus` is rounded up so the true start, anywhere in `(lo, hi]`, is always covered.
//! - The newest sample `s[n]` shows usage (pct > 0), so its window was running at `s[n].t` and
//!   began after `s[n].t - duration`: `lo` is raised to that in every case (this is the "window
//!   cannot be older than `duration`" bound above, taken from the newest sample rather than
//!   `now`, which may lie after the window ended). If the located start is not after it, a
//!   boundary went unseen — the window reset inside a sampling gap shorter than `duration` and
//!   the new one climbed past the old value — and the start is only known to lie in
//!   `(s[n].t - duration, s[n].t]`. Either way the estimate is never before the newest sample.

use std::borrow::Cow;

use crate::engine::types::{Confidence, ResetInfo, Sample, WindowKind, is_reset_drop};
use crate::time::{DAY_MS, MINUTE_MS, Ms};

pub use crate::engine::types::RESET_DROP_PCT;
/// FiveHour estimates with `plus_minus` up to this are [`Confidence::High`].
pub const HIGH_CONFIDENCE_MS: Ms = 10 * MINUTE_MS;
/// FiveHour estimates with `plus_minus` up to this are [`Confidence::Medium`]; beyond, `Low`.
pub const MEDIUM_CONFIDENCE_MS: Ms = 45 * MINUTE_MS;
/// Added to every weekly `plus_minus`: weekly resets do not follow a clean 7-day cycle.
pub const WEEKLY_EXTRA_MS: Ms = DAY_MS;

/// Estimates when the current window of `kind` resets (see the module docs).
///
/// `samples` is this window's series, ideally sorted ascending (unsorted input and non-finite
/// values are tolerated). `last_exact_reset_ms` is the newest `resets_at` Claude Code reported for
/// this window, if any.
pub fn estimate_reset(
    kind: &WindowKind,
    samples: &[Sample],
    last_exact_reset_ms: Option<Ms>,
    now_ms: Ms,
) -> ResetInfo {
    if let Some(at_ms) = last_exact_reset_ms.filter(|r| *r > now_ms) {
        return ResetInfo::Exact { at_ms };
    }
    let Some(duration) = kind.duration_ms().filter(|d| *d > 0) else {
        return ResetInfo::Unknown;
    };
    let weekly = *kind != WindowKind::FiveHour;
    // Any exact reset left at this point is in the past.
    let past_reset = last_exact_reset_ms;

    let samples = normalized(samples);
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return fallback(weekly, past_reset, duration, now_ms);
    };
    if last.pct <= 0.0 {
        return ResetInfo::Unknown;
    }
    if past_reset.is_some_and(|r| last.t_ms < r) {
        return fallback(weekly, past_reset, duration, now_ms);
    }

    let boundary = samples
        .windows(2)
        .rposition(|w| starts_window(&w[0], &w[1], duration, past_reset));
    let (lo, hi) = match boundary {
        Some(j) => {
            let (prev, cur) = (samples[j], samples[j + 1]);
            let lo = prev.t_ms.max(cur.t_ms.saturating_sub(duration));
            (bound_by_reset(lo, past_reset, cur.t_ms), cur.t_ms)
        }
        None if weekly && past_reset.is_some() => {
            return fallback(weekly, past_reset, duration, now_ms);
        }
        None if first.pct > 0.0 => {
            let hi = first.t_ms;
            (
                bound_by_reset(hi.saturating_sub(duration), past_reset, hi),
                hi,
            )
        }
        None => return ResetInfo::Unknown,
    };
    // The last sample shows usage, so its window was still running then and began after
    // `last - duration`. A located start at or before that means a boundary went unseen.
    let floor = last.t_ms.saturating_sub(duration);
    let (lo, hi) = if floor >= hi {
        (floor, last.t_ms)
    } else {
        (lo.max(floor), hi)
    };
    let zero_bound = boundary.is_some_and(|j| samples[j].pct <= 0.0 && samples[j].t_ms == lo);
    estimate(lo, hi, duration, weekly, zero_bound)
}

/// True if `cur` belongs to a later window than `prev`.
fn starts_window(prev: &Sample, cur: &Sample, duration: Ms, past_reset: Option<Ms>) -> bool {
    is_reset_drop(prev.pct, cur.pct)
        || cur.t_ms.saturating_sub(prev.t_ms) > duration
        || (prev.pct <= 0.0 && cur.pct > 0.0)
        || past_reset.is_some_and(|r| prev.t_ms < r && r <= cur.t_ms)
}

/// The window cannot have started before a known earlier reset.
fn bound_by_reset(lo: Ms, past_reset: Option<Ms>, hi: Ms) -> Ms {
    match past_reset {
        Some(r) if r <= hi => lo.max(r),
        _ => lo,
    }
}

/// Estimate for a window that started in `(lo, hi]` (`lo <= hi <= lo + duration`). `zero_bound`:
/// `lo` is the time of a sample at 0, which caps the confidence at Medium.
fn estimate(lo: Ms, hi: Ms, duration: Ms, weekly: bool, zero_bound: bool) -> ResetInfo {
    let width = hi - lo;
    let mid = lo + width / 2;
    let half = width - width / 2;
    let at_ms = mid.saturating_add(duration);
    let (plus_minus_ms, confidence) = if weekly {
        (half.saturating_add(WEEKLY_EXTRA_MS), Confidence::Low)
    } else if half <= HIGH_CONFIDENCE_MS && !zero_bound {
        (half, Confidence::High)
    } else if half <= MEDIUM_CONFIDENCE_MS {
        (half, Confidence::Medium)
    } else {
        (half, Confidence::Low)
    };
    ResetInfo::Estimated {
        at_ms,
        plus_minus_ms,
        confidence,
    }
}

/// Nothing in the samples locates the current window: weekly windows roll forward from the last
/// exact reset; anything else is unknown.
fn fallback(weekly: bool, past_reset: Option<Ms>, duration: Ms, now_ms: Ms) -> ResetInfo {
    match past_reset {
        Some(reset_ms) if weekly => {
            let mut at_ms = reset_ms.saturating_add(duration);
            if at_ms <= now_ms {
                let periods = now_ms.saturating_sub(at_ms) / duration + 1;
                at_ms = at_ms.saturating_add(periods.saturating_mul(duration));
            }
            ResetInfo::Estimated {
                at_ms,
                plus_minus_ms: WEEKLY_EXTRA_MS,
                confidence: Confidence::Low,
            }
        }
        _ => ResetInfo::Unknown,
    }
}

/// Finite samples sorted by time, borrowed when the input already is.
fn normalized(samples: &[Sample]) -> Cow<'_, [Sample]> {
    if samples.iter().all(|s| s.pct.is_finite()) && samples.is_sorted_by_key(|s| s.t_ms) {
        return Cow::Borrowed(samples);
    }
    let mut owned: Vec<Sample> = samples
        .iter()
        .filter(|s| s.pct.is_finite())
        .copied()
        .collect();
    owned.sort_by_key(|s| s.t_ms);
    Cow::Owned(owned)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    use super::*;
    use crate::sources::desktop_usage::synth;
    use crate::time::{FIVE_HOURS_MS, HOUR_MS, SEVEN_DAYS_MS};

    /// 2026-09-24T00:00:00Z.
    const T: Ms = 1_790_208_000_000;

    const fn m(n: i64) -> Ms {
        n * MINUTE_MS
    }

    fn series(points: &[(Ms, f32)]) -> Vec<Sample> {
        points
            .iter()
            .map(|&(t_ms, pct)| Sample { t_ms, pct })
            .collect()
    }

    fn est(at_ms: Ms, plus_minus_ms: Ms, confidence: Confidence) -> ResetInfo {
        ResetInfo::Estimated {
            at_ms,
            plus_minus_ms,
            confidence,
        }
    }

    fn five(samples: &[(Ms, f32)], now: Ms) -> ResetInfo {
        estimate_reset(&WindowKind::FiveHour, &series(samples), None, now)
    }

    fn week(samples: &[(Ms, f32)], exact: Option<Ms>, now: Ms) -> ResetInfo {
        estimate_reset(&WindowKind::SevenDay, &series(samples), exact, now)
    }

    #[test]
    fn exact_future_reset_wins() {
        let s = series(&[(T, 10.0), (T + m(15), 0.0)]);
        for kind in [
            WindowKind::FiveHour,
            WindowKind::SevenDay,
            WindowKind::Other("x".into()),
        ] {
            assert_eq!(
                estimate_reset(&kind, &s, Some(T + HOUR_MS), T + m(20)),
                ResetInfo::Exact { at_ms: T + HOUR_MS }
            );
            assert_eq!(
                estimate_reset(&kind, &[], Some(T + 1), T),
                ResetInfo::Exact { at_ms: T + 1 }
            );
        }
    }

    #[test]
    fn only_kinds_with_a_duration_are_estimated() {
        let s = series(&[(T, 10.0), (T + m(15), 20.0)]);
        assert_eq!(
            estimate_reset(&WindowKind::Other("xh".into()), &s, None, T + m(20)),
            ResetInfo::Unknown
        );
        let opus = estimate_reset(
            &WindowKind::Other("seven_day_opus".into()),
            &s,
            None,
            T + m(20),
        );
        assert_eq!(
            opus,
            estimate_reset(&WindowKind::SevenDay, &s, None, T + m(20))
        );
        assert!(matches!(
            opus,
            ResetInfo::Estimated {
                confidence: Confidence::Low,
                ..
            }
        ));
    }

    #[test]
    fn rise_from_zero_starts_a_window() {
        // Desktop's integer 0 hides usage below one point, so the window may already have been
        // running at T + 15 min: at most Medium, however narrow the interval.
        let got = five(
            &[
                (T, 0.0),
                (T + m(15), 0.0),
                (T + m(30), 5.0),
                (T + m(45), 8.0),
            ],
            T + m(50),
        );
        assert_eq!(
            got,
            est(
                T + m(15) + m(7) + 30_000 + FIVE_HOURS_MS,
                m(7) + 30_000,
                Confidence::Medium
            )
        );
        // Wider intervals still fall to Low.
        let got = five(&[(T, 0.0), (T + 2 * HOUR_MS, 5.0)], T + 2 * HOUR_MS);
        assert_eq!(
            got,
            est(T + HOUR_MS + FIVE_HOURS_MS, HOUR_MS, Confidence::Low)
        );
        // A known reset between the 0 and the rise bounds the start instead: the 0 belonged to
        // the window that ended there.
        let got = estimate_reset(
            &WindowKind::FiveHour,
            &series(&[(T, 0.0), (T + m(15), 5.0)]),
            Some(T + m(5)),
            T + m(20),
        );
        assert_eq!(got, est(T + m(10) + FIVE_HOURS_MS, m(5), Confidence::High));
    }

    #[test]
    fn light_usage_never_claims_a_precision_it_lacks() {
        // At 0.05–0.12 %/min Desktop still shows 0 up to ten minutes into a window, so a rise
        // from 0 does not bound its start. Estimates may then miss, but never as High.
        let synth = synth::light();
        let fh = synth.fh();
        let (mut checked, mut missed) = (0, 0);
        for (k, truth) in synth.samples.iter().enumerate() {
            let got = estimate_reset(
                &WindowKind::FiveHour,
                &fh[..=k],
                None,
                truth.t_ms + MINUTE_MS,
            );
            let (
                Some(true_at),
                ResetInfo::Estimated {
                    at_ms,
                    plus_minus_ms,
                    confidence,
                },
            ) = (truth.fh_reset_ms, &got)
            else {
                continue;
            };
            let hit = (at_ms - true_at).abs() <= *plus_minus_ms;
            assert!(
                hit || *confidence != Confidence::High,
                "5h sample {k}: High {at_ms}±{plus_minus_ms}, true {true_at}"
            );
            checked += 1;
            missed += usize::from(!hit);
        }
        assert!(checked > 100, "{checked}");
        assert!(missed > 0, "the hidden window start is reproduced");
    }

    #[test]
    fn decrease_is_a_boundary_even_without_zero() {
        // 73 → 13 in one 15-min gap: reset plus fresh usage.
        let got = five(
            &[
                (T, 60.0),
                (T + m(15), 73.0),
                (T + m(30), 13.0),
                (T + m(45), 20.0),
            ],
            T + m(50),
        );
        assert_eq!(
            got,
            est(
                T + m(22) + 30_000 + FIVE_HOURS_MS,
                m(7) + 30_000,
                Confidence::High
            )
        );
        // 58 → 48 behaves the same.
        let got = five(&[(T, 58.0), (T + m(15), 48.0)], T + m(20));
        assert_eq!(
            got,
            est(
                T + m(7) + 30_000 + FIVE_HOURS_MS,
                m(7) + 30_000,
                Confidence::High
            )
        );
    }

    #[test]
    fn plus_minus_grows_with_the_gap_around_the_boundary() {
        let got = five(&[(T, 73.0), (T + HOUR_MS, 13.0)], T + HOUR_MS);
        assert_eq!(
            got,
            est(T + m(30) + FIVE_HOURS_MS, m(30), Confidence::Medium)
        );
        let got = five(&[(T, 73.0), (T + 2 * HOUR_MS, 13.0)], T + 2 * HOUR_MS);
        assert_eq!(
            got,
            est(T + HOUR_MS + FIVE_HOURS_MS, HOUR_MS, Confidence::Low)
        );
        // Threshold edges: ±10 min is High, ±45 min Medium, just over is Low.
        let conf = |gap: Ms| match five(&[(T, 50.0), (T + gap, 10.0)], T + gap) {
            ResetInfo::Estimated { confidence, .. } => confidence,
            other => panic!("{other:?}"),
        };
        assert_eq!(conf(m(20)), Confidence::High);
        assert_eq!(conf(m(22)), Confidence::Medium);
        assert_eq!(conf(m(90)), Confidence::Medium);
        assert_eq!(conf(m(92)), Confidence::Low);
    }

    /// Start in `(T - 4 h 15 min, T]`: at or before the first sample, after `newest - 5 h`.
    fn from_first_sample_45_min_series() -> ResetInfo {
        let (lo, hi) = (T + m(45) - FIVE_HOURS_MS, T);
        est(
            (lo + hi) / 2 + FIVE_HOURS_MS,
            (hi - lo) / 2,
            Confidence::Low,
        )
    }

    #[test]
    fn one_point_dip_is_noise() {
        // 41 → 40 is not a reset, so the window reaches back to the first sample.
        let got = five(
            &[
                (T, 40.0),
                (T + m(15), 41.0),
                (T + m(30), 40.0),
                (T + m(45), 42.0),
            ],
            T + m(50),
        );
        assert_eq!(got, from_first_sample_45_min_series());
    }

    #[test]
    fn gap_longer_than_window_is_a_boundary() {
        // Rising across an 8 h gap: the window holding the new sample began within 5 h of it.
        let got = five(
            &[
                (T, 40.0),
                (T + 8 * HOUR_MS, 45.0),
                (T + 8 * HOUR_MS + m(15), 50.0),
            ],
            T + 8 * HOUR_MS + m(20),
        );
        // Bounded by both the gap (> T + 3 h) and the newest sample (> T + 8 h 15 min - 5 h).
        let (lo, hi) = (T + 3 * HOUR_MS + m(15), T + 8 * HOUR_MS);
        assert_eq!(
            got,
            est(
                (lo + hi) / 2 + FIVE_HOURS_MS,
                (hi - lo) / 2,
                Confidence::Low
            )
        );
    }

    #[test]
    fn monotone_series_reaches_back_one_window() {
        let got = five(
            &[
                (T, 10.0),
                (T + m(15), 20.0),
                (T + m(30), 30.0),
                (T + m(45), 40.0),
            ],
            T + m(50),
        );
        assert_eq!(got, from_first_sample_45_min_series());
        // A single sample reaches back a whole window.
        assert_eq!(
            five(&[(T, 10.0)], T + m(5)),
            est(T + FIVE_HOURS_MS / 2, FIVE_HOURS_MS / 2, Confidence::Low)
        );
    }

    #[test]
    fn estimate_never_precedes_the_last_active_sample() {
        // Window A runs 09:00–14:00 and is seen rising 5 → 30 until 13:00. The laptop sleeps
        // 13:00–16:00; meanwhile A resets and window B starts at 14:30 and climbs past A's last
        // value. No drop, no gap over 5 h, no rise from 0: the boundary is invisible. But the
        // 16:00 sample shows usage, so its window runs past 16:00 and began after 11:00.
        let mut pts = vec![(T + 8 * HOUR_MS + m(45), 0.0)];
        pts.extend((0..=16).map(|k| (T + 9 * HOUR_MS + k * m(15), 5.0 + k as f32 * 25.0 / 16.0)));
        let last = T + 16 * HOUR_MS;
        pts.push((last, 45.0));
        let true_reset = T + 19 * HOUR_MS + m(30);
        let got = five(&pts, last + MINUTE_MS);
        let ResetInfo::Estimated {
            at_ms,
            plus_minus_ms,
            confidence,
        } = got
        else {
            panic!("{got:?}");
        };
        assert!(at_ms > last, "reset before the last active sample: {got:?}");
        assert!((at_ms - true_reset).abs() <= plus_minus_ms, "{got:?}");
        assert_eq!(confidence, Confidence::Low);

        // Monotone for 4 h: the window began in (last - 5 h, first sample], not up to 5 h before
        // the first sample.
        let pts: Vec<(Ms, f32)> = (0..=16)
            .map(|k| (T + k * m(15), 5.0 + 2.0 * k as f32))
            .collect();
        let last = T + 4 * HOUR_MS;
        let got = five(&pts, last + MINUTE_MS);
        assert_eq!(got, est(T + 4 * HOUR_MS + m(30), m(30), Confidence::Medium));
        assert!(got.at_ms().is_some_and(|at| at > last));
    }

    #[test]
    fn last_sample_zero_is_unknown() {
        assert_eq!(
            five(&[(T, 30.0), (T + m(15), 0.0)], T + m(20)),
            ResetInfo::Unknown
        );
        assert_eq!(five(&[(T, 0.0)], T + m(20)), ResetInfo::Unknown);
        assert_eq!(
            five(&[(T, 0.0), (T + m(15), 0.0)], T + m(20)),
            ResetInfo::Unknown
        );
        assert_eq!(
            week(&[(T, 30.0), (T + m(15), 0.0)], Some(T - HOUR_MS), T + m(20)),
            ResetInfo::Unknown
        );
    }

    #[test]
    fn no_samples() {
        assert_eq!(five(&[], T), ResetInfo::Unknown);
        assert_eq!(week(&[], None, T), ResetInfo::Unknown);
        assert_eq!(
            estimate_reset(&WindowKind::FiveHour, &[], Some(T - HOUR_MS), T),
            ResetInfo::Unknown,
            "a 5 h window starts with the next message, not on a schedule"
        );
        assert_eq!(
            week(&[], Some(T - HOUR_MS), T),
            est(T - HOUR_MS + SEVEN_DAYS_MS, DAY_MS, Confidence::Low)
        );
    }

    #[test]
    fn weekly_is_always_low_with_an_extra_day() {
        let got = week(
            &[
                (T, 80.0),
                (T + m(15), 84.0),
                (T + m(30), 41.0),
                (T + m(45), 42.0),
            ],
            None,
            T + m(50),
        );
        assert_eq!(
            got,
            est(
                T + m(22) + 30_000 + SEVEN_DAYS_MS,
                m(7) + 30_000 + DAY_MS,
                Confidence::Low
            )
        );
        // Weekly monotone: reaches back a week from the newest sample.
        let got = week(&[(T, 10.0), (T + HOUR_MS, 12.0)], None, T + HOUR_MS);
        let (lo, hi) = (T + HOUR_MS - SEVEN_DAYS_MS, T);
        assert_eq!(
            got,
            est(
                (lo + hi) / 2 + SEVEN_DAYS_MS,
                (hi - lo) / 2 + DAY_MS,
                Confidence::Low
            )
        );
    }

    #[test]
    fn weekly_fallback_rolls_forward_whole_weeks() {
        let now = T;
        // Monotone since a reset ten days ago: 10 d - 7 d → next is 4 days ahead.
        let r = now - 10 * DAY_MS;
        let s = [
            (r + HOUR_MS, 5.0),
            (r + 4 * DAY_MS, 20.0),
            (now - HOUR_MS, 50.0),
        ];
        assert_eq!(
            week(&s, Some(r), now),
            est(now + 4 * DAY_MS, DAY_MS, Confidence::Low)
        );
        // Exactly one and two weeks ago: the result is strictly after now.
        assert_eq!(
            week(&[(now - HOUR_MS, 5.0)], Some(now - SEVEN_DAYS_MS), now),
            est(now + SEVEN_DAYS_MS, DAY_MS, Confidence::Low)
        );
        assert_eq!(
            week(&[], Some(now - 2 * SEVEN_DAYS_MS), now),
            est(now + SEVEN_DAYS_MS, DAY_MS, Confidence::Low)
        );
        // All samples predate the reset (they describe the finished window).
        let s = [(now - 3 * DAY_MS, 70.0), (now - 2 * HOUR_MS, 90.0)];
        assert_eq!(
            week(&s, Some(now - HOUR_MS), now),
            est(now - HOUR_MS + SEVEN_DAYS_MS, DAY_MS, Confidence::Low)
        );
        // A drop after the reset is newer information and wins over the schedule.
        let s = [
            (now - 3 * DAY_MS, 10.0),
            (now - DAY_MS - m(15), 60.0),
            (now - DAY_MS, 5.0),
        ];
        let got = week(&s, Some(now - 4 * DAY_MS), now);
        assert_eq!(
            got,
            est(
                now - DAY_MS - m(7) - 30_000 + SEVEN_DAYS_MS,
                m(7) + 30_000 + DAY_MS,
                Confidence::Low
            )
        );
    }

    #[test]
    fn past_exact_reset_bounds_the_start() {
        // Monotone samples right after a known reset: the window began in (reset, first sample].
        let got = estimate_reset(
            &WindowKind::FiveHour,
            &series(&[(T, 5.0), (T + m(15), 10.0)]),
            Some(T - m(10)),
            T + m(20),
        );
        assert_eq!(got, est(T - m(5) + FIVE_HOURS_MS, m(5), Confidence::High));
        // The reset between two samples is itself the boundary.
        let got = estimate_reset(
            &WindowKind::FiveHour,
            &series(&[(T - HOUR_MS, 40.0), (T, 50.0), (T + m(15), 52.0)]),
            Some(T + m(5)),
            T + m(20),
        );
        assert_eq!(got, est(T + m(10) + FIVE_HOURS_MS, m(5), Confidence::High));
        // 5 h samples all older than the reset: nothing is known about the new window.
        let got = estimate_reset(
            &WindowKind::FiveHour,
            &series(&[(T - HOUR_MS, 40.0), (T - m(30), 50.0)]),
            Some(T - m(10)),
            T,
        );
        assert_eq!(got, ResetInfo::Unknown);
    }

    #[test]
    fn old_estimates_do_not_move_with_now() {
        // A window located days ago: the estimate stays where the samples put it (in the past,
        // which tells merge "probably reset"), so repeated ticks give identical snapshots.
        let s = [(T, 73.0), (T + m(15), 13.0)];
        let five_expected = est(T + m(7) + 30_000 + FIVE_HOURS_MS, m(7) + 30_000, Confidence::High);
        for now in [T + 8 * HOUR_MS, T + 2 * DAY_MS, T + 2 * DAY_MS + 30_000] {
            assert_eq!(five(&s, now), five_expected, "{now}");
        }
        let week_expected = est(
            T + m(7) + 30_000 + SEVEN_DAYS_MS,
            m(7) + 30_000 + DAY_MS,
            Confidence::Low,
        );
        for now in [T + 30 * DAY_MS, T + 30 * DAY_MS + 30_000] {
            assert_eq!(week(&s, None, now), week_expected, "{now}");
        }
    }

    #[test]
    fn unsorted_and_non_finite_input_is_tolerated() {
        let sorted = [
            (T, 60.0),
            (T + m(15), 73.0),
            (T + m(30), 13.0),
            (T + m(45), 20.0),
        ];
        let expected = five(&sorted, T + m(50));
        let shuffled = [
            sorted[2],
            sorted[0],
            sorted[3],
            sorted[1],
            (T + m(40), f32::NAN),
        ];
        assert_eq!(five(&shuffled, T + m(50)), expected);
        let mut with_infinity = sorted.to_vec();
        with_infinity.insert(1, (T + m(5), f32::INFINITY));
        assert_eq!(five(&with_infinity, T + m(50)), expected);
    }

    #[test]
    fn realistic_history_brackets_the_true_reset() {
        let synth = synth::realistic();
        let fh = synth.fh();
        let sd = synth.sd();
        let (mut checked_fh, mut narrow, mut high) = (0, 0, 0);
        for (k, truth) in synth.samples.iter().enumerate() {
            let now = truth.t_ms + MINUTE_MS;

            let got = estimate_reset(&WindowKind::FiveHour, &fh[..=k], None, now);
            match (truth.fh > 0.0, truth.fh_reset_ms, &got) {
                (false, _, ResetInfo::Unknown) => {}
                (
                    true,
                    Some(true_at),
                    ResetInfo::Estimated {
                        at_ms,
                        plus_minus_ms,
                        confidence,
                    },
                ) => {
                    assert!(
                        (at_ms - true_at).abs() <= *plus_minus_ms,
                        "5h sample {k}: estimated {at_ms}±{plus_minus_ms}, true {true_at}"
                    );
                    checked_fh += 1;
                    narrow += usize::from(*plus_minus_ms <= HIGH_CONFIDENCE_MS);
                    high += usize::from(*confidence == Confidence::High);
                }
                other => panic!("5h sample {k}: unexpected {other:?}"),
            }

            let got = estimate_reset(&WindowKind::SevenDay, &sd[..=k], None, now);
            match (truth.sd > 0.0, &got) {
                (false, ResetInfo::Unknown) => {}
                (
                    true,
                    ResetInfo::Estimated {
                        at_ms,
                        plus_minus_ms,
                        confidence: Confidence::Low,
                    },
                ) => {
                    assert!(
                        (at_ms - truth.sd_reset_ms).abs() <= *plus_minus_ms,
                        "7d sample {k}: estimated {at_ms}±{plus_minus_ms}, true {}",
                        truth.sd_reset_ms
                    );
                }
                other => panic!("7d sample {k}: unexpected {other:?}"),
            }
        }
        assert!(checked_fh > 200, "{checked_fh}");
        assert!(
            narrow > 100,
            "most windows are located to ±7.5 min: {narrow}/{checked_fh}"
        );
        // Most of them rise from 0 and are capped at Medium; those located by a drop stay High.
        assert!(high > 20, "{high}/{checked_fh}");
    }

    proptest! {
        #[test]
        fn estimates_are_bounded(
            points in proptest::collection::vec((0_i64..20 * DAY_MS, -10.0_f32..110.0), 0..40),
            exact in proptest::option::of(-30 * DAY_MS..30 * DAY_MS),
            weekly in any::<bool>(),
            now_offset in 0_i64..40 * DAY_MS,
        ) {
            let now = T + now_offset;
            let samples: Vec<Sample> = points.iter().map(|&(t, pct)| Sample { t_ms: T + t, pct }).collect();
            let kind = if weekly { WindowKind::SevenDay } else { WindowKind::FiveHour };
            let mut sorted = samples.clone();
            sorted.sort_by_key(|s| s.t_ms);
            match estimate_reset(&kind, &samples, exact.map(|e| now + e), now) {
                ResetInfo::Exact { at_ms } => prop_assert!(at_ms > now),
                ResetInfo::Estimated { at_ms, plus_minus_ms, confidence } => {
                    // The newest sample's window was running then, so it resets later.
                    if let Some(last) = sorted.last().filter(|l| l.t_ms <= now) {
                        prop_assert!(at_ms >= last.t_ms, "{at_ms} < {}", last.t_ms);
                    }
                    prop_assert!(plus_minus_ms >= 0);
                    if weekly {
                        prop_assert_eq!(confidence, Confidence::Low);
                        prop_assert!(plus_minus_ms >= DAY_MS);
                    }
                }
                ResetInfo::Unknown => {}
            }
        }
    }

    #[test]
    fn extreme_times_do_not_overflow() {
        let s = series(&[
            (Ms::MIN, 50.0),
            (Ms::MIN + 1, 10.0),
            (Ms::MAX - 1, 20.0),
            (Ms::MAX, 5.0),
        ]);
        for kind in [WindowKind::FiveHour, WindowKind::SevenDay] {
            for exact in [None, Some(Ms::MIN), Some(0)] {
                for now in [Ms::MIN, 0, Ms::MAX] {
                    let _ = estimate_reset(&kind, &s, exact, now);
                }
            }
        }
        let _ = estimate_reset(&WindowKind::SevenDay, &[], Some(Ms::MIN), Ms::MAX);
    }
}
