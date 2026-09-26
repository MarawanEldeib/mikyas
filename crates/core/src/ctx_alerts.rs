//! Context-window alerts: one toast when a session's context % crosses a threshold, so the user
//! can `/compact` or start a new session in time. Pure state machine over `Snapshot::sessions`;
//! the app persists [`CtxAlertState`] in `state.json` so a restart never re-fires.
//!
//! Per session (keyed by the opaque [`SessionView::key`]):
//! - Sessions with `ctx_pct: None` (or an empty key) are ignored, and so is a % estimated over
//!   the 200K default size (`ctx_basis: Default` with `ctx_is_estimate`): a 1M session not yet
//!   recognised as one would look five times fuller than it is.
//! - Re-arm: once `ctx_pct` falls at least [`REARM_DROP_PCT`] points below the LOWEST fired
//!   threshold (e.g. after `/compact`), the session's fired set is cleared.
//! - Thresholds: for the highest threshold `t` with `pct >= t` that has not fired yet, emit ONE
//!   event (the highest crossed) and mark every threshold `<= pct` as fired (like `alerts.rs`).
//! - Only sessions active within [`ALERT_ACTIVE_MS`] alert. A session that crossed a threshold
//!   while idle (or while the app was closed) alerts on its next turn, not at startup.
//! - Sessions not seen in the list for [`FORGET_AFTER_MS`] are forgotten.
//!
//! Implementation notes:
//! - Only sessions with fired thresholds are stored; re-arming removes the entry.
//! - `last_seen_ms` is refreshed at most every [`SEEN_RESOLUTION_MS`], so the persisted state only
//!   changes when an alert fires, a session re-arms or is forgotten, or about once an hour.
//! - pct is sanitised first (NaN → ignored, clamped to 0..=100).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::engine::types::{CtxBasis, SessionView};
use crate::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms};

/// A drop of at least this many points below the lowest fired threshold re-arms a session.
pub const REARM_DROP_PCT: f32 = 10.0;
/// Sessions absent from the list for longer than this are forgotten.
pub const FORGET_AFTER_MS: Ms = DAY_MS;
/// `last_seen_ms` granularity (keeps `state.json` from being rewritten on every tick).
pub const SEEN_RESOLUTION_MS: Ms = HOUR_MS;
/// Only sessions whose last turn is at most this old alert.
pub const ALERT_ACTIVE_MS: Ms = 10 * MINUTE_MS;

/// Something the app should turn into an OS notification.
#[derive(Debug, Clone, PartialEq)]
pub struct CtxAlertEvent {
    /// [`SessionView::key`] of the session.
    pub key: String,
    pub threshold: u8,
    pub pct: f32,
    /// Display name, else the model id; `None` if the session names no model.
    pub model: Option<String>,
    /// Only present when the snapshot shows project names.
    pub project: Option<String>,
}

/// Alert bookkeeping for one session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionCtxState {
    pub fired: BTreeSet<u8>,
    pub last_seen_ms: Ms,
}

/// Persisted as JSON; keyed by [`SessionView::key`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CtxAlertState {
    pub sessions: BTreeMap<String, SessionCtxState>,
}

