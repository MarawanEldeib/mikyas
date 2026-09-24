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
//!
//! Implementation notes on the points above:
//! - A reset only counts as known while it is in the future. A reset already in the past (a
//!   stale estimate) neither bounds the window nor yields `pct_at_reset`.
//! - Estimated resets bound the window only approximately, and unknown ones not at all, so points
//!   before the newest decrease of more than [`DROP_TOLERANCE_PCT`] (including a decrease to the
//!   current value) are dropped as well: they belong to a previous window.
//! - The resampling grid is anchored at `now_ms` (`now`, `now - step`, …) so the current value is
//!   always a grid point. Samples at or after `now_ms` are ignored; `state.pct` is the value then.

use crate::engine::reset_estimate::DROP_TOLERANCE_PCT;
use crate::engine::types::{Burn, Phase, Sample, WindowKind, WindowState};
use crate::time::{HOUR_MS, MINUTE_MS, Ms};

pub const MIN_SLOPE_PCT_PER_H: f32 = 0.05;

/// Fit settings of one window kind.
struct Fit {
    lookback: Ms,
    step: Ms,
    min_span: Ms,
}

const FIVE_HOUR_FIT: Fit = Fit {
    lookback: 60 * MINUTE_MS,
    step: MINUTE_MS,
    min_span: 15 * MINUTE_MS,
};

const SEVEN_DAY_FIT: Fit = Fit {
    lookback: 24 * HOUR_MS,
    step: 10 * MINUTE_MS,
    min_span: 6 * HOUR_MS,
};

/// Forecasts when the window reaches 100 % at the recent pace (see the module docs).
///
/// `samples` is this window's history (Desktop samples or history rows, ideally sorted);
/// `state` is its merged current value.
pub fn compute(
    kind: &WindowKind,
    samples: &[Sample],
    state: &WindowState,
    now_ms: Ms,
) -> Option<Burn> {
    let fit = match kind {
        WindowKind::FiveHour => &FIVE_HOUR_FIT,
        WindowKind::SevenDay => &SEVEN_DAY_FIT,
        WindowKind::Other(_) => return None,
    };
    if state.stale
        || state.phase == Phase::ResetAwaitingData
        || state.limit_reached
        || !state.pct.is_finite()
    {
        return None;
    }
    let duration = kind.duration_ms()?;
    let pct = state.pct.clamp(0.0, 100.0);
    let reset_ms = state.reset.at_ms().filter(|r| *r > now_ms);
    let window_start = reset_ms.map(|r| r.saturating_sub(duration));

    let mut points: Vec<Sample> = samples
        .iter()
        .filter(|s| {
            s.pct.is_finite() && s.t_ms < now_ms && window_start.is_none_or(|w| s.t_ms >= w)
        })
        .copied()
        .collect();
    points.sort_by_key(|s| s.t_ms);
    points.push(Sample { t_ms: now_ms, pct });

    let current_window = points
        .windows(2)
        .rposition(|w| w[1].pct < w[0].pct - DROP_TOLERANCE_PCT)
        .map_or(0, |j| j + 1);
    let points = &points[current_window..];
    // The last point at or before the lookback start supplies the step value there.
    let lookback_start = now_ms.saturating_sub(fit.lookback);
    let first = points
        .partition_point(|s| s.t_ms <= lookback_start)
        .saturating_sub(1);
    let points = &points[first..];
    let start = points.first()?.t_ms.max(lookback_start);
    if now_ms.saturating_sub(start) < fit.min_span {
        return None;
    }

    let series = resample(points, start, now_ms, fit.step);
    let (_, y0) = *series.first()?;
    if series.iter().all(|&(_, y)| (y - y0).abs() < 1e-9) {
        return None;
    }
    let slope = ols_slope(&series)?;
    if slope.is_nan() || slope <= f64::from(MIN_SLOPE_PCT_PER_H) {
        return None;
    }

    let pct = f64::from(pct);
    let t100_ms = now_ms.saturating_add(hours_to_ms((100.0 - pct) / slope));
    let pct_at_reset =
        reset_ms.map(|r| (pct + slope * ms_to_hours(r.saturating_sub(now_ms))) as f32);
    Some(Burn {
        slope_pct_per_h: slope as f32,
        t100_ms: Some(t100_ms),
        pct_at_reset,
        hits_limit_before_reset: reset_ms.is_some_and(|r| t100_ms < r),
    })
}

