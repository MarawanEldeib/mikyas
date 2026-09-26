//! Merges CLI and Desktop observations of one window into a display state.
//!
//! Rules (all for a single `kind`):
//! 1. Split CLI observations into LIVE (`resets_at_ms > now_ms`) and EXPIRED.
//! 2. Group LIVE by `resets_at_ms`, treating values within [`RESET_GROUP_TOLERANCE_MS`] as equal.
//!    The group with the latest reset wins; within it `pct = max(pct)` and
//!    `observed_at = max(observed_at)` (several sessions report the same account-level numbers,
//!    an idle session may carry an older, lower value). Call the winner C.
//! 3. Let D be the Desktop observation (if any).
//!    - a. C and D, D newer than C:
//!      - D.observed_at >= C.resets_at → D is from a later window: pct = D,
//!        reset = `desktop_estimate`, source Desktop.
//!      - C.pct - D.pct >= [`RESET_DROP_PCT`] → early reset detected: pct = D,
//!        reset = `desktop_estimate`, source Desktop.
//!      - otherwise → pct = D.pct, reset = Exact(C.resets_at), source Desktop, observed = D.
//!    - b. C, and D missing or not newer → pct = C, reset = Exact(C.resets_at), source Cli.
//!    - c. no C, D present → pct = D, reset = `estimate`, source Desktop. If an EXPIRED CLI reset
//!      lies in (D.observed_at, now] — or `estimate.at_ms()` lies in (D.observed_at, now] — the
//!      window has reset since D was measured: phase = ResetAwaitingData, pct = 0.
//!    - d. no C, no D, but EXPIRED CLI observations → ResetAwaitingData, pct = 0, reset =
//!      `estimate`, source Cli, observed = newest expired observation.
//!    - e. nothing → `None`.
//! 4. `stale = now - observed_at > stale_after_ms` (always false in ResetAwaitingData).
//!    `limit_reached = pct >= 99.5`. pct is clamped to 0..=100.
//!
//! Implementation notes:
//! - CLI observations without `resets_at_ms` can be neither LIVE nor EXPIRED and are ignored.
//! - Grouping is anchored on the latest LIVE reset `R`: the winning group is every LIVE
//!   observation with `resets_at >= R - RESET_GROUP_TOLERANCE_MS`, and `C.resets_at = R`. Every
//!   reduction is a max, so the result does not depend on the order of `cli`.
//! - A `desktop` observation of another kind is treated as absent.
//! - NaN pct values (malformed input) are treated as 0 before any comparison.
//! - `estimate` may be the exact reset C reported (the snapshot folds the captures' reset times
//!   into it), which in 3a is the reset of the window that has already ended. `desktop_estimate`
//!   is built from the Desktop samples and past exact resets only; [`merge_window`] passes
//!   `estimate` for both.

use crate::engine::types::{Observation, Phase, ResetInfo, Source, WindowKind, WindowState, is_reset_drop};
use crate::time::Ms;

pub use crate::engine::types::RESET_DROP_PCT;
/// CLI `resets_at` values within this are the same window.
pub const RESET_GROUP_TOLERANCE_MS: Ms = 120_000;
/// A window at or above this percentage counts as having reached its limit.
pub const LIMIT_REACHED_PCT: f32 = 99.5;

/// Merges every source of one window into a display state (see the module docs for the rules).
///
/// `cli` may contain observations of other kinds (ignore them). `desktop` must be of `kind` if present.
pub fn merge_window(
    kind: &WindowKind,
    cli: &[Observation],
    desktop: Option<&Observation>,
    estimate: ResetInfo,
    now_ms: Ms,
    stale_after_ms: Ms,
) -> Option<WindowState> {
    merge_window_with(kind, cli, desktop, estimate.clone(), estimate, now_ms, stale_after_ms)
}

