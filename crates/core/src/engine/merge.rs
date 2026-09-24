//! Merges CLI and Desktop observations of one window into a display state.
//!
//! Rules (all for a single `kind`):
//! 1. Split CLI observations into LIVE (`resets_at_ms > now_ms`) and EXPIRED.
//! 2. Group LIVE by `resets_at_ms`, treating values within [`RESET_GROUP_TOLERANCE_MS`] as equal.
//!    The group with the latest reset wins; within it `pct = max(pct)` and
//!    `observed_at = max(observed_at)` (several sessions report the same account-level numbers,
//!    an idle session may carry an older, lower value). Call the winner C.
//! 3. Let D be the Desktop observation (if any).
//!    a. C and D, D newer than C:
//!       - D.observed_at >= C.resets_at → D is from a later window: pct = D, reset = `estimate`,
//!         source Desktop.
//!       - D.pct < C.pct - [`DESKTOP_TOLERANCE_PCT`] → early reset detected: pct = D,
//!         reset = `estimate`, source Desktop.
//!       - otherwise → pct = D.pct, reset = Exact(C.resets_at), source Desktop, observed = D.
//!    b. C, and D missing or not newer → pct = C, reset = Exact(C.resets_at), source Cli.
//!    c. no C, D present → pct = D, reset = `estimate`, source Desktop. If an EXPIRED CLI reset
//!       lies in (D.observed_at, now] — or `estimate.at_ms()` lies in (D.observed_at, now] — the
//!       window has reset since D was measured: phase = ResetAwaitingData, pct = 0.
//!    d. no C, no D, but EXPIRED CLI observations → ResetAwaitingData, pct = 0, reset =
//!       `estimate`, source Cli, observed = newest expired observation.
//!    e. nothing → `None`.
//! 4. `stale = now - observed_at > stale_after_ms` (always false in ResetAwaitingData).
//!    `limit_reached = pct >= 99.5`. pct is clamped to 0..=100.

use crate::engine::types::{Observation, ResetInfo, WindowKind, WindowState};
use crate::time::Ms;

/// Desktop reports integers while the CLI reports one decimal; differences within this are noise.
pub const DESKTOP_TOLERANCE_PCT: f32 = 1.0;
/// CLI `resets_at` values within this are the same window.
pub const RESET_GROUP_TOLERANCE_MS: Ms = 120_000;

/// `cli` may contain observations of other kinds (ignore them). `desktop` must be of `kind` if present.
pub fn merge_window(
    kind: &WindowKind,
    cli: &[Observation],
    desktop: Option<&Observation>,
    estimate: ResetInfo,
    now_ms: Ms,
    stale_after_ms: Ms,
) -> Option<WindowState> {
    let _ = (kind, cli, desktop, estimate, now_ms, stale_after_ms);
    todo!("merge::merge_window")
}