/// Samples the step function through `points` (each value holds until the next point) at
/// `end`, `end - step`, … down to `start`, as (hours since `start`, pct).
fn resample(points: &[Sample], start: Ms, end: Ms, step: Ms) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    // Number of points at or before `t`.
    let mut known = points.len();
    let mut t = end;
    while t >= start {
        while known > 0 && points[known - 1].t_ms > t {
            known -= 1;
        }
        let Some(p) = known.checked_sub(1).and_then(|i| points.get(i)) else {
            break;
        };
        out.push((ms_to_hours(t - start), f64::from(p.pct)));
        match t.checked_sub(step) {
            Some(next) => t = next,
            None => break,
        }
    }
    out
}

/// Ordinary least-squares slope of y over x; `None` unless x varies.
fn ols_slope(series: &[(f64, f64)]) -> Option<f64> {
    let n = series.len() as f64;
    let mean_x = series.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = series.iter().map(|p| p.1).sum::<f64>() / n;
    let (sxx, sxy) = series.iter().fold((0.0, 0.0), |(sxx, sxy), &(x, y)| {
        let dx = x - mean_x;
        (sxx + dx * dx, sxy + dx * (y - mean_y))
    });
    (sxx > 0.0).then(|| sxy / sxx)
}

fn ms_to_hours(ms: Ms) -> f64 {
    ms as f64 / HOUR_MS as f64
}