/// [`merge_window`] with a separate `desktop_estimate` for rule 3a (see the module docs): the
/// reset of a window Desktop saw start after the one the live CLI values describe.
pub fn merge_window_with(
    kind: &WindowKind,
    cli: &[Observation],
    desktop: Option<&Observation>,
    estimate: ResetInfo,
    desktop_estimate: ResetInfo,
    now_ms: Ms,
    stale_after_ms: Ms,
) -> Option<WindowState> {
    let mut live: Vec<(Ms, &Observation)> = Vec::new();
    let mut expired: Vec<(Ms, &Observation)> = Vec::new();
    for obs in cli.iter().filter(|o| &o.kind == kind) {
        let Some(reset) = obs.resets_at_ms else {
            continue;
        };
        if reset > now_ms {
            live.push((reset, obs));
        } else {
            expired.push((reset, obs));
        }
    }
    let c = winning_group(&live);
    let d = desktop.filter(|o| &o.kind == kind);

    let draft = match (c, d) {
        // 3a: Desktop is newer than the winning CLI group.
        (Some(c), Some(d)) if d.observed_at_ms > c.observed_at_ms => {
            let d_pct = sanitize_pct(d.pct);
            let later_window = d.observed_at_ms >= c.resets_at_ms;
            let early_reset = is_reset_drop(c.pct, d_pct);
            let reset =
                if later_window || early_reset { desktop_estimate } else { ResetInfo::Exact { at_ms: c.resets_at_ms } };
            Draft::active(d_pct, reset, Source::Desktop, d.observed_at_ms)
        }
        // 3b: CLI only, or Desktop not newer.
        (Some(c), _) => Draft::active(c.pct, ResetInfo::Exact { at_ms: c.resets_at_ms }, Source::Cli, c.observed_at_ms),
        // 3c: Desktop only (plus possibly expired CLI observations).
        (None, Some(d)) => {
            let reset_since_d = |t: Ms| t > d.observed_at_ms && t <= now_ms;
            let has_reset =
                expired.iter().any(|&(r, _)| reset_since_d(r)) || estimate.at_ms().is_some_and(reset_since_d);
            if has_reset {
                Draft::awaiting(estimate, Source::Desktop, d.observed_at_ms)
            } else {
                Draft::active(sanitize_pct(d.pct), estimate, Source::Desktop, d.observed_at_ms)
            }
        }
        // 3d / 3e: only expired CLI observations, or nothing at all.
        (None, None) => {
            let newest = expired.iter().map(|(_, o)| o.observed_at_ms).max()?;
            Draft::awaiting(estimate, Source::Cli, newest)
        }
    };
    Some(draft.finish(kind.clone(), now_ms, stale_after_ms))
}

/// The reduced winning LIVE CLI group ("C" in the module docs).
#[derive(Debug, Clone, Copy)]
struct CliWinner {
    pct: f32,
    observed_at_ms: Ms,
    resets_at_ms: Ms,
}

/// Rule 2: the group anchored on the latest LIVE reset.
fn winning_group(live: &[(Ms, &Observation)]) -> Option<CliWinner> {
    let anchor = live.iter().map(|&(r, _)| r).max()?;
    let floor = anchor.saturating_sub(RESET_GROUP_TOLERANCE_MS);
    let (pct, observed_at_ms) = live
        .iter()
        .filter(|&&(r, _)| r >= floor)
        .fold((0.0_f32, Ms::MIN), |(p, t), &(_, o)| (p.max(sanitize_pct(o.pct)), t.max(o.observed_at_ms)));
    Some(CliWinner { pct, observed_at_ms, resets_at_ms: anchor })
}

/// The merged state before the derived `stale` / `limit_reached` flags are computed.
struct Draft {
    pct: f32,
    reset: ResetInfo,
    source: Source,
    observed_at_ms: Ms,
    phase: Phase,
}

impl Draft {
    fn active(pct: f32, reset: ResetInfo, source: Source, observed_at_ms: Ms) -> Self {
        Self { pct, reset, source, observed_at_ms, phase: Phase::Active }
    }

    fn awaiting(reset: ResetInfo, source: Source, observed_at_ms: Ms) -> Self {
        Self { pct: 0.0, reset, source, observed_at_ms, phase: Phase::ResetAwaitingData }
    }

    fn finish(self, kind: WindowKind, now_ms: Ms, stale_after_ms: Ms) -> WindowState {
        let pct = sanitize_pct(self.pct);
        let stale = self.phase == Phase::Active && now_ms.saturating_sub(self.observed_at_ms) > stale_after_ms;
        WindowState {
            kind,
            pct,
            reset: self.reset,
            source: self.source,
            observed_at_ms: self.observed_at_ms,
            stale,
            limit_reached: pct >= LIMIT_REACHED_PCT,
            phase: self.phase,
        }
    }
}

