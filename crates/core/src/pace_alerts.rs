//! Pace alerts: warn BEFORE a limit is hit, and give a heads-up shortly before a capped window
//! reopens. Pure state machine (the pipeline persists [`PaceAlertState`] in `state.json`).
//!
//! Forecast alert (per window kind, once per window instance — same instance keying as
//! `alerts.rs`: the reset `at_ms` rounded to 5 min, aliases within `alerts::alias_between` of the
//! current reset time's uncertainty):
//! - fires when `burn.hits_limit_before_reset` is true, `pct >= MIN_PCT` (50), the state is not
//!   stale, not ResetAwaitingData, not limit_reached, the reset is known and in the future, and
//!   `t100_ms - now >= MIN_LEAD_MS` (10 min) — a forecast that close is not a warning anymore;
//! - never re-fires in the same instance, even if the forecast recovers and worsens again. An
//!   estimate can be replaced by an exact time further off than the alias window, so the forecast
//!   uses twice that window (consecutive windows' resets are 5 h / 7 days apart).
//!
//! Heads-up (per window kind, once per instance): when `limit_reached` (or pct >=
//! [`LIMIT_REACHED_PCT`]), the phase
//! is Active and the reset is known, still ahead and `reset - now <= lead` (five_hour: 10 min,
//! weekly kinds: 60 min). An estimate counts only if its `plus_minus` is within the lead ("reopens
//! in 10 min" from a ±1 day guess is no heads-up). Stale data does not stop it: a capped window
//! stays capped until then.
//!
//! `first_run` suppresses nothing here (a forecast is still useful right after start), but events
//! whose instance was already handled before a restart must not repeat (the state is persisted).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::alerts::{alias_between, alias_ms, instance_key, plus_minus};
use crate::engine::merge::LIMIT_REACHED_PCT;
use crate::engine::types::{Phase, WindowKind, WindowState, WindowView};
use crate::time::{MINUTE_MS, Ms};

pub const MIN_PCT: f32 = 50.0;
pub const MIN_LEAD_MS: Ms = 10 * MINUTE_MS;
pub const HEADS_UP_FIVE_HOUR_MS: Ms = 10 * MINUTE_MS;
pub const HEADS_UP_WEEKLY_MS: Ms = 60 * MINUTE_MS;

