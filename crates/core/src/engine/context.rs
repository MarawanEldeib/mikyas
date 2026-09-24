//! Resolves the context-window size and % for the active session.
//!
//! Size priority (first that applies wins; basis recorded in [`CtxBasis`]):
//! 1. `capture.context.context_window_size` from a statusline capture of the SAME session → Statusline
//! 2. `tail.identity_1m == Some(true)` → 1M, Identity
//! 3. `desktop_session.model` ends with `[1m]` → 1M, DesktopModel
//! 4. `overrides[base model id]` (id without `[1m]`) → Override
//! 5. `tail.max_ctx_tokens_seen > 200_000` → 1M, Heuristic
//! 6. 200k, Default
//!
//! Percentage: if a same-session capture has `context.used_percentage` and its `changed_at_ms` is
//! not older than `tail.last_assistant_ms - 5 s` (or there is no tail) → use it, not an estimate.
//! Otherwise `tail.ctx_tokens / size * 100` (clamped 0..=100) with `is_estimate = true`.
//! No tail and no capture pct → `pct: None`.

use std::collections::BTreeMap;

use crate::capture::CaptureRecord;
use crate::engine::types::CtxBasis;
use crate::sources::desktop_sessions::DesktopSession;
use crate::sources::transcript::TranscriptTail;
use crate::time::Ms;

pub const DEFAULT_CTX: u64 = 200_000;
pub const ONE_M_CTX: u64 = 1_000_000;
/// Allowed lag between a statusline capture and the transcript's last assistant line.
pub const CAPTURE_FRESH_SLACK_MS: Ms = 5_000;

#[derive(Debug, Clone, Copy)]
pub struct ContextInputs<'a> {
    pub tail: Option<&'a TranscriptTail>,
    /// Must already be matched to the tail's session (same `session_id`).
    pub capture: Option<&'a CaptureRecord>,
    /// Must already be matched (`cli_session_id == tail.session_id`).
    pub desktop_session: Option<&'a DesktopSession>,
    pub overrides: &'a BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextResult {
    pub size: u64,
    pub basis: CtxBasis,
    pub pct: Option<f32>,
    pub tokens: Option<u64>,
    pub is_estimate: bool,
}

pub fn resolve(inputs: &ContextInputs<'_>) -> ContextResult {
    let _ = inputs;
    todo!("context::resolve")
}
