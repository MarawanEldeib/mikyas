//! Limit alerts: fire once per window instance when usage crosses a threshold, and optionally
//! when a window resets. Pure state machine; the app persists [`AlertState`] to `alerts.json`
//! and turns [`AlertEvent`]s into OS notifications.
//!
//! Window instance identity (per kind):
//! - The instance key is the reset `at_ms` rounded to 5 minutes (exact or estimated).
//! - A changed key within the alias window ([`alias_between`]) of the stored one, with no pct
//!   drop, is the SAME instance (an alias — e.g. switching between an estimated and an exact
//!   reset); update the stored key. The window is the kind's own ([`alias_ms`]: ±30 min for
//!   five_hour, ±1 day for weekly and other kinds, whose estimates carry ±1 day), widened to the
//!   `plus_minus_ms` of the stored or the new reset time (an estimate can be that far off the exact
//!   time that replaces it), but never beyond half the window's length.
//! - A NEW instance starts when pct drops by [`RESET_DROP_PCT`] (2) points or more versus the
//!   stored `last_pct` (the reset rule of the whole engine: Desktop's integers lag the CLI's
//!   decimals, so 80.2 then 79 is one window), the phase is ResetAwaitingData, or the key moves by
//!   more than the alias window. On a new instance `fired` is cleared.
//!   If the previous instance's `last_pct > 0`, `settings.notify_reset` is on and `first_run` is
//!   false, emit `Reset { kind }`.
//! - Unknown reset + no drop → same instance.
//!
//! Thresholds: after instance handling, for the highest threshold `t` in `settings.thresholds`
//! with `pct >= t` that is not yet in `fired`, emit ONE `Threshold` event (the highest crossed)
//! and mark every threshold `<= pct` as fired (jumping 50 → 97 alerts once, for 95).
//! Stale windows never alert (neither threshold nor reset).
//!
//! Implementation notes:
//! - Keys are rounded to the NEAREST multiple of [`INSTANCE_ROUND_MS`].
//! - A stale window is skipped entirely (its stored state is not touched either), so a reset or
//!   crossing that happened while the data was stale is reported once fresh data arrives.
//! - A kind seen for the first time starts from `KindAlertState::default()` (no key, `last_pct`
//!   0), so it never emits `Reset`, but does alert for thresholds it is already above.
//! - An unknown reset keeps the stored key, so a later exact/estimated key is compared with it.
//! - pct is sanitised first (NaN → 0, clamped to 0..=100): serde_json writes non-finite floats as
//!   `null`, which would make the persisted state unreadable. Missing fields in a persisted state
//!   take their defaults.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::engine::types::{Phase, ResetInfo, WindowKind, WindowState, is_reset_drop};
use crate::time::{DAY_MS, MINUTE_MS, Ms};

pub use crate::engine::types::RESET_DROP_PCT;

/// Reset times are rounded to this before being used as an instance key.
pub const INSTANCE_ROUND_MS: Ms = 5 * MINUTE_MS;
/// Five-hour keys closer than this (inclusive) belong to the same window instance.
pub const INSTANCE_ALIAS_MS: Ms = 30 * MINUTE_MS;
/// Weekly (and other) keys closer than this (inclusive) belong to the same window instance:
/// weekly reset estimates carry ±1 day.
pub const WEEKLY_INSTANCE_ALIAS_MS: Ms = DAY_MS;

/// Alias window of a kind (see the module docs).
pub fn alias_ms(kind: &WindowKind) -> Ms {
    match kind {
        WindowKind::FiveHour => INSTANCE_ALIAS_MS,
        _ => WEEKLY_INSTANCE_ALIAS_MS,
    }
}

/// Largest distance (inclusive) between two instance keys of the same window instance, given the
/// `plus_minus_ms` of both reset times: the kind's [`alias_ms`], widened to the larger
/// uncertainty, but at most half the window's length (consecutive windows' resets are a whole
/// window apart). Shared with `pace_alerts`.
pub fn alias_between(kind: &WindowKind, plus_minus_a: Ms, plus_minus_b: Ms) -> Ms {
    let base = alias_ms(kind);
    let widened = base.max(plus_minus_a).max(plus_minus_b);
    match kind.duration_ms() {
        Some(duration) => widened.min(base.max(duration / 2)),
        None => widened,
    }
}