#[derive(Debug, Clone, PartialEq)]
pub enum PaceAlertEvent {
    /// At the current pace the window reaches 100% at `t100_ms`, before `reset_at_ms`.
    Forecast { kind: WindowKind, pct: f32, t100_ms: Ms, reset_at_ms: Ms },
    /// A capped window reopens at `reset_at_ms` (soon).
    HeadsUp { kind: WindowKind, reset_at_ms: Ms },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KindPaceState {
    /// Instance key of the window the forecast alert already fired for.
    pub forecast_fired_for: Option<Ms>,
    /// Instance key of the window the heads-up already fired for.
    pub heads_up_fired_for: Option<Ms>,
}

/// Persisted; keyed by `WindowKind::key()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaceAlertState {
    pub kinds: BTreeMap<String, KindPaceState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaceSettings {
    pub forecast: bool,
    pub heads_up: bool,
}

impl PaceAlertState {
    /// Returns the events to show now, in window order. Only the parts named by `settings` run
    /// (and advance their state).
    pub fn evaluate(&mut self, windows: &[WindowView], settings: PaceSettings, now_ms: Ms) -> Vec<PaceAlertEvent> {
        let mut events = Vec::new();
        for view in windows {
            let w = &view.state;
            let Some(key) = w.reset.at_ms().map(instance_key) else {
                continue;
            };
            let entry = self.kinds.entry(w.kind.key().to_owned()).or_default();
            let alias = alias_between(&w.kind, plus_minus(&w.reset), 0);
            if settings.forecast && !w.stale {
                let alias = alias.max(alias_ms(&w.kind).saturating_mul(2));
                if let Some(event) = forecast(view, now_ms) {
                    if !claim(&mut entry.forecast_fired_for, key, alias) {
                        events.push(event);
                    }
                } else if w.phase == Phase::Active {
                    // Keep following an alias (e.g. a drifting weekly estimate) of the fired instance.
                    claim_alias(&mut entry.forecast_fired_for, key, alias);
                }
            }
            if settings.heads_up {
                if let Some(event) = heads_up(w, now_ms) {
                    if !claim(&mut entry.heads_up_fired_for, key, alias) {
                        events.push(event);
                    }
                }
            }
        }
        events
    }
}

/// A forecast alert for `view`, if every condition in the module docs holds.
fn forecast(view: &WindowView, now_ms: Ms) -> Option<PaceAlertEvent> {
    let w = &view.state;
    let burn = view.burn.as_ref().filter(|b| b.hits_limit_before_reset)?;
    let reset_at_ms = w.reset.at_ms().filter(|&r| r > now_ms)?;
    let t100_ms = Some(burn.t100_ms).filter(|&t| t < reset_at_ms)?;
    let ok = w.pct >= MIN_PCT
        && w.phase == Phase::Active
        && !w.limit_reached
        && t100_ms.saturating_sub(now_ms) >= MIN_LEAD_MS;
    ok.then(|| PaceAlertEvent::Forecast {
        kind: w.kind.clone(),
        pct: w.pct.min(100.0),
        t100_ms,
        reset_at_ms,
    })
}

/// A heads-up for a capped window whose (future) reset is within the kind's lead.
fn heads_up(w: &WindowState, now_ms: Ms) -> Option<PaceAlertEvent> {
    let capped = w.limit_reached || w.pct >= LIMIT_REACHED_PCT;
    let lead = match w.kind {
        WindowKind::FiveHour => HEADS_UP_FIVE_HOUR_MS,
        _ => HEADS_UP_WEEKLY_MS,
    };
    if plus_minus(&w.reset) > lead {
        return None;
    }
    let reset_at_ms = w.reset.at_ms().filter(|&r| r > now_ms && r - now_ms <= lead)?;
    (capped && w.phase == Phase::Active).then(|| PaceAlertEvent::HeadsUp {
        kind: w.kind.clone(),
        reset_at_ms,
    })
}

/// Marks instance `key` as fired. Returns true if it (or an alias of it) already was; an alias
/// replaces the stored key, as in `alerts.rs`.
fn claim(fired_for: &mut Option<Ms>, key: Ms, alias: Ms) -> bool {
    let already = claim_alias(fired_for, key, alias);
    *fired_for = Some(key);
    already
}

/// Moves the stored key to `key` if it is an alias of it; returns whether it was.
fn claim_alias(fired_for: &mut Option<Ms>, key: Ms, alias: Ms) -> bool {
    let same = fired_for.is_some_and(|old| old.abs_diff(key) <= alias.unsigned_abs());
    if same {
        *fired_for = Some(key);
    }
    same
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Burn, Confidence, ResetInfo, Source};
    use crate::time::{DAY_MS, HOUR_MS};
    use pretty_assertions::assert_eq;

    const T0: Ms = 1_790_000_100_000; // a multiple of 5 min
    const R: Ms = T0 + 3 * HOUR_MS;
    const BOTH: PaceSettings = PaceSettings {
        forecast: true,
        heads_up: true,
    };

    fn view(kind: WindowKind, pct: f32, reset: ResetInfo, t100_ms: Option<Ms>) -> WindowView {
        let reset_at = reset.at_ms();
        WindowView {
            state: WindowState {
                kind,
                pct,
                reset,
                source: Source::Cli,
                observed_at_ms: T0,
                stale: false,
                limit_reached: pct >= LIMIT_REACHED_PCT,
                phase: Phase::Active,
            },
            burn: t100_ms.map(|t| Burn {
                slope_pct_per_h: 10.0,
                t100_ms: t,
                pct_at_reset: None,
                hits_limit_before_reset: reset_at.is_some_and(|r| t < r),
            }),
            spark: vec![],
            worked_since: false,
        }
    }

