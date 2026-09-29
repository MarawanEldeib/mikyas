//! Resolves the context-window size and % for the active session.
//!
//! Size priority (first that applies wins; basis recorded in [`CtxBasis`]):
//! 1. `capture.context.context_window_size` from a statusline capture of the SAME session → Statusline
//! 2. `tail.identity_1m == Some(true)` → the learned size of `<base>[1m]`, else 1M; Identity
//! 3. `desktop_session.model` ends with `[1m]` → the learned size of `<base>[1m]`, else 1M; DesktopModel
//!    (the same for a same-session capture's `model.id` ending with `[1m]`, basis Statusline)
//! 4. `overrides[base model id]` (id without `[1m]`) → Override
//! 5. `learned[base model id]`, unless the session already held more tokens than that → Learned
//! 6. `tail.max_ctx_tokens_seen > 200_000` → the learned size of `<base>[1m]` if it is larger than
//!    what was seen, else 1M; Heuristic
//! 7. 200k, Default
//!
//! Learned sizes ([`learn_sizes`]) are the `context_window_size` statusline captures reported for
//! a model id, from any session, kept by the app across restarts. They are keyed by the model id
//! as Claude Code reports it (`claude-opus-5-5` and `claude-opus-5-5[1m]` are different windows),
//! so a new model or window size is picked up once Claude Code reports it; the fixed 200k/1M
//! values are only the fallback when nothing was learned.
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
//! for its current model, so after `/model` switches away from a 1M model rules 2 and 6 no longer
//! fire on the old model's behalf. Learned sizes and overrides outside
//! [`MIN_CTX_TOKENS`]..=[`MAX_CTX_TOKENS`] never apply.

use std::collections::BTreeMap;

use crate::capture::CaptureRecord;
use crate::engine::types::CtxBasis;
use crate::model_names::split_1m;
use crate::sources::desktop_sessions::DesktopSession;
use crate::sources::transcript::TranscriptTail;
use crate::time::Ms;

pub const DEFAULT_CTX: u64 = 200_000;
pub const ONE_M_CTX: u64 = 1_000_000;
/// Smallest context-window size accepted from a capture or the user's override.
pub const MIN_CTX_TOKENS: u64 = 1_000;
/// Largest context-window size accepted from a capture or the user's override.
pub const MAX_CTX_TOKENS: u64 = 100_000_000;
/// Longest model id a learned size is kept for.
const MAX_LEARNED_ID_LEN: usize = 128;
/// Suffix of a learned key for a model's 1M-context variant (always lower case).
const ONE_M_KEY_SUFFIX: &str = "[1m]";
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
    /// Window sizes learned from statusline captures, by [`learned_key`] (see [`learn_sizes`]).
    pub learned: &'a BTreeMap<String, u64>,
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

/// Rules 1–7. Always returns a non-zero size.
fn resolve_size(inputs: &ContextInputs<'_>) -> (u64, CtxBasis) {
    let capture_ctx = inputs.capture.and_then(|c| c.context.as_ref());

    if let Some(size) = capture_ctx.and_then(|c| c.context_window_size).filter(|&s| s > 0) {
        return (size, CtxBasis::Statusline);
    }
    let base = base_model_id(inputs);
    let learned = |one_m: bool| {
        let key = learned_key_parts(base?, one_m)?;
        inputs.learned.get(&key).copied().filter(|&s| plausible_size(s))
    };
    if inputs.tail.is_some_and(|t| t.identity_1m == Some(true)) {
        return (learned(true).unwrap_or(ONE_M_CTX), CtxBasis::Identity);
    }
    let desktop_model = inputs.desktop_session.and_then(|d| d.model.as_deref());
    if desktop_model.is_some_and(|m| split_1m(m).1) {
        return (learned(true).unwrap_or(ONE_M_CTX), CtxBasis::DesktopModel);
    }
    let capture_model = inputs.capture.and_then(|c| c.model.as_ref()).and_then(|m| m.id.as_deref());
    if capture_model.is_some_and(|m| split_1m(m.trim()).1) {
        return (learned(true).unwrap_or(ONE_M_CTX), CtxBasis::Statusline);
    }
    if let Some(size) = base.and_then(|id| override_for(inputs.overrides, id)) {
        return (size, CtxBasis::Override);
    }
    let mut seen = inputs.tail.map_or(0, |t| t.max_ctx_tokens_seen.max(t.ctx_tokens));
    // A capture saying the session is past 200K counts as having seen more than 200K.
    if capture_ctx.is_some_and(|c| c.exceeds_200k == Some(true)) {
        seen = seen.max(DEFAULT_CTX + 1);
    }
    if let Some(size) = learned(false).filter(|&s| seen <= s) {
        return (size, CtxBasis::Learned);
    }
    if seen > DEFAULT_CTX {
        return (learned(true).filter(|&s| s > seen).unwrap_or(ONE_M_CTX), CtxBasis::Heuristic);
    }
    (DEFAULT_CTX, CtxBasis::Default)
}