fn hours_to_ms(hours: f64) -> Ms {
    // `as` saturates, so an absurd forecast cannot overflow.
    (hours * HOUR_MS as f64).round() as Ms
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    use super::*;
    use crate::engine::reset_estimate::estimate_reset;
    use crate::engine::types::{Confidence, ResetInfo, Source};
    use crate::sources::desktop_usage::synth;
    use crate::time::DAY_MS;

    /// 2026-09-24T12:00:00Z.
    const NOW: Ms = 1_790_208_000_000 + 12 * HOUR_MS;

    const fn m(n: i64) -> Ms {
        n * MINUTE_MS
    }

    fn state(kind: WindowKind, pct: f32, reset: ResetInfo) -> WindowState {
        WindowState {
            kind,
            pct,
            reset,
            source: Source::Desktop,
            observed_at_ms: NOW,
            stale: false,
            limit_reached: pct >= 99.5,
            phase: Phase::Active,
        }
    }

    fn five(pct: f32, reset: ResetInfo) -> WindowState {
        state(WindowKind::FiveHour, pct, reset)
    }

    fn series(points: &[(Ms, f32)]) -> Vec<Sample> {
        points
            .iter()
            .map(|&(t_ms, pct)| Sample { t_ms, pct })
            .collect()
    }

    fn exact(at_ms: Ms) -> ResetInfo {
        ResetInfo::Exact { at_ms }
    }

    /// One sample per minute over the last hour at `rate` %/h, reaching `pct_now` at `NOW`.
    fn per_minute(rate: f32, pct_now: f32) -> Vec<Sample> {
        (1..=60)
            .rev()
            .map(|k| Sample {
                t_ms: NOW - m(k),
                pct: pct_now - rate * k as f32 / 60.0,
            })
            .collect()
    }

    /// Desktop-style 15-minute samples over the last hour, 2.5 points apart, ending below 50.
    fn quarter_hourly() -> Vec<Sample> {
        series(&[
            (NOW - m(60), 40.0),
            (NOW - m(45), 42.5),
            (NOW - m(30), 45.0),
            (NOW - m(15), 47.5),
        ])
    }

    #[test]
    fn linear_ten_percent_per_hour() {
        let burn = compute(
            &WindowKind::FiveHour,
            &per_minute(10.0, 50.0),
            &five(50.0, exact(NOW + 3 * HOUR_MS)),
            NOW,
        )
        .unwrap();
        assert!((burn.slope_pct_per_h - 10.0).abs() < 0.01, "{burn:?}");
        let expected = NOW + 5 * HOUR_MS;
        assert!(
            (burn.t100_ms.unwrap() - expected).abs() <= MINUTE_MS,
            "{burn:?}"
        );
        assert!((burn.pct_at_reset.unwrap() - 80.0).abs() < 0.1, "{burn:?}");
        assert!(!burn.hits_limit_before_reset);
    }

    #[test]
    fn projects_past_100_before_a_late_reset() {
        let reset = ResetInfo::Estimated {
            at_ms: NOW + 4 * HOUR_MS,
            plus_minus_ms: m(7),
            confidence: Confidence::High,
        };
        let burn = compute(
            &WindowKind::FiveHour,
            &per_minute(10.0, 70.0),
            &five(70.0, reset),
            NOW,
        )
        .unwrap();
        assert!(
            (burn.t100_ms.unwrap() - (NOW + 3 * HOUR_MS)).abs() <= MINUTE_MS,
            "{burn:?}"
        );
        assert!(
            (burn.pct_at_reset.unwrap() - 110.0).abs() < 0.1,
            "not clamped: {burn:?}"
        );
        assert!(burn.hits_limit_before_reset);
    }

    #[test]
    fn unknown_reset_gives_no_reset_projection() {
        let burn = compute(
            &WindowKind::FiveHour,
            &per_minute(10.0, 50.0),
            &five(50.0, ResetInfo::Unknown),
            NOW,
        )
        .unwrap();
        assert!((burn.slope_pct_per_h - 10.0).abs() < 0.01);
        assert_eq!(burn.pct_at_reset, None);
        assert!(!burn.hits_limit_before_reset);
        // A reset already in the past is not a known future reset either.
        let past = five(50.0, exact(NOW - m(5)));
        let burn = compute(&WindowKind::FiveHour, &per_minute(10.0, 50.0), &past, NOW).unwrap();
        assert_eq!(
            (burn.pct_at_reset, burn.hits_limit_before_reset),
            (None, false)
        );
    }

    #[test]
    fn desktop_staircase_is_close_to_the_true_pace() {
        let burn = compute(
            &WindowKind::FiveHour,
            &quarter_hourly(),
            &five(50.0, ResetInfo::Unknown),
            NOW,
        )
        .unwrap();
        assert!((8.0..12.0).contains(&burn.slope_pct_per_h), "{burn:?}");
    }

    #[test]
    fn flat_is_none() {
        let s = series(&[
            (NOW - m(60), 40.0),
            (NOW - m(30), 40.0),
            (NOW - m(15), 40.0),
        ]);
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(40.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
    }

    #[test]
    fn negative_or_tiny_slope_is_none() {
        // Half-point dips are noise, not resets, so the fit sees a falling line.
        let s = series(&[
            (NOW - m(60), 50.0),
            (NOW - m(45), 49.5),
            (NOW - m(30), 49.0),
            (NOW - m(15), 48.5),
        ]);
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(48.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
        // 0.04 %/h rise is below the minimum.
        let s = per_minute(0.04, 50.0);
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(50.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
    }

    #[test]
    fn short_span_is_none() {
        let s = series(&[(NOW - m(10), 40.0)]);
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(45.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &[],
                &five(45.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
        // Exactly the minimum span is enough.
        let s = series(&[(NOW - m(15), 40.0)]);
        assert!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(45.0, ResetInfo::Unknown),
                NOW
            )
            .is_some()
        );
        // Weekly needs 6 h.
        let s = series(&[(NOW - 5 * HOUR_MS, 40.0), (NOW - 2 * HOUR_MS, 44.0)]);
        let weekly = state(WindowKind::SevenDay, 45.0, ResetInfo::Unknown);
        assert_eq!(compute(&WindowKind::SevenDay, &s, &weekly, NOW), None);
    }

    #[test]
    fn unusable_states_are_none() {
        let s = per_minute(10.0, 50.0);
        let mut st = five(50.0, ResetInfo::Unknown);
        assert!(compute(&WindowKind::FiveHour, &s, &st, NOW).is_some());
        st.stale = true;
        assert_eq!(compute(&WindowKind::FiveHour, &s, &st, NOW), None);
        let mut st = five(50.0, ResetInfo::Unknown);
        st.phase = Phase::ResetAwaitingData;
        assert_eq!(compute(&WindowKind::FiveHour, &s, &st, NOW), None);
        let st = five(99.5, ResetInfo::Unknown);
        assert!(st.limit_reached);
        assert_eq!(
            compute(&WindowKind::FiveHour, &per_minute(10.0, 99.5), &st, NOW),
            None
        );
        let st = five(f32::NAN, ResetInfo::Unknown);
        assert_eq!(compute(&WindowKind::FiveHour, &s, &st, NOW), None);
    }

    #[test]
    fn only_five_hour_and_seven_day() {
        let s = per_minute(10.0, 50.0);
        for kind in [
            WindowKind::Other("seven_day_opus".into()),
            WindowKind::Other("xh".into()),
        ] {
            let st = state(kind.clone(), 50.0, ResetInfo::Unknown);
            assert_eq!(compute(&kind, &s, &st, NOW), None);
        }
    }

    #[test]
    fn previous_window_is_excluded_by_known_reset() {
        // Reset in 4.5 h: the window began 30 min ago, so 90/95 belong to the previous one.
        let s = series(&[
            (NOW - m(60), 90.0),
            (NOW - m(45), 95.0),
            (NOW - m(30), 2.0),
            (NOW - m(15), 6.0),
        ]);
        let with_reset = compute(
            &WindowKind::FiveHour,
            &s,
            &five(10.0, exact(NOW + m(270))),
            NOW,
        )
        .unwrap();
        assert!(
            (10.0..25.0).contains(&with_reset.slope_pct_per_h),
            "{with_reset:?}"
        );
        // Same result without a reset: the drop itself separates the windows.
        let without = compute(
            &WindowKind::FiveHour,
            &s,
            &five(10.0, ResetInfo::Unknown),
            NOW,
        )
        .unwrap();
        assert_eq!(without.slope_pct_per_h, with_reset.slope_pct_per_h);
        assert_eq!(without.t100_ms, with_reset.t100_ms);
        let only_new = &s[2..];
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                only_new,
                &five(10.0, ResetInfo::Unknown),
                NOW
            ),
            Some(without)
        );
    }

    #[test]
    fn drop_inside_lookback_never_yields_a_garbled_slope() {
        // The reset is 10 min old: too little of the new window to fit, and never negative.
        let s = series(&[
            (NOW - m(60), 80.0),
            (NOW - m(45), 85.0),
            (NOW - m(30), 90.0),
            (NOW - m(10), 3.0),
        ]);
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(5.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
        // A decrease right at the current value (e.g. fresh CLI value after a reset).
        let s = quarter_hourly();
        assert_eq!(
            compute(
                &WindowKind::FiveHour,
                &s,
                &five(4.0, ResetInfo::Unknown),
                NOW
            ),
            None
        );
        // An estimated reset whose window start is fuzzy still cannot leak old samples.
        let est = ResetInfo::Estimated {
            at_ms: NOW + 5 * HOUR_MS - m(50),
            plus_minus_ms: m(15),
            confidence: Confidence::Medium,
        };
        let s = series(&[
            (NOW - m(60), 70.0),
            (NOW - m(45), 72.0),
            (NOW - m(30), 2.0),
            (NOW - m(15), 6.0),
        ]);
        let burn = compute(&WindowKind::FiveHour, &s, &five(10.0, est), NOW).unwrap();
        assert!(burn.slope_pct_per_h > 10.0, "{burn:?}");
    }

    #[test]
    fn weekly_uses_a_day_of_history() {
        // 1 %/h for the last 30 h, sampled every 15 min.
        let s: Vec<Sample> = (1..=120)
            .rev()
            .map(|k| Sample {
                t_ms: NOW - k * m(15),
                pct: 50.0 - k as f32 / 4.0,
            })
            .collect();
        let st = state(WindowKind::SevenDay, 50.0, exact(NOW + DAY_MS));
        let burn = compute(&WindowKind::SevenDay, &s, &st, NOW).unwrap();
        assert!((burn.slope_pct_per_h - 1.0).abs() < 0.05, "{burn:?}");
        assert!((burn.pct_at_reset.unwrap() - 74.0).abs() < 1.5, "{burn:?}");
        assert!(!burn.hits_limit_before_reset);
        let st = state(WindowKind::SevenDay, 50.0, exact(NOW + 3 * DAY_MS));
        let burn = compute(&WindowKind::SevenDay, &s, &st, NOW).unwrap();
        assert!(burn.hits_limit_before_reset, "{burn:?}");
    }

    #[test]
    fn input_order_and_future_samples_do_not_matter() {
        let sorted = quarter_hourly();
        let st = five(50.0, ResetInfo::Unknown);
        let expected = compute(&WindowKind::FiveHour, &sorted, &st, NOW);
        let mut messy = sorted.clone();
        messy.reverse();
        messy.push(Sample {
            t_ms: NOW + m(5),
            pct: 99.0,
        });
        messy.push(Sample {
            t_ms: NOW,
            pct: 1.0,
        });
        messy.push(Sample {
            t_ms: NOW - m(20),
            pct: f32::NAN,
        });
        assert_eq!(compute(&WindowKind::FiveHour, &messy, &st, NOW), expected);
    }

    #[test]
    fn realistic_history() {
        let synth = synth::realistic();
        let fh = synth.fh();
        let mut steady = 0;
        for (k, truth) in synth.samples.iter().enumerate() {
            let now = truth.t_ms;
            let history = &fh[..=k];
            let reset = estimate_reset(&WindowKind::FiveHour, history, None, now);
            let st = WindowState {
                observed_at_ms: now,
                ..state(WindowKind::FiveHour, truth.fh, reset)
            };
            let got = compute(&WindowKind::FiveHour, history, &st, now);
            if let Some(b) = &got {
                assert!(b.slope_pct_per_h > MIN_SLOPE_PCT_PER_H, "sample {k}: {b:?}");
                assert!(b.t100_ms.is_some_and(|t| t > now), "sample {k}: {b:?}");
            }

            // Samples before a decrease inside the lookback never influence the result.
            if let Some(j) = (1..=k)
                .rev()
                .find(|&j| fh[j].pct < fh[j - 1].pct - DROP_TOLERANCE_PCT)
            {
                if fh[j].t_ms > now - m(60) {
                    assert_eq!(
                        got,
                        compute(&WindowKind::FiveHour, &fh[j..=k], &st, now),
                        "sample {k}"
                    );
                }
            }

            // A steady hour inside one window (15-min increments equal up to integer rounding):
            // the forecast pace matches the simulated pace.
            if k >= 4 {
                let hour = &synth.samples[k - 4..=k];
                let incs: Vec<f32> = hour.windows(2).map(|w| w[1].fh - w[0].fh).collect();
                let (lo, hi) = incs
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &i| (lo.min(i), hi.max(i)));
                let steady_hour = lo >= 5.0
                    && hi - lo <= 1.0
                    && hour.windows(2).all(|w| {
                        w[1].t_ms - w[0].t_ms == m(15) && w[1].fh_reset_ms == w[0].fh_reset_ms
                    });
                if steady_hour && truth.fh < 95.0 {
                    let b =
                        got.unwrap_or_else(|| panic!("sample {k}: steady hour without forecast"));
                    let pace = hour[4].fh - hour[0].fh;
                    assert!(
                        (b.slope_pct_per_h - pace).abs() <= 0.15 * pace,
                        "sample {k}: {b:?} vs {pace}"
                    );
                    steady += 1;
                }
            }
        }
        assert!(steady >= 8, "{steady} steady hours checked");
    }

    proptest! {
        #[test]
        fn forecasts_are_sane(
            points in proptest::collection::vec((0_i64..2 * DAY_MS, -10.0_f32..110.0), 0..60),
            pct in 0.0_f32..99.0,
            weekly in any::<bool>(),
            reset_offset in proptest::option::of(-DAY_MS..8 * DAY_MS),
        ) {
            let samples: Vec<Sample> = points.iter().map(|&(t, p)| Sample { t_ms: NOW - t, pct: p }).collect();
            let kind = if weekly { WindowKind::SevenDay } else { WindowKind::FiveHour };
            let reset = reset_offset.map_or(ResetInfo::Unknown, |o| exact(NOW + o));
            let st = state(kind.clone(), pct, reset.clone());
            if let Some(b) = compute(&kind, &samples, &st, NOW) {
                prop_assert!(b.slope_pct_per_h > MIN_SLOPE_PCT_PER_H);
                prop_assert!(b.t100_ms.is_some_and(|t| t > NOW));
                let future_reset = reset.at_ms().filter(|r| *r > NOW);
                prop_assert_eq!(b.pct_at_reset.is_some(), future_reset.is_some());
                if let Some(p) = b.pct_at_reset {
                    prop_assert!(p >= pct - 0.01);
                }
            }
        }
    }

    #[test]
    fn extreme_times_do_not_overflow() {
        let s = series(&[
            (Ms::MIN, 10.0),
            (Ms::MIN + 1, 20.0),
            (Ms::MAX - 1, 30.0),
            (Ms::MAX, 40.0),
        ]);
        for now in [Ms::MIN, 0, Ms::MAX] {
            for reset in [ResetInfo::Unknown, exact(Ms::MAX), exact(Ms::MIN)] {
                for kind in [WindowKind::FiveHour, WindowKind::SevenDay] {
                    let _ = compute(&kind, &s, &state(kind.clone(), 50.0, reset.clone()), now);
                }
            }
        }
    }
}