    /// A 5h window resetting at `R`, projected to hit 100% at `t100`.
    fn fh(pct: f32, t100: Ms) -> WindowView {
        view(WindowKind::FiveHour, pct, ResetInfo::Exact { at_ms: R }, Some(t100))
    }

    fn capped(kind: WindowKind, reset_at: Ms) -> WindowView {
        view(kind, 100.0, ResetInfo::Exact { at_ms: reset_at }, None)
    }

    fn forecast(pct: f32, t100_ms: Ms) -> PaceAlertEvent {
        PaceAlertEvent::Forecast {
            kind: WindowKind::FiveHour,
            pct,
            t100_ms,
            reset_at_ms: R,
        }
    }

    #[test]
    fn forecast_fires_once_per_instance() {
        let mut s = PaceAlertState::default();
        let t100 = T0 + HOUR_MS;
        assert_eq!(s.evaluate(&[fh(60.0, t100)], BOTH, T0), vec![forecast(60.0, t100)]);
        assert_eq!(s.evaluate(&[fh(65.0, t100)], BOTH, T0 + MINUTE_MS), vec![]);
        // The pace recovers, then worsens again: still the same window, no second alert.
        let calm = view(WindowKind::FiveHour, 66.0, ResetInfo::Exact { at_ms: R }, None);
        assert_eq!(s.evaluate(&[calm], BOTH, T0 + 2 * MINUTE_MS), vec![]);
        assert_eq!(s.evaluate(&[fh(70.0, t100)], BOTH, T0 + 3 * MINUTE_MS), vec![]);
        assert_eq!(s.kinds["five_hour"].forecast_fired_for, Some(R));
        assert_eq!(s.kinds["five_hour"].heads_up_fired_for, None);
    }

    #[test]
    fn next_window_rearms_the_forecast() {
        let mut s = PaceAlertState::default();
        assert_eq!(s.evaluate(&[fh(60.0, T0 + HOUR_MS)], BOTH, T0).len(), 1);
        let next = R + 5 * HOUR_MS;
        let later = view(WindowKind::FiveHour, 55.0, ResetInfo::Exact { at_ms: next }, Some(next - HOUR_MS));
        assert_eq!(
            s.evaluate(&[later], BOTH, R + HOUR_MS),
            vec![PaceAlertEvent::Forecast {
                kind: WindowKind::FiveHour,
                pct: 55.0,
                t100_ms: next - HOUR_MS,
                reset_at_ms: next,
            }]
        );
    }

    #[test]
    fn forecast_conditions() {
        let run = |v: WindowView, now: Ms| PaceAlertState::default().evaluate(&[v], BOTH, now);
        let t100 = T0 + HOUR_MS;
        assert_eq!(run(fh(49.9, t100), T0), vec![], "below 50%");
        assert_eq!(run(fh(50.0, t100), T0), vec![forecast(50.0, t100)]);
        assert_eq!(run(fh(80.0, T0 + 10 * MINUTE_MS), T0).len(), 1, "exactly 10 min ahead");
        assert_eq!(run(fh(80.0, T0 + 10 * MINUTE_MS - 1), T0), vec![], "too close to warn");
        assert_eq!(run(fh(80.0, R + MINUTE_MS), T0), vec![], "100% only after the reset");
        let mut stale = fh(80.0, t100);
        stale.state.stale = true;
        assert_eq!(run(stale, T0), vec![]);
        let mut awaiting = fh(80.0, t100);
        awaiting.state.phase = Phase::ResetAwaitingData;
        assert_eq!(run(awaiting, T0), vec![]);
        let mut reached = fh(99.0, t100);
        reached.state.limit_reached = true;
        assert_eq!(run(reached, T0), vec![]);
        let unknown = view(WindowKind::FiveHour, 80.0, ResetInfo::Unknown, Some(t100));
        assert_eq!(run(unknown, T0), vec![]);
        assert_eq!(run(fh(80.0, t100), R), vec![], "reset already passed");
        let mut no_burn = fh(80.0, t100);
        no_burn.burn = None;
        assert_eq!(run(no_burn, T0), vec![]);
        // A burn that says "before the reset" while its own t100 is not: trust neither.
        let mut inconsistent = fh(80.0, R);
        inconsistent.burn.as_mut().unwrap().hits_limit_before_reset = true;
        assert_eq!(run(inconsistent, T0), vec![]);
        assert_eq!(run(fh(f32::NAN, t100), T0), vec![]);
    }

