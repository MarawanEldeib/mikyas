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
//!
//! Details beyond the list above: a size of 0 (capture or override) never applies; the base
//! model id for rule 4 is taken from the tail, else the capture, else the Desktop session, and
//! override keys written with a `[1m]` suffix still match; a capture with
//! `exceeds_200k_tokens: true` also triggers rule 5. When the capture's % is used, `tokens` is
//! still the tail's count (if any). The tail's `identity_1m` and `max_ctx_tokens_seen` only speak
//! for its current model, so after `/model` switches away from a 1M model rules 2 and 5 no longer
//! fire on the old model's behalf.

use std::collections::BTreeMap;

use crate::capture::CaptureRecord;
use crate::engine::types::CtxBasis;
use crate::model_names::split_1m;
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

/// Resolves size, basis and % following the priority rules in the module docs.
pub fn resolve(inputs: &ContextInputs<'_>) -> ContextResult {
    let (size, basis) = resolve_size(inputs);
    let tokens = inputs.tail.map(|t| t.ctx_tokens);

    let capture_pct = inputs.capture.and_then(|c| {
        let pct = c.context.as_ref()?.used_percentage.filter(|p| p.is_finite())?;
        let fresh =
            inputs.tail.is_none_or(|t| c.changed_at_ms >= t.last_assistant_ms.saturating_sub(CAPTURE_FRESH_SLACK_MS));
        fresh.then_some(pct.clamp(0.0, 100.0))
    });

    let (pct, is_estimate) = match (capture_pct, inputs.tail) {
        (Some(pct), _) => (Some(pct), false),
        (None, Some(tail)) => {
            let pct = (tail.ctx_tokens as f64 / size as f64 * 100.0).clamp(0.0, 100.0) as f32;
            (Some(pct), true)
        }
        (None, None) => (None, false),
    };

    ContextResult { size, basis, pct, tokens, is_estimate }
}

/// Rules 1–6. Always returns a non-zero size.
fn resolve_size(inputs: &ContextInputs<'_>) -> (u64, CtxBasis) {
    let capture_ctx = inputs.capture.and_then(|c| c.context.as_ref());

    if let Some(size) = capture_ctx.and_then(|c| c.context_window_size).filter(|&s| s > 0) {
        return (size, CtxBasis::Statusline);
    }
    if inputs.tail.is_some_and(|t| t.identity_1m == Some(true)) {
        return (ONE_M_CTX, CtxBasis::Identity);
    }
    let desktop_model = inputs.desktop_session.and_then(|d| d.model.as_deref());
    if desktop_model.is_some_and(|m| split_1m(m).1) {
        return (ONE_M_CTX, CtxBasis::DesktopModel);
    }
    if let Some(size) = base_model_id(inputs).and_then(|id| override_for(inputs.overrides, id)) {
        return (size, CtxBasis::Override);
    }
    let seen_over_200k = inputs.tail.is_some_and(|t| t.max_ctx_tokens_seen.max(t.ctx_tokens) > DEFAULT_CTX);
    if seen_over_200k || capture_ctx.is_some_and(|c| c.exceeds_200k == Some(true)) {
        return (ONE_M_CTX, CtxBasis::Heuristic);
    }
    (DEFAULT_CTX, CtxBasis::Default)
}

/// The session's model id without `[1m]`: tail first, then capture, then Desktop metadata.
fn base_model_id<'a>(inputs: &ContextInputs<'a>) -> Option<&'a str> {
    let tail = inputs.tail.and_then(|t| t.model_id.as_deref());
    let capture = inputs.capture.and_then(|c| c.model.as_ref()).and_then(|m| m.id.as_deref());
    let desktop = inputs.desktop_session.and_then(|d| d.model.as_deref());
    [tail, capture, desktop].into_iter().flatten().map(|id| split_1m(id.trim()).0).find(|id| !id.is_empty())
}