/// Clamps to `0..=100`; NaN (malformed input) becomes 0.
fn sanitize_pct(pct: f32) -> f32 {
    if pct.is_nan() { 0.0 } else { pct.clamp(0.0, 100.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::Confidence;
    use crate::time::{HOUR_MS, MINUTE_MS, SECOND_MS};
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    const NOW: Ms = 1_790_000_000_000;
    const STALE: Ms = 30 * MINUTE_MS;
    const FH: WindowKind = WindowKind::FiveHour;

    /// CLI observation of the 5h window: resets `reset_in` after now, observed `ago` before now.
    fn cli(pct: f32, reset_in: Ms, ago: Ms) -> Observation {
        cli_kind(FH, pct, Some(NOW + reset_in), ago)
    }

    fn cli_kind(kind: WindowKind, pct: f32, resets_at_ms: Option<Ms>, ago: Ms) -> Observation {
        Observation { kind, pct, resets_at_ms, observed_at_ms: NOW - ago, source: Source::Cli }
    }

    fn desk(pct: f32, ago: Ms) -> Observation {
        Observation { kind: FH, pct, resets_at_ms: None, observed_at_ms: NOW - ago, source: Source::Desktop }
    }

    fn est(at_ms: Ms) -> ResetInfo {
        ResetInfo::Estimated { at_ms, plus_minus_ms: 10 * MINUTE_MS, confidence: Confidence::Medium }
    }

    fn exact(in_ms: Ms) -> ResetInfo {
        ResetInfo::Exact { at_ms: NOW + in_ms }
    }

    fn merge(cli: &[Observation], desktop: Option<&Observation>, estimate: ResetInfo) -> Option<WindowState> {
        merge_window(&FH, cli, desktop, estimate, NOW, STALE)
    }

    // ---- rule 1: live vs expired ----

    #[test]
    fn live_cli_gives_exact_reset() {
        let s = merge(&[cli(22.5, 2 * HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!(
            s,
            WindowState {
                kind: FH,
                pct: 22.5,
                reset: exact(2 * HOUR_MS),
                source: Source::Cli,
                observed_at_ms: NOW - MINUTE_MS,
                stale: false,
                limit_reached: false,
                phase: Phase::Active,
            }
        );
    }

    #[test]
    fn live_beats_expired() {
        let obs = [cli(90.0, -HOUR_MS, 2 * HOUR_MS), cli(30.0, 2 * HOUR_MS, 10 * MINUTE_MS)];
        let s = merge(&obs, None, ResetInfo::Unknown).unwrap();
        assert_eq!((s.pct, s.reset, s.phase), (30.0, exact(2 * HOUR_MS), Phase::Active));
    }

    #[test]
    fn reset_exactly_now_is_expired() {
        let s = merge(&[cli(40.0, 0, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!((s.phase, s.pct), (Phase::ResetAwaitingData, 0.0));
    }

    #[test]
    fn other_kinds_are_ignored() {
        let weekly = cli_kind(WindowKind::SevenDay, 70.0, Some(NOW + 3 * HOUR_MS), MINUTE_MS);
        assert_eq!(merge(std::slice::from_ref(&weekly), None, ResetInfo::Unknown), None);
        let s = merge(&[weekly, cli(10.0, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!(s.pct, 10.0);
    }

    #[test]
    fn cli_without_reset_is_ignored() {
        let no_reset = cli_kind(FH, 50.0, None, MINUTE_MS);
        assert_eq!(merge(std::slice::from_ref(&no_reset), None, ResetInfo::Unknown), None);
        let s = merge(&[no_reset, cli(20.0, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!(s.pct, 20.0);
    }

    // ---- rule 2: grouping ----

    #[test]
    fn resets_within_tolerance_are_one_group_with_max_pct() {
        let r = 3 * HOUR_MS;
        let obs = [cli(40.0, r, 5 * MINUTE_MS), cli(42.0, r - 90 * SECOND_MS, 10 * MINUTE_MS)];
        let s = merge(&obs, None, ResetInfo::Unknown).unwrap();
        assert_eq!(s.pct, 42.0);
        assert_eq!(s.observed_at_ms, NOW - 5 * MINUTE_MS, "newest observation of the group");
        assert_eq!(s.reset, exact(r), "latest reset of the group");
    }

    #[test]
    fn group_tolerance_boundary() {
        let r = 3 * HOUR_MS;
        let at_edge = [cli(10.0, r, MINUTE_MS), cli(80.0, r - RESET_GROUP_TOLERANCE_MS, MINUTE_MS)];
        assert_eq!(merge(&at_edge, None, ResetInfo::Unknown).unwrap().pct, 80.0);
        let beyond = [cli(10.0, r, MINUTE_MS), cli(80.0, r - RESET_GROUP_TOLERANCE_MS - 1, MINUTE_MS)];
        assert_eq!(merge(&beyond, None, ResetInfo::Unknown).unwrap().pct, 10.0);
    }

    #[test]
    fn idle_session_with_older_lower_value_does_not_win() {
        let r = 2 * HOUR_MS;
        let active = cli(55.0, r, MINUTE_MS);
        let idle = cli(20.0, r, 3 * HOUR_MS);
        for obs in [[active.clone(), idle.clone()], [idle, active]] {
            let s = merge(&obs, None, ResetInfo::Unknown).unwrap();
            assert_eq!((s.pct, s.observed_at_ms, s.stale), (55.0, NOW - MINUTE_MS, false));
        }
    }

    #[test]
    fn latest_reset_group_wins_over_higher_older_window() {
        // An idle session still reports the pre-early-reset window (its reset is still ahead).
        let obs = [cli(90.0, HOUR_MS, 2 * HOUR_MS), cli(5.0, 4 * HOUR_MS, MINUTE_MS)];
        let s = merge(&obs, None, ResetInfo::Unknown).unwrap();
        assert_eq!((s.pct, s.reset), (5.0, exact(4 * HOUR_MS)));
    }

    // ---- rule 3a / 3b: CLI and Desktop ----

    #[test]
    fn newer_consistent_desktop_keeps_exact_reset() {
        let c = cli(40.3, 2 * HOUR_MS, 10 * MINUTE_MS);
        for d_pct in [41.0, 40.0, 39.5, 55.0] {
            let d = desk(d_pct, 2 * MINUTE_MS);
            let s = merge(std::slice::from_ref(&c), Some(&d), est(NOW + 5 * HOUR_MS)).unwrap();
            assert_eq!(
                (s.pct, s.reset, s.source, s.observed_at_ms, s.phase),
                (d_pct, exact(2 * HOUR_MS), Source::Desktop, NOW - 2 * MINUTE_MS, Phase::Active),
                "desktop {d_pct}"
            );
        }
    }

    #[test]
    fn desktop_tolerance_boundary() {
        let c = cli(40.0, 2 * HOUR_MS, 10 * MINUTE_MS);
        let within = desk(38.1, MINUTE_MS);
        let s = merge(std::slice::from_ref(&c), Some(&within), est(NOW + 5 * HOUR_MS)).unwrap();
        assert_eq!(s.reset, exact(2 * HOUR_MS), "a drop below two points is noise");
        let beyond = desk(38.0, MINUTE_MS);
        let s = merge(std::slice::from_ref(&c), Some(&beyond), est(NOW + 5 * HOUR_MS)).unwrap();
        assert_eq!(s.reset, est(NOW + 5 * HOUR_MS));
    }

    #[test]
    fn desktop_drop_is_early_reset_with_estimate() {
        let c = cli(60.0, 2 * HOUR_MS, 20 * MINUTE_MS);
        let d = desk(12.0, MINUTE_MS);
        let estimate = est(NOW + 4 * HOUR_MS);
        let s = merge(&[c], Some(&d), estimate.clone()).unwrap();
        assert_eq!(
            (s.pct, s.reset, s.source, s.phase, s.observed_at_ms),
            (12.0, estimate, Source::Desktop, Phase::Active, NOW - MINUTE_MS)
        );
    }

    #[test]
    fn cli_decimal_then_lagging_desktop_integer_is_not_a_reset() {
        let c = cli(80.2, 2 * HOUR_MS, 10 * MINUTE_MS);
        let s = merge(&[c], Some(&desk(79.0, MINUTE_MS)), est(NOW + 5 * HOUR_MS)).unwrap();
        assert_eq!((s.pct, s.reset), (79.0, exact(2 * HOUR_MS)));
    }

    #[test]
    fn early_reset_uses_the_desktop_estimate_not_the_cli_reset() {
        let c = cli(60.0, 2 * HOUR_MS, 20 * MINUTE_MS);
        let d = desk(12.0, MINUTE_MS);
        let desktop_estimate = est(NOW + 4 * HOUR_MS);
        let s =
            merge_window_with(&FH, &[c], Some(&d), exact(2 * HOUR_MS), desktop_estimate.clone(), NOW, STALE).unwrap();
        assert_eq!((s.pct, s.reset), (12.0, desktop_estimate));
        // Without an early reset the other estimate is unused: the CLI reset stays exact.
        let c = cli(60.0, 2 * HOUR_MS, 20 * MINUTE_MS);
        let s = merge_window_with(&FH, &[c], Some(&desk(61.0, MINUTE_MS)), ResetInfo::Unknown, est(NOW), NOW, STALE)
            .unwrap();
        assert_eq!(s.reset, exact(2 * HOUR_MS));
    }

    #[test]
    fn desktop_observed_after_cli_reset_is_a_later_window() {
        // Only reachable with clock skew: C is LIVE, so D must be stamped after `now`.
        let c = cli(30.0, MINUTE_MS, 10 * MINUTE_MS);
        let d = desk(35.0, -2 * MINUTE_MS);
        let estimate = est(NOW + 5 * HOUR_MS);
        let s = merge(&[c], Some(&d), estimate.clone()).unwrap();
        assert_eq!((s.pct, s.reset, s.source), (35.0, estimate, Source::Desktop));
        let at_reset = desk(35.0, -MINUTE_MS);
        let s = merge(&[cli(30.0, MINUTE_MS, 10 * MINUTE_MS)], Some(&at_reset), ResetInfo::Unknown).unwrap();
        assert_eq!(s.reset, ResetInfo::Unknown, "D.observed_at == C.resets_at is a later window");
    }

    #[test]
    fn older_or_equal_desktop_loses_to_cli() {
        let c = cli(50.0, 2 * HOUR_MS, 5 * MINUTE_MS);
        for ago in [30 * MINUTE_MS, 5 * MINUTE_MS] {
            let d = desk(10.0, ago);
            let s = merge(std::slice::from_ref(&c), Some(&d), est(NOW + HOUR_MS)).unwrap();
            assert_eq!(
                (s.pct, s.reset, s.source, s.observed_at_ms),
                (50.0, exact(2 * HOUR_MS), Source::Cli, NOW - 5 * MINUTE_MS)
            );
        }
    }

    #[test]
    fn desktop_of_another_kind_is_ignored() {
        let mut d = desk(10.0, MINUTE_MS);
        d.kind = WindowKind::SevenDay;
        let s = merge(&[cli(50.0, HOUR_MS, 5 * MINUTE_MS)], Some(&d), ResetInfo::Unknown).unwrap();
        assert_eq!(s.source, Source::Cli);
        assert_eq!(merge(&[], Some(&d), ResetInfo::Unknown), None);
    }

    // ---- rule 3c: Desktop only ----

    #[test]
    fn desktop_only_uses_estimate() {
        let estimate = est(NOW + 3 * HOUR_MS);
        let s = merge(&[], Some(&desk(29.0, 5 * MINUTE_MS)), estimate.clone()).unwrap();
        assert_eq!(
            s,
            WindowState {
                kind: FH,
                pct: 29.0,
                reset: estimate,
                source: Source::Desktop,
                observed_at_ms: NOW - 5 * MINUTE_MS,
                stale: false,
                limit_reached: false,
                phase: Phase::Active,
            }
        );
    }

    #[test]
    fn expired_cli_reset_after_desktop_sample_awaits_data() {
        let d = desk(64.0, 2 * HOUR_MS);
        let expired = cli(70.0, -HOUR_MS, 3 * HOUR_MS);
        let estimate = est(NOW + 2 * HOUR_MS);
        let s = merge(&[expired], Some(&d), estimate.clone()).unwrap();
        assert_eq!(
            (s.phase, s.pct, s.reset, s.source, s.observed_at_ms, s.stale),
            (Phase::ResetAwaitingData, 0.0, estimate, Source::Desktop, NOW - 2 * HOUR_MS, false)
        );
    }

    #[test]
    fn expired_cli_reset_before_desktop_sample_keeps_desktop() {
        let d = desk(8.0, 10 * MINUTE_MS);
        let expired = cli(70.0, -HOUR_MS, 2 * HOUR_MS);
        let s = merge(&[expired], Some(&d), ResetInfo::Unknown).unwrap();
        assert_eq!((s.phase, s.pct, s.source), (Phase::Active, 8.0, Source::Desktop));
    }

    #[test]
    fn expired_cli_reset_exactly_at_desktop_sample_keeps_desktop() {
        let d = desk(8.0, HOUR_MS);
        let expired = cli(70.0, -HOUR_MS, 2 * HOUR_MS);
        let s = merge(&[expired], Some(&d), ResetInfo::Unknown).unwrap();
        assert_eq!(s.phase, Phase::Active, "the interval is (D.observed_at, now]");
    }

    #[test]
    fn estimate_in_the_past_after_desktop_sample_awaits_data() {
        let d = desk(64.0, 2 * HOUR_MS);
        let estimate = est(NOW - 30 * MINUTE_MS);
        let s = merge(&[], Some(&d), estimate.clone()).unwrap();
        assert_eq!((s.phase, s.pct, s.reset), (Phase::ResetAwaitingData, 0.0, estimate));
        assert_eq!(merge(&[], Some(&d), est(NOW)).unwrap().phase, Phase::ResetAwaitingData);
    }

    #[test]
    fn estimate_before_desktop_sample_or_in_future_stays_active() {
        let d = desk(64.0, 2 * HOUR_MS);
        for estimate in [est(NOW - 3 * HOUR_MS), est(NOW - 2 * HOUR_MS), est(NOW + MINUTE_MS), ResetInfo::Unknown] {
            let s = merge(&[], Some(&d), estimate.clone()).unwrap();
            assert_eq!((s.phase, s.pct), (Phase::Active, 64.0), "{estimate:?}");
        }
    }

    // ---- rule 3d / 3e ----

    #[test]
    fn only_expired_cli_awaits_data() {
        let obs = [cli(88.0, -2 * HOUR_MS, 3 * HOUR_MS), cli(91.0, -HOUR_MS, 90 * MINUTE_MS)];
        let estimate = est(NOW + 4 * HOUR_MS);
        let s = merge(&obs, None, estimate.clone()).unwrap();
        assert_eq!(
            s,
            WindowState {
                kind: FH,
                pct: 0.0,
                reset: estimate,
                source: Source::Cli,
                observed_at_ms: NOW - 90 * MINUTE_MS,
                stale: false,
                limit_reached: false,
                phase: Phase::ResetAwaitingData,
            }
        );
    }

    #[test]
    fn nothing_is_none() {
        assert_eq!(merge(&[], None, ResetInfo::Unknown), None);
        assert_eq!(merge(&[], None, est(NOW - HOUR_MS)), None);
    }

    // ---- rule 4: derived flags ----

    #[test]
    fn stale_flag() {
        let fresh = merge(&[cli(10.0, HOUR_MS, STALE)], None, ResetInfo::Unknown).unwrap();
        assert!(!fresh.stale, "exactly stale_after old is not stale");
        let old = merge(&[cli(10.0, HOUR_MS, STALE + 1)], None, ResetInfo::Unknown).unwrap();
        assert!(old.stale);
        let d = merge(&[], Some(&desk(10.0, 2 * HOUR_MS)), ResetInfo::Unknown).unwrap();
        assert!(d.stale);
    }

    #[test]
    fn limit_reached_threshold() {
        let at = merge(&[cli(99.5, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert!(at.limit_reached);
        let below = merge(&[cli(99.4, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert!(!below.limit_reached);
        let desktop = merge(&[], Some(&desk(100.0, MINUTE_MS)), ResetInfo::Unknown).unwrap();
        assert!(desktop.limit_reached);
        let desktop99 = merge(&[], Some(&desk(99.0, MINUTE_MS)), ResetInfo::Unknown).unwrap();
        assert!(!desktop99.limit_reached);
    }

    #[test]
    fn pct_is_clamped_and_nan_is_zero() {
        let high = merge(&[cli(120.0, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!((high.pct, high.limit_reached), (100.0, true));
        let low = merge(&[cli(-5.0, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!(low.pct, 0.0);
        let nan = merge(&[cli(f32::NAN, HOUR_MS, MINUTE_MS)], None, ResetInfo::Unknown).unwrap();
        assert_eq!(nan.pct, 0.0);
        let d = merge(&[], Some(&desk(150.0, MINUTE_MS)), ResetInfo::Unknown).unwrap();
        assert_eq!(d.pct, 100.0);
        let d_nan = merge(&[], Some(&desk(f32::NAN, MINUTE_MS)), ResetInfo::Unknown).unwrap();
        assert_eq!(d_nan.pct, 0.0);
    }

    #[test]
    fn extreme_times_do_not_panic() {
        let obs = [cli_kind(FH, 10.0, Some(Ms::MAX), NOW - Ms::MIN / 2), cli_kind(FH, 10.0, Some(Ms::MIN), 0)];
        let d = desk(5.0, NOW);
        let s = merge_window(&FH, &obs, Some(&d), est(Ms::MIN), Ms::MAX, Ms::MAX);
        assert!(s.is_some());
        let s = merge_window(&FH, &obs, None, ResetInfo::Unknown, Ms::MIN, 0).unwrap();
        assert_eq!(s.reset, ResetInfo::Exact { at_ms: Ms::MAX });
    }

    // ---- properties ----

    fn obs_strategy() -> impl Strategy<Value = Observation> {
        let kind = prop_oneof![Just(WindowKind::FiveHour), Just(WindowKind::SevenDay)];
        // Resets cluster around a few anchors so tolerance grouping is exercised.
        let reset = proptest::option::weighted(
            0.9,
            (proptest::sample::select(vec![-2 * HOUR_MS, HOUR_MS, 3 * HOUR_MS]), -150 * SECOND_MS..150 * SECOND_MS)
                .prop_map(|(base, jitter)| NOW + base + jitter),
        );
        (kind, 0.0_f32..=100.0, reset, 0..10 * HOUR_MS).prop_map(|(kind, pct, resets_at_ms, ago)| Observation {
            kind,
            pct,
            resets_at_ms,
            observed_at_ms: NOW - ago,
            source: Source::Cli,
        })
    }

    fn desktop_strategy() -> impl Strategy<Value = Option<Observation>> {
        proptest::option::of((0.0_f32..=100.0, -MINUTE_MS..10 * HOUR_MS).prop_map(|(pct, ago)| desk(pct.round(), ago)))
    }

    fn estimate_strategy() -> impl Strategy<Value = ResetInfo> {
        prop_oneof![
            Just(ResetInfo::Unknown),
            (-6 * HOUR_MS..6 * HOUR_MS).prop_map(|o| est(NOW + o)),
            (-6 * HOUR_MS..6 * HOUR_MS).prop_map(|o| ResetInfo::Exact { at_ms: NOW + o }),
        ]
    }

    fn cli_and_shuffled() -> impl Strategy<Value = (Vec<Observation>, Vec<Observation>)> {
        proptest::collection::vec(obs_strategy(), 0..8).prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle()))
    }

    proptest! {
        #[test]
        fn invariant_under_permutation(
            (cli, shuffled) in cli_and_shuffled(),
            desktop in desktop_strategy(),
            estimate in estimate_strategy(),
        ) {
            let a = merge(&cli, desktop.as_ref(), estimate.clone());
            let b = merge(&shuffled, desktop.as_ref(), estimate);
            prop_assert_eq!(a, b);
        }

        #[test]
        fn merging_is_idempotent(
            cli in proptest::collection::vec(obs_strategy(), 0..8),
            desktop in desktop_strategy(),
            estimate in estimate_strategy(),
        ) {
            let once = merge(&cli, desktop.as_ref(), estimate.clone());
            let doubled: Vec<Observation> = cli.iter().chain(cli.iter()).cloned().collect();
            prop_assert_eq!(&merge(&doubled, desktop.as_ref(), estimate.clone()), &once);
            prop_assert_eq!(&merge(&cli, desktop.as_ref(), estimate), &once);
        }

        #[test]
        fn output_invariants(
            cli in proptest::collection::vec(obs_strategy(), 0..8),
            desktop in desktop_strategy(),
            estimate in estimate_strategy(),
        ) {
            if let Some(s) = merge(&cli, desktop.as_ref(), estimate) {
                prop_assert!((0.0..=100.0).contains(&s.pct));
                prop_assert_eq!(s.limit_reached, s.pct >= LIMIT_REACHED_PCT);
                if s.phase == Phase::ResetAwaitingData {
                    prop_assert_eq!(s.pct, 0.0);
                    prop_assert!(!s.stale);
                }
                if s.source == Source::Cli && s.phase == Phase::Active {
                    prop_assert!(s.reset.is_exact());
                }
            }
        }
    }
}