    #[test]
    fn forecast_does_not_consume_state_while_quiet() {
        // Below 50% nothing is recorded, so crossing 50% later in the same window still alerts.
        let mut s = PaceAlertState::default();
        let t100 = T0 + 2 * HOUR_MS;
        assert_eq!(s.evaluate(&[fh(40.0, t100)], BOTH, T0), vec![]);
        assert_eq!(s.kinds["five_hour"], KindPaceState::default());
        assert_eq!(s.evaluate(&[fh(52.0, t100)], BOTH, T0 + MINUTE_MS), vec![forecast(52.0, t100)]);
    }

    #[test]
    fn weekly_estimate_drift_stays_one_instance() {
        let weekly = |at_ms: Ms| {
            view(
                WindowKind::SevenDay,
                70.0,
                ResetInfo::Estimated {
                    at_ms,
                    plus_minus_ms: DAY_MS,
                    confidence: Confidence::Low,
                },
                Some(at_ms - DAY_MS),
            )
        };
        let base = T0 + 3 * DAY_MS;
        let mut s = PaceAlertState::default();
        assert_eq!(s.evaluate(&[weekly(base)], BOTH, T0).len(), 1);
        // Re-estimated 40 h later, twice: 80 h from the first key, but each step is an alias.
        assert_eq!(s.evaluate(&[weekly(base + 40 * HOUR_MS)], BOTH, T0 + HOUR_MS), vec![]);
        assert_eq!(s.evaluate(&[weekly(base + 80 * HOUR_MS)], BOTH, T0 + 2 * HOUR_MS), vec![]);
        assert_eq!(s.kinds["seven_day"].forecast_fired_for, Some(base + 80 * HOUR_MS));
        // The drift is followed while no forecast shows, too.
        let mut calm = weekly(base + 120 * HOUR_MS);
        calm.burn = None;
        assert_eq!(s.evaluate(&[calm], BOTH, T0 + 3 * HOUR_MS), vec![]);
        assert_eq!(s.evaluate(&[weekly(base + 160 * HOUR_MS)], BOTH, T0 + 4 * HOUR_MS), vec![]);
    }

    #[test]
    fn five_hour_forecast_alias_is_an_hour() {
        let mut s = PaceAlertState::default();
        assert_eq!(s.evaluate(&[fh(60.0, T0 + HOUR_MS)], BOTH, T0).len(), 1);
        let near = |at_ms: Ms| view(WindowKind::FiveHour, 61.0, ResetInfo::Exact { at_ms }, Some(T0 + HOUR_MS));
        assert_eq!(s.evaluate(&[near(R + 60 * MINUTE_MS)], BOTH, T0), vec![]);
        assert_eq!(s.evaluate(&[near(R + 125 * MINUTE_MS)], BOTH, T0).len(), 1, "a new window");
    }