/// Override for `base`: exact key first, then any key that equals `base` once `[1m]` is stripped.
fn override_for(overrides: &BTreeMap<String, u64>, base: &str) -> Option<u64> {
    overrides
        .get(base)
        .copied()
        .or_else(|| overrides.iter().find(|(k, _)| split_1m(k.trim()).0 == base).map(|(_, &v)| v))
        .filter(|&v| v > 0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::capture::{CtxInfo, ModelInfo};
    use crate::engine::types::Entrypoint;

    const T: Ms = 1_790_208_000_000;

    fn tail(ctx_tokens: u64, max_seen: u64, identity_1m: Option<bool>) -> TranscriptTail {
        TranscriptTail {
            path: PathBuf::from("s.jsonl"),
            session_id: "00000000-0000-4000-8000-000000000001".into(),
            entrypoint: Entrypoint::Cli,
            model_id: Some("claude-opus-5-5".into()),
            ctx_tokens,
            max_ctx_tokens_seen: max_seen,
            identity_1m,
            last_assistant_ms: T,
            project: None,
            turn: Default::default(),
        }
    }

    fn capture(pct: Option<f32>, size: Option<u64>, changed_at_ms: Ms) -> CaptureRecord {
        CaptureRecord {
            v: 1,
            session_id: "00000000-0000-4000-8000-000000000001".into(),
            written_at_ms: changed_at_ms,
            changed_at_ms,
            fingerprint: 0,
            transcript_path: None,
            model: Some(ModelInfo {
                id: Some("claude-opus-5-5[1m]".into()),
                display_name: Some("Opus 5.5 (1M context)".into()),
            }),
            context: Some(CtxInfo { used_percentage: pct, context_window_size: size, exceeds_200k: None }),
            rate_limits: BTreeMap::new(),
            api_ms: None,
            cc_version: None,
        }
    }

    fn desktop(model: &str) -> DesktopSession {
        DesktopSession {
            cli_session_id: Some("00000000-0000-4000-8000-000000000001".into()),
            model: Some(model.into()),
            last_focused_ms: None,
            last_activity_ms: None,
        }
    }

    fn run(
        tail: Option<&TranscriptTail>,
        capture: Option<&CaptureRecord>,
        desktop: Option<&DesktopSession>,
        overrides: &BTreeMap<String, u64>,
    ) -> ContextResult {
        resolve(&ContextInputs { tail, capture, desktop_session: desktop, overrides })
    }

    fn overrides(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn default_estimate_from_tail() {
        let t = tail(50_000, 60_000, None);
        let r = run(Some(&t), None, None, &BTreeMap::new());
        assert_eq!(
            r,
            ContextResult {
                size: DEFAULT_CTX,
                basis: CtxBasis::Default,
                pct: Some(25.0),
                tokens: Some(50_000),
                is_estimate: true,
            }
        );
    }

    #[test]
    fn nothing_known() {
        let r = run(None, None, None, &BTreeMap::new());
        assert_eq!(
            r,
            ContextResult { size: DEFAULT_CTX, basis: CtxBasis::Default, pct: None, tokens: None, is_estimate: false }
        );
    }

    #[test]
    fn statusline_size_has_top_priority() {
        let t = tail(423_933, 423_933, Some(true));
        let c = capture(None, Some(1_000_000), T);
        let d = desktop("claude-opus-5-5[1m]");
        let o = overrides(&[("claude-opus-5-5", 500_000)]);
        let r = run(Some(&t), Some(&c), Some(&d), &o);
        assert_eq!((r.size, r.basis), (1_000_000, CtxBasis::Statusline));
        // No capture pct → estimate from the tail against the statusline size.
        assert_eq!(r.pct, Some(42.3933));
        assert!(r.is_estimate);
    }

    #[test]
    fn zero_statusline_size_falls_through() {
        let t = tail(1_000, 1_000, None);
        let c = capture(None, Some(0), T);
        let r = run(Some(&t), Some(&c), None, &BTreeMap::new());
        assert_eq!((r.size, r.basis), (DEFAULT_CTX, CtxBasis::Default));
    }

    #[test]
    fn identity_beats_desktop_override_and_heuristic() {
        let t = tail(10_000, 300_000, Some(true));
        let d = desktop("claude-opus-5-5");
        let o = overrides(&[("claude-opus-5-5", 500_000)]);
        let r = run(Some(&t), None, Some(&d), &o);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Identity));
        assert_eq!(r.pct, Some(1.0));
        // identity Some(false) does not force 200k: lower rules still apply.
        let t = tail(10_000, 10_000, Some(false));
        let r = run(Some(&t), None, None, &BTreeMap::new());
        assert_eq!(r.basis, CtxBasis::Default);
    }

    #[test]
    fn desktop_model_beats_override() {
        let t = tail(10_000, 10_000, None);
        let d = desktop("claude-opus-5-5[1m]");
        let o = overrides(&[("claude-opus-5-5", 500_000)]);
        let r = run(Some(&t), None, Some(&d), &o);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::DesktopModel));
    }

    #[test]
    fn override_keyed_by_base_id() {
        let t = tail(100_000, 300_000, None);
        let o = overrides(&[("claude-opus-5-5", 400_000)]);
        let r = run(Some(&t), None, Some(&desktop("claude-opus-5-5")), &o);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Override), "override beats the heuristic");
        assert_eq!(r.pct, Some(25.0));

        // A key written with [1m] still matches the base id.
        let o = overrides(&[("claude-opus-5-5[1m]", 800_000)]);
        let r = run(Some(&t), None, None, &o);
        assert_eq!((r.size, r.basis), (800_000, CtxBasis::Override));

        // Other models' overrides and zero values do not apply.
        let o = overrides(&[("claude-sonnet-5", 400_000), ("claude-opus-5-5", 0)]);
        let r = run(Some(&t), None, None, &o);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
    }

    #[test]
    fn override_uses_capture_or_desktop_model_without_tail_model() {
        let mut t = tail(100_000, 100_000, None);
        t.model_id = None;
        let o = overrides(&[("claude-opus-5-5", 400_000)]);
        // Capture model id is "claude-opus-5-5[1m]" → base "claude-opus-5-5".
        let c = capture(None, None, T);
        let r = run(Some(&t), Some(&c), None, &o);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Override));
        let r = run(Some(&t), None, Some(&desktop("claude-opus-5-5")), &o);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Override));
        let r = run(Some(&t), None, None, &o);
        assert_eq!(r.basis, CtxBasis::Default);
    }

    #[test]
    fn heuristic_needs_strictly_more_than_200k() {
        let t = tail(150_000, 200_000, None);
        let r = run(Some(&t), None, None, &BTreeMap::new());
        assert_eq!((r.size, r.basis), (DEFAULT_CTX, CtxBasis::Default));
        assert_eq!(r.pct, Some(75.0));

        let t = tail(150_000, 200_001, None);
        let r = run(Some(&t), None, None, &BTreeMap::new());
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
        assert_eq!(r.pct, Some(15.0));

        // A capture flagging exceeds_200k also proves a 1M window.
        let t = tail(150_000, 150_000, None);
        let mut c = capture(None, None, T);
        if let Some(ctx) = c.context.as_mut() {
            ctx.exceeds_200k = Some(true);
        }
        c.model = None;
        let r = run(Some(&t), Some(&c), None, &BTreeMap::new());
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
    }

    #[test]
    fn fresh_capture_pct_is_used() {
        let t = tail(423_933, 423_933, None);
        let c = capture(Some(43.0), Some(1_000_000), T - CAPTURE_FRESH_SLACK_MS);
        let r = run(Some(&t), Some(&c), None, &BTreeMap::new());
        assert_eq!(r.pct, Some(43.0));
        assert!(!r.is_estimate);
        assert_eq!(r.tokens, Some(423_933));
    }

    #[test]
    fn stale_capture_pct_is_ignored() {
        let t = tail(423_933, 423_933, None);
        let c = capture(Some(10.0), Some(1_000_000), T - CAPTURE_FRESH_SLACK_MS - 1);
        let r = run(Some(&t), Some(&c), None, &BTreeMap::new());
        assert_eq!(r.basis, CtxBasis::Statusline, "a stale capture still provides the size");
        assert_eq!(r.pct, Some(42.3933));
        assert!(r.is_estimate);
    }

    #[test]
    fn capture_without_tail() {
        let c = capture(Some(12.5), Some(1_000_000), T);
        let r = run(None, Some(&c), None, &BTreeMap::new());
        assert_eq!(
            r,
            ContextResult {
                size: 1_000_000,
                basis: CtxBasis::Statusline,
                pct: Some(12.5),
                tokens: None,
                is_estimate: false,
            }
        );
        // No tail and no capture pct → unknown.
        let c = capture(None, Some(1_000_000), T);
        let r = run(None, Some(&c), None, &BTreeMap::new());
        assert_eq!((r.pct, r.is_estimate), (None, false));
    }

    #[test]
    fn percentages_are_clamped() {
        let c = capture(Some(140.0), Some(200_000), T);
        assert_eq!(run(None, Some(&c), None, &BTreeMap::new()).pct, Some(100.0));
        let c = capture(Some(-3.0), Some(200_000), T);
        assert_eq!(run(None, Some(&c), None, &BTreeMap::new()).pct, Some(0.0));
        let c = capture(Some(f32::NAN), Some(200_000), T);
        assert_eq!(run(None, Some(&c), None, &BTreeMap::new()).pct, None);

        // Estimate above the size (e.g. override smaller than actual usage) clamps to 100.
        let t = tail(300_000, 300_000, None);
        let o = overrides(&[("claude-opus-5-5", 100_000)]);
        let r = run(Some(&t), None, None, &o);
        assert_eq!((r.pct, r.is_estimate), (Some(100.0), true));
    }
}