/// How far off a reset time may be: its `plus_minus_ms` when estimated, else 0.
pub fn plus_minus(reset: &ResetInfo) -> Ms {
    match reset {
        ResetInfo::Estimated { plus_minus_ms, .. } => (*plus_minus_ms).max(0),
        ResetInfo::Exact { .. } | ResetInfo::Unknown => 0,
    }
}

/// User-configurable alert behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertSettings {
    /// Ascending percentages, default `[80, 95]`.
    pub thresholds: Vec<u8>,
    pub notify_reset: bool,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self { thresholds: vec![80, 95], notify_reset: true }
    }
}

/// Something the app should turn into an OS notification.
#[derive(Debug, Clone, PartialEq)]
pub enum AlertEvent {
    Threshold { kind: WindowKind, threshold: u8, pct: f32, reset_at_ms: Option<Ms> },
    Reset { kind: WindowKind },
}

/// Alert bookkeeping for one window kind.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KindAlertState {
    pub instance_key: Option<Ms>,
    pub fired: BTreeSet<u8>,
    pub last_pct: f32,
    /// Uncertainty of the reset time behind `instance_key` (0 when exact or unknown).
    #[serde(skip_serializing_if = "is_zero")]
    pub plus_minus_ms: Ms,
}

fn is_zero(ms: &Ms) -> bool {
    *ms == 0
}

/// Persisted as JSON; keyed by `WindowKind::key()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertState {
    pub kinds: BTreeMap<String, KindAlertState>,
}

impl AlertState {
    /// Advances the state machine with the current window states and returns the events to show,
    /// in window order.
    ///
    /// `first_run` = this is the first evaluation since the app started (suppresses Reset toasts
    /// for resets that happened while the app was closed).
    pub fn evaluate(&mut self, windows: &[WindowState], settings: &AlertSettings, first_run: bool) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        for window in windows.iter().filter(|w| !w.stale) {
            let entry = self.kinds.entry(window.kind.key().to_owned()).or_default();
            entry.observe(window, settings, first_run, &mut events);
        }
        events
    }
}

impl KindAlertState {
    fn observe(
        &mut self,
        window: &WindowState,
        settings: &AlertSettings,
        first_run: bool,
        events: &mut Vec<AlertEvent>,
    ) {
        let pct = if window.pct.is_nan() { 0.0 } else { window.pct.clamp(0.0, 100.0) };
        let key = window.reset.at_ms().map(instance_key);
        let plus_minus_ms = plus_minus(&window.reset);

        let dropped = is_reset_drop(self.last_pct, pct);
        let alias = alias_between(&window.kind, self.plus_minus_ms, plus_minus_ms);
        let moved_far = matches!(
            (self.instance_key, key),
            (Some(old), Some(new)) if old.abs_diff(new) > alias.unsigned_abs()
        );
        if window.phase == Phase::ResetAwaitingData || dropped || moved_far {
            if self.last_pct > 0.0 && settings.notify_reset && !first_run {
                events.push(AlertEvent::Reset { kind: window.kind.clone() });
            }
            self.fired.clear();
            self.instance_key = key;
            self.plus_minus_ms = plus_minus_ms;
        } else if key.is_some() {
            // Same instance: adopt an alias (or a first known key); an unknown reset keeps the old one.
            self.instance_key = key;
            self.plus_minus_ms = plus_minus_ms;
        }

        let crossed = || settings.thresholds.iter().copied().filter(|&t| pct >= f32::from(t));
        if let Some(threshold) = crossed().filter(|t| !self.fired.contains(t)).max() {
            events.push(AlertEvent::Threshold {
                kind: window.kind.clone(),
                threshold,
                pct,
                reset_at_ms: window.reset.at_ms(),
            });
            self.fired.extend(crossed());
        }
        self.last_pct = pct;
    }
}

