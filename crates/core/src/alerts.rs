//! Limit alerts: fire once per window instance when usage crosses a threshold, and optionally
//! when a window resets. Pure state machine; the app persists [`AlertState`] to `alerts.json`
//! and turns [`AlertEvent`]s into OS notifications.
//!
//! Window instance identity (per kind):
//! - The instance key is the reset `at_ms` rounded to 5 minutes (exact or estimated).
//! - A changed key within ±30 min of the stored one, with no pct drop, is the SAME instance (an
//!   alias — e.g. switching between an estimated and an exact reset); update the stored key.
//! - A NEW instance starts when pct drops by ≥ 1 point versus the stored `last_pct`, the phase is
//!   ResetAwaitingData, or the key moves by more than 30 min. On a new instance `fired` is cleared.
//!   If the previous instance's `last_pct > 0`, `settings.notify_reset` is on and `first_run` is
//!   false, emit `Reset { kind }`.
//! - Unknown reset + no drop → same instance.
//!
//! Thresholds: after instance handling, for the highest threshold `t` in `settings.thresholds`
//! with `pct >= t` that is not yet in `fired`, emit ONE `Threshold` event (the highest crossed)
//! and mark every threshold `<= pct` as fired (jumping 50 → 97 alerts once, for 95).
//! Stale windows never alert (neither threshold nor reset).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::engine::types::{WindowKind, WindowState};
use crate::time::{MINUTE_MS, Ms};

pub const INSTANCE_ROUND_MS: Ms = 5 * MINUTE_MS;
pub const INSTANCE_ALIAS_MS: Ms = 30 * MINUTE_MS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertSettings {
    /// Ascending percentages, default `[80, 95]`.
    pub thresholds: Vec<u8>,
    pub notify_reset: bool,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            thresholds: vec![80, 95],
            notify_reset: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlertEvent {
    Threshold {
        kind: WindowKind,
        threshold: u8,
        pct: f32,
        reset_at_ms: Option<Ms>,
    },
    Reset {
        kind: WindowKind,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KindAlertState {
    pub instance_key: Option<Ms>,
    pub fired: BTreeSet<u8>,
    pub last_pct: f32,
}

/// Persisted as JSON; keyed by `WindowKind::key()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AlertState {
    pub kinds: BTreeMap<String, KindAlertState>,
}

impl AlertState {
    /// `first_run` = this is the first evaluation since the app started (suppresses Reset toasts
    /// for resets that happened while the app was closed).
    pub fn evaluate(&mut self, windows: &[WindowState], settings: &AlertSettings, first_run: bool) -> Vec<AlertEvent> {
        let _ = (windows, settings, first_run);
        todo!("AlertState::evaluate")
    }
}
