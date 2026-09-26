//! Pace alerts: warn BEFORE a limit is hit, and give a heads-up shortly before a capped window
//! reopens. Pure state machine (the pipeline persists [`PaceAlertState`] in `state.json`).
//!
//! Forecast alert (per window kind, once per window instance — same instance keying as
//! `alerts.rs`: the reset `at_ms` rounded to 5 min, aliases within the kind's alias window):
//! - fires when `burn.hits_limit_before_reset` is true, `pct >= MIN_PCT` (50), the state is not
//!   stale, not ResetAwaitingData, not limit_reached, the reset is known and in the future, and
//!   `t100_ms - now >= MIN_LEAD_MS` (10 min) — a forecast that close is not a warning anymore;
//! - never re-fires in the same instance, even if the forecast recovers and worsens again.
//!
//! Heads-up (per window kind, once per instance): when `limit_reached` (or pct >= 99.5) and the
//! reset is known and `reset - now <= lead` (five_hour: 10 min, weekly kinds: 60 min).
//!
//! `first_run` suppresses nothing here (a forecast is still useful right after start), but events
//! whose instance was already handled before a restart must not repeat (the state is persisted).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::types::{WindowKind, WindowView};
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
    /// Returns the events to show now, in window order. TODO(stream A1): implement per the module docs.
    pub fn evaluate(&mut self, windows: &[WindowView], settings: PaceSettings, now_ms: Ms) -> Vec<PaceAlertEvent> {
        let _ = (windows, settings, now_ms);
        Vec::new()
    }
}