/// Rounds a reset time to the nearest [`INSTANCE_ROUND_MS`] (saturating at the extremes). Shared
/// with `pace_alerts`.
pub(crate) fn instance_key(at_ms: Ms) -> Ms {
    at_ms.saturating_add(INSTANCE_ROUND_MS / 2).div_euclid(INSTANCE_ROUND_MS).saturating_mul(INSTANCE_ROUND_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Confidence, ResetInfo, Source};
    use crate::time::{DAY_MS, HOUR_MS};
    use pretty_assertions::assert_eq;

    const T0: Ms = 1_790_000_100_000; // a multiple of 5 min
    const R: Ms = T0 + 3 * HOUR_MS;

    fn win(kind: WindowKind, pct: f32, reset: ResetInfo) -> WindowState {
        WindowState {
            kind,
            pct,
            reset,
            source: Source::Cli,
            observed_at_ms: T0,
            stale: false,
            limit_reached: pct >= crate::engine::merge::LIMIT_REACHED_PCT,
            phase: Phase::Active,
        }
    }

    fn fh(pct: f32) -> WindowState {
        win(WindowKind::FiveHour, pct, ResetInfo::Exact { at_ms: R })
    }

    fn fh_at(pct: f32, at_ms: Ms) -> WindowState {
        win(WindowKind::FiveHour, pct, ResetInfo::Exact { at_ms })
    }

    fn awaiting() -> WindowState {
        let mut w = win(WindowKind::FiveHour, 0.0, ResetInfo::Unknown);
        w.phase = Phase::ResetAwaitingData;
        w
    }

    fn threshold(t: u8, pct: f32, reset_at_ms: Option<Ms>) -> AlertEvent {
        AlertEvent::Threshold { kind: WindowKind::FiveHour, threshold: t, pct, reset_at_ms }
    }

    fn reset_event() -> AlertEvent {
        AlertEvent::Reset { kind: WindowKind::FiveHour }
    }

    /// Evaluates one 5h window with default settings, not first run.
    fn step(state: &mut AlertState, w: WindowState) -> Vec<AlertEvent> {
        state.evaluate(&[w], &AlertSettings::default(), false)
    }

    fn fh_state(state: &AlertState) -> &KindAlertState {
        &state.kinds["five_hour"]
    }

    #[test]
    fn crossing_80_then_95_fires_once_each() {
        let mut s = AlertState::default();
        assert_eq!(step(&mut s, fh(50.0)), vec![]);
        assert_eq!(step(&mut s, fh(81.0)), vec![threshold(80, 81.0, Some(R))]);
        assert_eq!(step(&mut s, fh(85.0)), vec![]);
        assert_eq!(step(&mut s, fh(95.0)), vec![threshold(95, 95.0, Some(R))]);
        assert_eq!(step(&mut s, fh(99.0)), vec![]);
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80, 95]));
    }

    #[test]
    fn jump_fires_only_highest_and_marks_lower() {
        let mut s = AlertState::default();
        step(&mut s, fh(50.0));
        assert_eq!(step(&mut s, fh(97.0)), vec![threshold(95, 97.0, Some(R))]);
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80, 95]));
        assert_eq!(step(&mut s, fh(98.0)), vec![]);
    }

    #[test]
    fn same_state_fires_nothing_twice() {
        let mut s = AlertState::default();
        assert_eq!(step(&mut s, fh(85.0)), vec![threshold(80, 85.0, Some(R))]);
        let snapshot = s.clone();
        assert_eq!(step(&mut s, fh(85.0)), vec![]);
        assert_eq!(s, snapshot);
    }

    #[test]
    fn first_sight_above_threshold_alerts_without_reset() {
        let mut s = AlertState::default();
        assert_eq!(step(&mut s, fh(90.0)), vec![threshold(80, 90.0, Some(R))]);
        assert_eq!(fh_state(&s).instance_key, Some(R));
        assert_eq!(fh_state(&s).last_pct, 90.0);
    }

    #[test]
    fn restart_does_not_refire() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, format!(r#"{{"kinds":{{"five_hour":{{"instance_key":{R},"fired":[80],"last_pct":85.0}}}}}}"#));
        let mut restored: AlertState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s);
        let events = restored.evaluate(&[fh(85.0)], &AlertSettings::default(), true);
        assert_eq!(events, vec![]);
        assert_eq!(restored.evaluate(&[fh(86.0)], &AlertSettings::default(), false), vec![]);
    }

    #[test]
    fn reset_while_closed_is_silent_on_first_run_but_rearms() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        let mut restored: AlertState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let next = R + 5 * HOUR_MS;
        let events = restored.evaluate(&[fh_at(10.0, next)], &AlertSettings::default(), true);
        assert_eq!(events, vec![], "no Reset toast on first run");
        assert!(fh_state(&restored).fired.is_empty());
        assert_eq!(fh_state(&restored).instance_key, Some(next));
        assert_eq!(step(&mut restored, fh_at(81.0, next)), vec![threshold(80, 81.0, Some(next))]);
    }

    #[test]
    fn alias_within_30_min_keeps_instance() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        let estimated = win(
            WindowKind::FiveHour,
            85.5,
            ResetInfo::Estimated {
                at_ms: R + 20 * MINUTE_MS + 1_000,
                plus_minus_ms: 10 * MINUTE_MS,
                confidence: Confidence::High,
            },
        );
        assert_eq!(step(&mut s, estimated), vec![]);
        assert_eq!(fh_state(&s).instance_key, Some(R + 20 * MINUTE_MS), "alias adopted, rounded");
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80]));
        // Exactly 30 min from the (updated) stored key is still an alias.
        assert_eq!(step(&mut s, fh_at(86.0, R + 50 * MINUTE_MS)), vec![]);
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80]));
    }

    #[test]
    fn weekly_estimated_to_exact_switch_is_an_alias() {
        // Weekly estimates carry ±1 day: an exact time 20 h from the estimate is the same week.
        let settings = AlertSettings::default();
        let weekly_reset = T0 + 3 * DAY_MS;
        let estimated = |at_ms: Ms| {
            win(
                WindowKind::SevenDay,
                82.0,
                ResetInfo::Estimated { at_ms, plus_minus_ms: DAY_MS, confidence: Confidence::Low },
            )
        };
        let exact = win(WindowKind::SevenDay, 82.3, ResetInfo::Exact { at_ms: weekly_reset });
        let mut s = AlertState::default();
        assert_eq!(s.evaluate(&[estimated(weekly_reset - 20 * HOUR_MS)], &settings, false).len(), 1);
        assert_eq!(s.evaluate(std::slice::from_ref(&exact), &settings, false), vec![]);
        assert_eq!(s.kinds["seven_day"].instance_key, Some(weekly_reset));
        // 30 h apart is beyond the estimate's error: a new week.
        let mut s = AlertState::default();
        assert_eq!(s.evaluate(&[estimated(weekly_reset - 30 * HOUR_MS)], &settings, false).len(), 1);
        let events = s.evaluate(&[exact], &settings, false);
        assert_eq!(events[0], AlertEvent::Reset { kind: WindowKind::SevenDay });
        assert_eq!(events.len(), 2, "reset + re-armed threshold");
    }

    #[test]
    fn weekly_alias_window_is_one_day() {
        let settings = AlertSettings::default();
        let weekly = |pct: f32, at_ms: Ms| {
            win(
                WindowKind::SevenDay,
                pct,
                ResetInfo::Estimated { at_ms, plus_minus_ms: DAY_MS, confidence: Confidence::Low },
            )
        };
        let base = T0 + 3 * DAY_MS;
        let mut s = AlertState::default();
        assert_eq!(s.evaluate(&[weekly(82.0, base)], &settings, false).len(), 1);
        // A re-estimate 20 h later is the same week: nothing fires again.
        assert_eq!(s.evaluate(&[weekly(83.0, base + 20 * HOUR_MS)], &settings, false), vec![]);
        assert_eq!(s.kinds["seven_day"].instance_key, Some(base + 20 * HOUR_MS));
        // A key 26 h away from the stored one is a new week.
        let events = s.evaluate(&[weekly(84.0, base + 46 * HOUR_MS)], &settings, false);
        assert_eq!(events[0], AlertEvent::Reset { kind: WindowKind::SevenDay });
        assert_eq!(events.len(), 2, "reset + re-armed threshold");
        assert_eq!(alias_ms(&WindowKind::Other("seven_day_opus".into())), DAY_MS);
        assert_eq!(alias_ms(&WindowKind::FiveHour), 30 * MINUTE_MS);
    }

    #[test]
    fn key_moving_beyond_30_min_is_a_new_instance() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        let moved = R + 35 * MINUTE_MS;
        assert_eq!(step(&mut s, fh_at(86.0, moved)), vec![reset_event(), threshold(80, 86.0, Some(moved))]);
        assert_eq!(fh_state(&s).instance_key, Some(moved));
    }

    #[test]
    fn drop_of_two_points_is_a_new_instance_with_reset_event() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        assert_eq!(step(&mut s, fh(83.5)), vec![], "a drop below two points is noise");
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80]));
        assert_eq!(step(&mut s, fh(81.5)), vec![reset_event(), threshold(80, 81.5, Some(R))]);
        assert_eq!(step(&mut s, fh(20.0)), vec![reset_event()]);
        assert!(fh_state(&s).fired.is_empty());
        assert_eq!(step(&mut s, fh(81.0)), vec![threshold(80, 81.0, Some(R))]);
    }

    fn desktop_fh(pct: f32) -> WindowState {
        let mut w = fh(pct);
        w.source = Source::Desktop;
        w
    }

    #[test]
    fn mixed_source_dip_below_two_points_is_not_a_reset() {
        let mut s = AlertState::default();
        assert_eq!(step(&mut s, fh(80.2)), vec![threshold(80, 80.2, Some(R))]);
        assert_eq!(step(&mut s, desktop_fh(79.0)), vec![], "Desktop integer lag");
        assert_eq!(step(&mut s, fh(80.5)), vec![], "no second 80 % alert");
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80]));
    }

    #[test]
    fn integer_one_point_dip_is_noise() {
        let mut s = AlertState::default();
        assert_eq!(step(&mut s, fh(80.0)), vec![threshold(80, 80.0, Some(R))]);
        assert_eq!(step(&mut s, desktop_fh(79.0)), vec![]);
        assert_eq!(step(&mut s, fh(80.0)), vec![]);
    }

    #[test]
    fn five_hour_estimate_to_exact_switch_within_the_estimate_error_is_an_alias() {
        // A Low-confidence five-hour estimate (±1 h) 50 min from the exact time is the same window.
        let mut s = AlertState::default();
        let estimated = win(
            WindowKind::FiveHour,
            82.0,
            ResetInfo::Estimated { at_ms: R + 50 * MINUTE_MS, plus_minus_ms: HOUR_MS, confidence: Confidence::Low },
        );
        assert_eq!(step(&mut s, estimated), vec![threshold(80, 82.0, Some(R + 50 * MINUTE_MS))]);
        assert_eq!(step(&mut s, fh(82.5)), vec![], "exact time arrives: same window");
        assert_eq!(fh_state(&s).instance_key, Some(R));
        // Exact to exact keeps the narrow alias: 35 min is a new window.
        let moved = R + 35 * MINUTE_MS;
        assert_eq!(step(&mut s, fh_at(83.0, moved)), vec![reset_event(), threshold(80, 83.0, Some(moved))]);
    }

    #[test]
    fn reset_event_conditions() {
        // notify_reset off: still a new instance, but silent.
        let mut s = AlertState::default();
        let quiet = AlertSettings { notify_reset: false, ..AlertSettings::default() };
        s.evaluate(&[fh(85.0)], &quiet, false);
        assert_eq!(s.evaluate(&[fh(10.0)], &quiet, false), vec![]);
        assert!(fh_state(&s).fired.is_empty());

        // first_run: silent.
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        assert_eq!(s.evaluate(&[fh(10.0)], &AlertSettings::default(), true), vec![]);

        // Previous instance at 0%: nothing to announce.
        let mut s = AlertState::default();
        step(&mut s, fh(0.0));
        assert_eq!(step(&mut s, awaiting()), vec![]);
    }

    #[test]
    fn reset_awaiting_data_is_a_new_instance_once() {
        let mut s = AlertState::default();
        step(&mut s, fh(97.0));
        assert_eq!(step(&mut s, awaiting()), vec![reset_event()]);
        let st = fh_state(&s);
        assert!(st.fired.is_empty());
        assert_eq!((st.last_pct, st.instance_key), (0.0, None));
        assert_eq!(step(&mut s, awaiting()), vec![], "last_pct is now 0");
        let next = R + 5 * HOUR_MS;
        assert_eq!(step(&mut s, fh_at(2.0, next)), vec![]);
        assert_eq!(fh_state(&s).instance_key, Some(next));
    }

    #[test]
    fn stale_windows_never_alert_or_change_state() {
        let mut s = AlertState::default();
        let mut stale = fh(90.0);
        stale.stale = true;
        assert_eq!(step(&mut s, stale.clone()), vec![]);
        assert!(s.kinds.is_empty());

        step(&mut s, fh(85.0));
        let before = s.clone();
        let mut stale_drop = fh(5.0);
        stale_drop.stale = true;
        assert_eq!(step(&mut s, stale_drop), vec![]);
        let mut stale_rad = awaiting();
        stale_rad.stale = true;
        assert_eq!(step(&mut s, stale_rad), vec![]);
        assert_eq!(s, before);
        // Once fresh data shows the drop, the reset is announced.
        assert_eq!(step(&mut s, fh(5.0)), vec![reset_event()]);
    }

    #[test]
    fn custom_thresholds_in_any_order() {
        let settings = AlertSettings { thresholds: vec![90, 50, 75], notify_reset: true };
        let mut s = AlertState::default();
        let mut run = |pct: f32| s.evaluate(&[fh(pct)], &settings, false);
        assert_eq!(run(49.9), vec![]);
        assert_eq!(run(50.0), vec![threshold(50, 50.0, Some(R))]);
        assert_eq!(run(80.0), vec![threshold(75, 80.0, Some(R))]);
        assert_eq!(run(95.0), vec![threshold(90, 95.0, Some(R))]);
        assert_eq!(run(96.0), vec![]);

        let none = AlertSettings { thresholds: vec![], notify_reset: false };
        let mut s = AlertState::default();
        assert_eq!(s.evaluate(&[fh(100.0)], &none, false), vec![]);
    }

    #[test]
    fn unknown_reset_without_drop_is_same_instance() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        let unknown = win(WindowKind::FiveHour, 86.0, ResetInfo::Unknown);
        assert_eq!(step(&mut s, unknown), vec![]);
        assert_eq!(fh_state(&s).instance_key, Some(R), "stored key kept");
        assert_eq!(step(&mut s, fh(87.0)), vec![]);
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80]));
    }

    #[test]
    fn kinds_are_independent() {
        let mut s = AlertState::default();
        let weekly = win(WindowKind::SevenDay, 96.0, ResetInfo::Unknown);
        let events = s.evaluate(&[fh(81.0), weekly], &AlertSettings::default(), false);
        assert_eq!(
            events,
            vec![
                threshold(80, 81.0, Some(R)),
                AlertEvent::Threshold { kind: WindowKind::SevenDay, threshold: 95, pct: 96.0, reset_at_ms: None },
            ]
        );
        let opus = win(WindowKind::Other("seven_day_opus".into()), 10.0, ResetInfo::Unknown);
        s.evaluate(&[opus], &AlertSettings::default(), false);
        let keys: Vec<&str> = s.kinds.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["five_hour", "seven_day", "seven_day_opus"]);
    }

    #[test]
    fn alias_widens_to_the_estimate_error_up_to_half_a_window() {
        let fh = WindowKind::FiveHour;
        assert_eq!(alias_between(&fh, 0, 0), 30 * MINUTE_MS);
        assert_eq!(alias_between(&fh, HOUR_MS, 0), HOUR_MS);
        assert_eq!(alias_between(&fh, 0, 4 * HOUR_MS), 150 * MINUTE_MS, "capped at half of 5 h");
        assert_eq!(alias_between(&WindowKind::SevenDay, 2 * DAY_MS, 0), 2 * DAY_MS);
        let unknown = WindowKind::Other("x".into());
        assert_eq!(alias_between(&unknown, 3 * DAY_MS, 0), 3 * DAY_MS);
    }

    #[test]
    fn estimate_error_is_persisted() {
        let mut s = AlertState::default();
        let estimated = win(
            WindowKind::FiveHour,
            10.0,
            ResetInfo::Estimated { at_ms: R, plus_minus_ms: HOUR_MS, confidence: Confidence::Low },
        );
        step(&mut s, estimated);
        let restored: AlertState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(fh_state(&restored).plus_minus_ms, HOUR_MS);
        // An exact key 50 min away is still the same window after a restart.
        let mut restored = restored;
        assert_eq!(step(&mut restored, fh_at(11.0, R + 50 * MINUTE_MS)), vec![]);
        assert_eq!(fh_state(&restored).plus_minus_ms, 0);
    }

    #[test]
    fn instance_key_rounds_to_nearest_five_minutes() {
        assert_eq!(instance_key(T0), T0);
        assert_eq!(instance_key(T0 + 149_999), T0);
        assert_eq!(instance_key(T0 + 150_000), T0 + INSTANCE_ROUND_MS);
        assert_eq!(instance_key(T0 - 150_001), T0 - INSTANCE_ROUND_MS);
        // Extremes saturate instead of overflowing.
        let _ = instance_key(Ms::MAX);
        let _ = instance_key(Ms::MIN);
    }

    #[test]
    fn non_finite_pct_keeps_state_persistable() {
        // serde_json writes a non-finite f32 as `null`, which does not deserialize back into f32:
        // one bad value would make alerts.json unreadable and every threshold would re-fire.
        let mut s = AlertState::default();
        let events = step(&mut s, fh(f32::INFINITY));
        assert_eq!(events, vec![threshold(95, 100.0, Some(R))]);
        let json = serde_json::to_string(&s).unwrap();
        let restored: AlertState = serde_json::from_str(&json).expect("persisted state reloads");
        assert_eq!(restored, s);
        assert_eq!(fh_state(&restored).last_pct, 100.0);

        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        assert_eq!(step(&mut s, fh(f32::NEG_INFINITY)), vec![reset_event()]);
        assert_eq!(fh_state(&s).last_pct, 0.0);
        let restored: AlertState =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).expect("persisted state reloads");
        assert_eq!(restored, s);
    }

    #[test]
    fn persisted_state_tolerates_missing_fields() {
        // Older/hand-edited alerts.json: a missing field must not discard the whole state.
        let s: AlertState = serde_json::from_str(r#"{"kinds":{"five_hour":{"instance_key":123}}}"#).unwrap();
        assert_eq!(
            fh_state(&s),
            &KindAlertState { instance_key: Some(123), fired: BTreeSet::new(), last_pct: 0.0, plus_minus_ms: 0 }
        );
        let s: AlertState = serde_json::from_str(r#"{"kinds":{"five_hour":{"fired":[80]}}}"#).unwrap();
        assert_eq!(fh_state(&s).fired, BTreeSet::from([80]));
        let empty: AlertState = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, AlertState::default());
    }

    #[test]
    fn nan_pct_is_treated_as_zero() {
        let mut s = AlertState::default();
        step(&mut s, fh(85.0));
        assert_eq!(step(&mut s, fh(f32::NAN)), vec![reset_event()]);
        assert_eq!(fh_state(&s).last_pct, 0.0);
    }
}