    #[test]
    fn heads_up_fires_once_within_the_lead() {
        let mut s = PaceAlertState::default();
        let w = capped(WindowKind::FiveHour, R);
        assert_eq!(s.evaluate(std::slice::from_ref(&w), BOTH, R - 10 * MINUTE_MS - 1), vec![]);
        assert_eq!(
            s.evaluate(std::slice::from_ref(&w), BOTH, R - 10 * MINUTE_MS),
            vec![PaceAlertEvent::HeadsUp {
                kind: WindowKind::FiveHour,
                reset_at_ms: R
            }]
        );
        assert_eq!(s.evaluate(std::slice::from_ref(&w), BOTH, R - MINUTE_MS), vec![]);
        assert_eq!(s.kinds["five_hour"].heads_up_fired_for, Some(R));
        // The forecast never fires for a capped window.
        assert_eq!(s.kinds["five_hour"].forecast_fired_for, None);
    }

    #[test]
    fn heads_up_conditions() {
        let run = |v: WindowView, now: Ms| PaceAlertState::default().evaluate(&[v], BOTH, now);
        let heads_up = |kind: WindowKind| vec![PaceAlertEvent::HeadsUp { kind, reset_at_ms: R }];
        // Weekly kinds get an hour.
        assert_eq!(run(capped(WindowKind::SevenDay, R), R - HOUR_MS), heads_up(WindowKind::SevenDay));
        assert_eq!(run(capped(WindowKind::SevenDay, R), R - HOUR_MS - 1), vec![]);
        let opus = WindowKind::Other("seven_day_opus".into());
        assert_eq!(run(capped(opus.clone(), R), R - 45 * MINUTE_MS), heads_up(opus));
        // 99.5% counts as capped; 99% does not.
        let almost = view(WindowKind::FiveHour, 99.5, ResetInfo::Exact { at_ms: R }, None);
        assert_eq!(run(almost, R - MINUTE_MS).len(), 1);
        let below = view(WindowKind::FiveHour, 99.0, ResetInfo::Exact { at_ms: R }, None);
        assert_eq!(run(below, R - MINUTE_MS), vec![]);
        // A reset that already passed is not "soon".
        assert_eq!(run(capped(WindowKind::FiveHour, R), R), vec![]);
        let unknown = view(WindowKind::FiveHour, 100.0, ResetInfo::Unknown, None);
        assert_eq!(run(unknown, R - MINUTE_MS), vec![]);
        let mut awaiting = capped(WindowKind::FiveHour, R);
        awaiting.state.phase = Phase::ResetAwaitingData;
        assert_eq!(run(awaiting, R - MINUTE_MS), vec![]);
        // Old data for a capped window with a known reset is still capped.
        let mut stale = capped(WindowKind::FiveHour, R);
        stale.state.stale = true;
        assert_eq!(run(stale, R - MINUTE_MS).len(), 1);
    }

    #[test]
    fn heads_up_needs_a_reset_time_precise_enough_for_its_lead() {
        let run = |v: WindowView, now: Ms| PaceAlertState::default().evaluate(&[v], BOTH, now);
        let est = |kind: WindowKind, plus_minus_ms: Ms, confidence: Confidence| {
            view(
                kind,
                100.0,
                ResetInfo::Estimated {
                    at_ms: R,
                    plus_minus_ms,
                    confidence,
                },
                None,
            )
        };
        // "Reopens in 8 min" from a ±2 h guess would be a promise the data cannot keep.
        assert_eq!(run(est(WindowKind::FiveHour, 2 * HOUR_MS, Confidence::Low), R - 8 * MINUTE_MS), vec![]);
        assert_eq!(run(est(WindowKind::FiveHour, 45 * MINUTE_MS, Confidence::Medium), R - 8 * MINUTE_MS), vec![]);
        assert_eq!(run(est(WindowKind::SevenDay, DAY_MS, Confidence::Low), R - 30 * MINUTE_MS), vec![]);
        // An estimate within the lead is good enough.
        let close = est(WindowKind::FiveHour, 10 * MINUTE_MS, Confidence::High);
        assert_eq!(run(close, R - 8 * MINUTE_MS).len(), 1);
    }