/// True for a size inside [`MIN_CTX_TOKENS`]..=[`MAX_CTX_TOKENS`].
pub fn plausible_size(size: u64) -> bool {
    (MIN_CTX_TOKENS..=MAX_CTX_TOKENS).contains(&size)
}

/// The key a model id's learned size is stored under: the trimmed id without `[1m]`, plus a
/// lower-case `[1m]` when it had one. `None` for an empty, overlong or control-character id.
pub fn learned_key(id: &str) -> Option<String> {
    let (base, one_m) = split_1m(id.trim());
    learned_key_parts(base, one_m)
}

fn learned_key_parts(base: &str, one_m: bool) -> Option<String> {
    let base = base.trim();
    if base.is_empty() || base.len() > MAX_LEARNED_ID_LEN || base.chars().any(char::is_control) {
        return None;
    }
    Some(if one_m { format!("{base}{ONE_M_KEY_SUFFIX}") } else { base.to_owned() })
}

/// Folds each capture's reported `context_window_size` into `map` under [`learned_key`] of its
/// model id (the newest capture wins). Implausible sizes are skipped. True if `map` changed.
pub fn learn_sizes(map: &mut BTreeMap<String, u64>, captures: &[CaptureRecord]) -> bool {
    let mut ordered: Vec<&CaptureRecord> = captures.iter().collect();
    ordered.sort_by_key(|c| c.changed_at_ms);
    let mut newest = BTreeMap::new();
    for cap in ordered {
        let Some(id) = cap.model.as_ref().and_then(|m| m.id.as_deref()) else { continue };
        let size = cap.context.as_ref().and_then(|c| c.context_window_size).filter(|&s| plausible_size(s));
        if let (Some(size), Some(key)) = (size, learned_key(id)) {
            newest.insert(key, size);
        }
    }
    let mut changed = false;
    for (key, size) in newest {
        changed |= map.insert(key, size) != Some(size);
    }
    changed
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
        .filter(|&v| plausible_size(v))
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
            model: Some(ModelInfo {
                id: Some("claude-opus-5-5[1m]".into()),
                display_name: Some("Opus 5.5 (1M context)".into()),
            }),
            context: Some(CtxInfo { used_percentage: pct, context_window_size: size, exceeds_200k: None }),
            rate_limits: BTreeMap::new(),
            api_ms: None,
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
        run_learned(tail, capture, desktop, overrides, &BTreeMap::new())
    }

    fn run_learned(
        tail: Option<&TranscriptTail>,
        capture: Option<&CaptureRecord>,
        desktop: Option<&DesktopSession>,
        overrides: &BTreeMap<String, u64>,
        learned: &BTreeMap<String, u64>,
    ) -> ContextResult {
        resolve(&ContextInputs { tail, capture, desktop_session: desktop, overrides, learned })
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
        let mut c = capture(None, Some(0), T);
        // The capture's `[1m]` model id still says 1M.
        let r = run(Some(&t), Some(&c), None, &BTreeMap::new());
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Statusline));
        c.model = None;
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
        // A capture naming the plain model gives the base id.
        let mut c = capture(None, None, T);
        c.model = Some(ModelInfo { id: Some("claude-opus-5-5".into()), display_name: None });
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

    fn learned(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        overrides(pairs)
    }

    #[test]
    fn learned_size_beats_heuristic_and_default() {
        // A model whose plain window is 400K: no guessing 200K or 1M.
        let t = tail(100_000, 100_000, None);
        let l = learned(&[("claude-opus-5-5", 400_000)]);
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis, r.pct), (400_000, CtxBasis::Learned, Some(25.0)));
        // Past 200K but within the learned window: still the learned size, not "must be 1M".
        let t = tail(300_000, 300_000, None);
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Learned));
        // Other models learn nothing from it.
        let mut t = tail(100_000, 100_000, None);
        t.model_id = Some("claude-sonnet-5".into());
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!(r.basis, CtxBasis::Default);
    }

    #[test]
    fn learned_size_that_the_session_outgrew_is_skipped() {
        // Learned 200K for the plain id, but this session already held 450K: it is a bigger
        // window, and the learned 1M-variant size is used for it.
        let t = tail(450_000, 450_000, None);
        let l = learned(&[("claude-opus-5-5", 200_000), ("claude-opus-5-5[1m]", 2_000_000)]);
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (2_000_000, CtxBasis::Heuristic));
        // Without a learned 1M-variant size the heuristic falls back to 1M.
        let l = learned(&[("claude-opus-5-5", 200_000)]);
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
        // A learned 1M-variant size smaller than what was seen is not believed.
        let l = learned(&[("claude-opus-5-5[1m]", 400_000)]);
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
    }

    #[test]
    fn capture_saying_over_200k_outgrows_a_learned_200k() {
        // No tail (capture-only session) and no reported size, but the capture says the session
        // is past 200K: a learned plain 200K window cannot be right.
        let mut c = cap_with(Some("claude-opus-5-5"), None, T);
        c.context.as_mut().unwrap().exceeds_200k = Some(true);
        let l = learned(&[("claude-opus-5-5", 200_000)]);
        let r = run_learned(None, Some(&c), None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
        // A learned window larger than 200K still fits.
        let l = learned(&[("claude-opus-5-5", 400_000)]);
        let r = run_learned(None, Some(&c), None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Learned));
    }

    #[test]
    fn learned_one_m_variant_sizes_identity_and_desktop_rules() {
        let l = learned(&[("claude-opus-5-5[1m]", 1_500_000), ("claude-opus-5-5", 300_000)]);
        let t = tail(15_000, 15_000, Some(true));
        let r = run_learned(Some(&t), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis, r.pct), (1_500_000, CtxBasis::Identity, Some(1.0)));
        let t = tail(15_000, 15_000, None);
        let r = run_learned(Some(&t), None, Some(&desktop("claude-opus-5-5[1m]")), &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (1_500_000, CtxBasis::DesktopModel));
    }

    #[test]
    fn capture_model_with_1m_suffix_is_never_sized_by_the_plain_learned_size() {
        // The capture names the 1M variant but reports no size: the plain model's learned 200K
        // must not apply (it would fire context alerts far too early).
        let t = tail(150_000, 150_000, None);
        let c = capture(None, None, T);
        let l = learned(&[("claude-opus-5-5", 200_000)]);
        let o = overrides(&[("claude-opus-5-5", 300_000)]);
        let r = run_learned(Some(&t), Some(&c), None, &o, &l);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Statusline));
        let l = learned(&[("claude-opus-5-5", 200_000), ("claude-opus-5-5[1m]", 1_200_000)]);
        let r = run_learned(Some(&t), Some(&c), None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (1_200_000, CtxBasis::Statusline));
    }

    #[test]
    fn override_and_statusline_beat_learned() {
        let t = tail(10_000, 10_000, None);
        let l = learned(&[("claude-opus-5-5", 400_000)]);
        let o = overrides(&[("claude-opus-5-5", 500_000)]);
        let r = run_learned(Some(&t), None, None, &o, &l);
        assert_eq!((r.size, r.basis), (500_000, CtxBasis::Override));
        let c = capture(None, Some(600_000), T);
        let r = run_learned(Some(&t), Some(&c), None, &o, &l);
        assert_eq!((r.size, r.basis), (600_000, CtxBasis::Statusline));
    }

    #[test]
    fn implausible_learned_and_override_sizes_never_apply() {
        let t = tail(10_000, 10_000, None);
        for bad in [0, MIN_CTX_TOKENS - 1, MAX_CTX_TOKENS + 1, u64::MAX] {
            let m = learned(&[("claude-opus-5-5", bad)]);
            assert_eq!(run_learned(Some(&t), None, None, &m, &m).basis, CtxBasis::Default, "{bad}");
        }
        let t = tail(500, 500, None);
        for ok in [MIN_CTX_TOKENS, MAX_CTX_TOKENS] {
            let m = learned(&[("claude-opus-5-5", ok)]);
            assert_eq!(run_learned(Some(&t), None, None, &BTreeMap::new(), &m).size, ok);
            assert_eq!(run_learned(Some(&t), None, None, &m, &BTreeMap::new()).size, ok);
        }
    }

    fn cap_with(id: Option<&str>, size: Option<u64>, at: Ms) -> CaptureRecord {
        let mut c = capture(None, size, at);
        c.model = id.map(|id| ModelInfo { id: Some(id.into()), display_name: None });
        c
    }

    #[test]
    fn learn_sizes_keys_by_model_id_and_keeps_the_newest() {
        let mut map = BTreeMap::new();
        let caps = [
            cap_with(Some("claude-opus-5-5[1M]"), Some(1_000_000), T + 1),
            cap_with(Some(" claude-opus-5-5 "), Some(250_000), T + 2),
            cap_with(Some("claude-opus-5-5"), Some(200_000), T),
            cap_with(Some("claude-sonnet-5"), Some(0), T),
            cap_with(Some("claude-sonnet-5"), Some(MAX_CTX_TOKENS + 1), T),
            cap_with(Some("claude-sonnet-5"), None, T),
            cap_with(None, Some(300_000), T),
            cap_with(Some("  "), Some(300_000), T),
            cap_with(Some("bad\u{7}id"), Some(300_000), T),
            cap_with(Some(&"x".repeat(MAX_LEARNED_ID_LEN + 1)), Some(300_000), T),
        ];
        assert!(learn_sizes(&mut map, &caps));
        assert_eq!(map, learned(&[("claude-opus-5-5", 250_000), ("claude-opus-5-5[1m]", 1_000_000)]));
        assert!(!learn_sizes(&mut map, &caps), "unchanged");
        // A newer report replaces an older value (e.g. a model got a bigger window).
        assert!(learn_sizes(&mut map, &[cap_with(Some("claude-opus-5-5"), Some(500_000), T + 3)]));
        assert_eq!(map.get("claude-opus-5-5"), Some(&500_000));
    }

    #[test]
    fn learned_key_normalises() {
        assert_eq!(learned_key(" claude-x[1M] ").as_deref(), Some("claude-x[1m]"));
        assert_eq!(learned_key("claude-x").as_deref(), Some("claude-x"));
        assert_eq!(learned_key("[1m]"), None);
        assert_eq!(learned_key(""), None);
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            /// Whatever was learned or overridden, the size is non-zero and the % is in range.
            #[test]
            fn size_is_positive_and_pct_in_range(
                ctx in 0_u64..3_000_000,
                extra in 0_u64..3_000_000,
                identity in proptest::option::of(any::<bool>()),
                learned_plain in proptest::option::of(any::<u64>()),
                learned_1m in proptest::option::of(any::<u64>()),
                over in proptest::option::of(any::<u64>()),
            ) {
                let t = tail(ctx, ctx.saturating_add(extra), identity);
                let mut l = BTreeMap::new();
                if let Some(v) = learned_plain { l.insert("claude-opus-5-5".to_owned(), v); }
                if let Some(v) = learned_1m { l.insert("claude-opus-5-5[1m]".to_owned(), v); }
                let o = over.map(|v| overrides(&[("claude-opus-5-5", v)])).unwrap_or_default();
                let r = run_learned(Some(&t), None, None, &o, &l);
                prop_assert!(r.size > 0);
                prop_assert!(r.basis == CtxBasis::Default || plausible_size(r.size));
                let pct = r.pct.unwrap();
                prop_assert!((0.0..=100.0).contains(&pct));
                // A learned plain size is only used when the session fits in it.
                if r.basis == CtxBasis::Learned {
                    prop_assert!(ctx.saturating_add(extra) <= r.size);
                }
            }
        }
    }
}