impl CtxAlertState {
    /// Advances the state machine with the current sessions and returns the events to show, in
    /// session order. `thresholds` may be in any order; 0 is ignored.
    pub fn evaluate(&mut self, sessions: &[SessionView], thresholds: &[u8], now_ms: Ms) -> Vec<CtxAlertEvent> {
        let mut events = Vec::new();
        for session in sessions.iter().filter(|s| !s.key.is_empty()) {
            if let Some(entry) = self
                .sessions
                .get_mut(&session.key)
                .filter(|e| now_ms.abs_diff(e.last_seen_ms) >= SEEN_RESOLUTION_MS.unsigned_abs())
            {
                entry.last_seen_ms = now_ms;
            }
            let guessed = session.ctx_basis == CtxBasis::Default && session.ctx_is_estimate;
            let Some(pct) = session.ctx_pct.filter(|p| !p.is_nan() && !guessed).map(|p| p.clamp(0.0, 100.0)) else {
                continue;
            };
            let rearm = self
                .sessions
                .get(&session.key)
                .and_then(|e| e.fired.first())
                .is_some_and(|&lowest| pct <= f32::from(lowest) - REARM_DROP_PCT);
            if rearm {
                self.sessions.remove(&session.key);
            }
            if now_ms.saturating_sub(session.last_active_ms) > ALERT_ACTIVE_MS {
                continue;
            }

            let crossed = || thresholds.iter().copied().filter(|&t| t > 0 && pct >= f32::from(t));
            let fired = self.sessions.get(&session.key).map(|e| &e.fired);
            let Some(threshold) = crossed().filter(|t| fired.is_none_or(|f| !f.contains(t))).max() else {
                continue;
            };
            events.push(CtxAlertEvent {
                key: session.key.clone(),
                threshold,
                pct,
                model: session.display_name.clone().or_else(|| session.model_id.clone()),
                project: session.project.clone(),
            });
            let entry = self
                .sessions
                .entry(session.key.clone())
                .or_insert_with(|| SessionCtxState { fired: BTreeSet::new(), last_seen_ms: now_ms });
            entry.fired.extend(crossed());
        }
        self.sessions.retain(|_, e| now_ms.abs_diff(e.last_seen_ms) <= FORGET_AFTER_MS.unsigned_abs());
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::Entrypoint;
    use pretty_assertions::assert_eq;

    const NOW: Ms = 1_790_000_000_000;
    const T: &[u8] = &[80, 90];

    fn session(key: &str, pct: Option<f32>) -> SessionView {
        SessionView {
            key: key.into(),
            model_id: Some("claude-opus-5-5".into()),
            display_name: Some("Opus 5.5".into()),
            ctx_pct: pct,
            ctx_tokens: None,
            ctx_size: 1_000_000,
            ctx_basis: CtxBasis::Statusline,
            ctx_is_estimate: false,
            entrypoint: Entrypoint::Cli,
            last_active_ms: NOW,
            project: None,
            concurrent: 1,
        }
    }

    fn fired(state: &CtxAlertState, key: &str) -> Vec<u8> {
        state.sessions.get(key).map(|e| e.fired.iter().copied().collect()).unwrap_or_default()
    }

    /// Evaluates one session "a" at `pct`, active now.
    fn step(state: &mut CtxAlertState, pct: f32) -> Vec<u8> {
        state.evaluate(&[session("a", Some(pct))], T, NOW).iter().map(|e| e.threshold).collect()
    }

    #[test]
    fn crossing_each_threshold_fires_once() {
        let mut s = CtxAlertState::default();
        assert_eq!(step(&mut s, 50.0), Vec::<u8>::new());
        assert!(s.sessions.is_empty(), "nothing stored until something fires");
        assert_eq!(step(&mut s, 80.0), vec![80]);
        assert_eq!(step(&mut s, 85.0), Vec::<u8>::new());
        assert_eq!(step(&mut s, 91.5), vec![90]);
        assert_eq!(step(&mut s, 99.0), Vec::<u8>::new());
        assert_eq!(fired(&s, "a"), vec![80, 90]);
    }

    #[test]
    fn jump_fires_only_the_highest() {
        let mut s = CtxAlertState::default();
        let events = s.evaluate(&[session("a", Some(95.0))], &[90, 80], NOW);
        assert_eq!(
            events,
            vec![CtxAlertEvent {
                key: "a".into(),
                threshold: 90,
                pct: 95.0,
                model: Some("Opus 5.5".into()),
                project: None,
            }]
        );
        assert_eq!(fired(&s, "a"), vec![80, 90]);
    }

    #[test]
    fn rearms_ten_points_below_the_lowest_fired_threshold() {
        let mut s = CtxAlertState::default();
        step(&mut s, 92.0);
        assert_eq!(step(&mut s, 70.1), Vec::<u8>::new(), "9.9 points below 80: still armed off");
        assert_eq!(fired(&s, "a"), vec![80, 90]);
        assert_eq!(step(&mut s, 70.0), Vec::<u8>::new(), "exactly 10 below re-arms");
        assert!(s.sessions.is_empty());
        assert_eq!(step(&mut s, 81.0), vec![80], "fires again after /compact");
    }

    #[test]
    fn rearm_uses_the_lowest_fired_threshold() {
        let mut s = CtxAlertState::default();
        // Only 90 is configured: it re-arms below 80.
        s.evaluate(&[session("a", Some(90.0))], &[90], NOW);
        s.evaluate(&[session("a", Some(80.5))], &[90], NOW);
        assert_eq!(fired(&s, "a"), vec![90]);
        s.evaluate(&[session("a", Some(80.0))], &[90], NOW);
        assert!(s.sessions.is_empty());
    }

    #[test]
    fn sessions_are_independent_and_unknown_pct_is_ignored() {
        let mut s = CtxAlertState::default();
        let mut b = session("b", Some(85.0));
        b.display_name = None;
        b.project = Some("demo-app".into());
        let events = s.evaluate(&[session("a", None), b, session("", Some(99.0))], T, NOW);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, "b");
        assert_eq!(events[0].model.as_deref(), Some("claude-opus-5-5"), "falls back to the id");
        assert_eq!(events[0].project.as_deref(), Some("demo-app"));
        assert_eq!(s.sessions.keys().collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(s.evaluate(&[session("a", Some(81.0))], T, NOW).len(), 1);
    }

    #[test]
    fn estimates_over_the_default_size_never_fire() {
        // A 1M session not yet recognised as one looks five times fuller than it is.
        let mut s = CtxAlertState::default();
        let mut guess = session("a", Some(92.0));
        guess.ctx_basis = CtxBasis::Default;
        guess.ctx_is_estimate = true;
        assert_eq!(s.evaluate(std::slice::from_ref(&guess), T, NOW), vec![]);
        assert!(s.sessions.is_empty());
        // The statusline's own % over the default size is no guess.
        let mut reported = guess.clone();
        reported.ctx_is_estimate = false;
        assert_eq!(s.evaluate(std::slice::from_ref(&reported), T, NOW).len(), 1);
        // A guess neither re-arms nor keeps the session from being remembered.
        guess.ctx_pct = Some(5.0);
        for h in 1..=30 {
            assert_eq!(s.evaluate(std::slice::from_ref(&guess), T, NOW + h * HOUR_MS), vec![]);
        }
        assert_eq!(fired(&s, "a"), vec![80, 90]);
        // Estimates over a known size fire as before.
        for basis in [CtxBasis::Heuristic, CtxBasis::Override, CtxBasis::Identity] {
            let mut known = session("b", Some(85.0));
            known.ctx_basis = basis;
            known.ctx_is_estimate = true;
            let mut fresh = CtxAlertState::default();
            assert_eq!(fresh.evaluate(&[known], T, NOW).len(), 1, "{basis:?}");
        }
    }

    #[test]
    fn idle_sessions_do_not_alert_until_their_next_turn() {
        let mut s = CtxAlertState::default();
        let mut idle = session("a", Some(88.0));
        idle.last_active_ms = NOW - ALERT_ACTIVE_MS - 1;
        assert_eq!(s.evaluate(std::slice::from_ref(&idle), T, NOW), vec![]);
        assert!(s.sessions.is_empty());
        idle.last_active_ms = NOW - ALERT_ACTIVE_MS;
        assert_eq!(s.evaluate(&[idle], T, NOW).len(), 1, "boundary is inclusive");
    }

    #[test]
    fn idle_sessions_still_rearm() {
        let mut s = CtxAlertState::default();
        step(&mut s, 85.0);
        let mut compacted = session("a", Some(20.0));
        compacted.last_active_ms = NOW - HOUR_MS;
        s.evaluate(&[compacted], T, NOW);
        assert!(s.sessions.is_empty());
    }

    #[test]
    fn unseen_sessions_are_forgotten_after_a_day() {
        let mut s = CtxAlertState::default();
        step(&mut s, 85.0);
        // Still listed: last_seen is refreshed (hourly), never forgotten.
        let mut later = session("a", Some(85.0));
        later.last_active_ms = NOW - 3 * DAY_MS;
        for h in 1..=30 {
            assert_eq!(s.evaluate(std::slice::from_ref(&later), T, NOW + h * HOUR_MS), vec![]);
        }
        assert_eq!(s.sessions["a"].last_seen_ms, NOW + 30 * HOUR_MS);
        // Gone from the list: kept for a day, then dropped.
        s.evaluate(&[], T, NOW + 54 * HOUR_MS);
        assert!(s.sessions.contains_key("a"));
        s.evaluate(&[], T, NOW + 54 * HOUR_MS + 1);
        assert!(s.sessions.is_empty());
    }

    #[test]
    fn last_seen_changes_at_most_hourly() {
        let mut s = CtxAlertState::default();
        step(&mut s, 85.0);
        let before = s.clone();
        s.evaluate(&[session("a", Some(86.0))], T, NOW + HOUR_MS - 1);
        assert_eq!(s, before, "no churn within the hour");
        s.evaluate(&[session("a", Some(86.0))], T, NOW + HOUR_MS);
        assert_eq!(s.sessions["a"].last_seen_ms, NOW + HOUR_MS);
    }

    #[test]
    fn restart_does_not_refire() {
        let mut s = CtxAlertState::default();
        step(&mut s, 85.0);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, format!(r#"{{"sessions":{{"a":{{"fired":[80],"last_seen_ms":{NOW}}}}}}}"#));
        let mut restored: CtxAlertState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s);
        assert_eq!(step(&mut restored, 86.0), Vec::<u8>::new());
    }

    #[test]
    fn persisted_state_tolerates_missing_fields() {
        let s: CtxAlertState = serde_json::from_str(r#"{"sessions":{"a":{"fired":[80]}}}"#).unwrap();
        assert_eq!(fired(&s, "a"), vec![80]);
        assert_eq!(s.sessions["a"].last_seen_ms, 0);
        assert_eq!(serde_json::from_str::<CtxAlertState>("{}").unwrap(), CtxAlertState::default());
    }

    #[test]
    fn odd_values_are_sanitised() {
        let mut s = CtxAlertState::default();
        assert_eq!(step(&mut s, f32::NAN), Vec::<u8>::new());
        assert_eq!(step(&mut s, f32::INFINITY), vec![90]);
        assert_eq!(s.evaluate(&[session("a", Some(100.0))], &[0], NOW), vec![], "0 is not a threshold");
        assert_eq!(s.evaluate(&[session("z", Some(100.0))], &[], NOW), vec![]);
        // A far-future last_seen (clock change) is refreshed rather than kept forever.
        let mut future = CtxAlertState::default();
        future
            .sessions
            .insert("a".into(), SessionCtxState { fired: BTreeSet::from([80]), last_seen_ms: NOW + 400 * DAY_MS });
        future.evaluate(&[session("a", Some(85.0))], T, NOW);
        assert_eq!(future.sessions["a"].last_seen_ms, NOW);
    }
}