    #[test]
    fn an_exact_reset_replacing_the_estimate_is_the_same_window() {
        // Desktop-only first (a ±45 min estimate), then Claude Code reports the exact time 40 min
        // later than estimated: still the window the forecast already fired for.
        let mut s = PaceAlertState::default();
        let estimated = ResetInfo::Estimated {
            at_ms: R,
            plus_minus_ms: 45 * MINUTE_MS,
            confidence: Confidence::Medium,
        };
        let t100 = T0 + HOUR_MS;
        assert_eq!(s.evaluate(&[view(WindowKind::FiveHour, 60.0, estimated, Some(t100))], BOTH, T0).len(), 1);
        let exact = view(WindowKind::FiveHour, 61.0, ResetInfo::Exact { at_ms: R + 40 * MINUTE_MS }, Some(t100));
        assert_eq!(s.evaluate(&[exact], BOTH, T0 + MINUTE_MS), vec![]);
        // The next window (five hours on) is still new.
        let next = R + 40 * MINUTE_MS + 5 * HOUR_MS;
        let later = view(WindowKind::FiveHour, 60.0, ResetInfo::Exact { at_ms: next }, Some(next - HOUR_MS));
        assert_eq!(s.evaluate(&[later], BOTH, next - 3 * HOUR_MS).len(), 1);
    }

    #[test]
    fn settings_gate_each_part_and_its_state() {
        let t100 = T0 + HOUR_MS;
        let mut s = PaceAlertState::default();
        let heads_up_only = PaceSettings {
            forecast: false,
            heads_up: true,
        };
        assert_eq!(s.evaluate(&[fh(60.0, t100)], heads_up_only, T0), vec![]);
        assert_eq!(s.kinds["five_hour"].forecast_fired_for, None, "off: no state consumed");
        assert_eq!(s.evaluate(&[fh(60.0, t100)], BOTH, T0), vec![forecast(60.0, t100)]);

        let forecast_only = PaceSettings {
            forecast: true,
            heads_up: false,
        };
        let w = capped(WindowKind::FiveHour, R);
        assert_eq!(s.evaluate(std::slice::from_ref(&w), forecast_only, R - MINUTE_MS), vec![]);
        assert_eq!(s.kinds["five_hour"].heads_up_fired_for, None);
        assert_eq!(s.evaluate(&[w], BOTH, R - MINUTE_MS).len(), 1);
    }

    #[test]
    fn events_follow_window_order() {
        let mut s = PaceAlertState::default();
        let weekly = capped(WindowKind::SevenDay, T0 + 30 * MINUTE_MS);
        assert_eq!(
            s.evaluate(&[fh(60.0, T0 + HOUR_MS), weekly], BOTH, T0),
            vec![
                forecast(60.0, T0 + HOUR_MS),
                PaceAlertEvent::HeadsUp {
                    kind: WindowKind::SevenDay,
                    reset_at_ms: T0 + 30 * MINUTE_MS
                },
            ]
        );
    }

    #[test]
    fn restart_does_not_repeat() {
        let mut s = PaceAlertState::default();
        s.evaluate(&[fh(60.0, T0 + HOUR_MS)], BOTH, T0);
        s.evaluate(&[capped(WindowKind::SevenDay, T0 + 30 * MINUTE_MS)], BOTH, T0);
        let json = serde_json::to_string(&s).unwrap();
        let mut restored: PaceAlertState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s);
        let again = [fh(62.0, T0 + HOUR_MS), capped(WindowKind::SevenDay, T0 + 30 * MINUTE_MS)];
        assert_eq!(restored.evaluate(&again, BOTH, T0 + MINUTE_MS), vec![]);
        // Missing fields take their defaults.
        let partial: PaceAlertState =
            serde_json::from_str(r#"{"kinds":{"five_hour":{"heads_up_fired_for":5}}}"#).unwrap();
        assert_eq!(
            partial.kinds["five_hour"],
            KindPaceState {
                forecast_fired_for: None,
                heads_up_fired_for: Some(5)
            }
        );
        assert_eq!(serde_json::from_str::<PaceAlertState>("{}").unwrap(), PaceAlertState::default());
    }
}
