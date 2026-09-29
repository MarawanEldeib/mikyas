//! Resolves the context-window size and % for the active session.
//!
//! Size priority (first that applies wins; basis recorded in [`CtxBasis`]):
//! 1. `capture.context.context_window_size` from a statusline capture of the SAME session → Statusline
//! 2. The session runs a tagged (long-context) model id — `tail.identity_1m == Some(true)`
//!    (basis Identity), else a Desktop session `model` with a context tag (DesktopModel), else a
//!    same-session capture's `model.id` with a tag (Statusline). Its size: a user override for
//!    exactly that tagged id (Override), else the learned size of the tagged id, else the size the
//!    tag names (`[2m]` → 2M), else 1M ([`ONE_M_CTX`]).
//! 3. `overrides[base model id]` (the id without its tag; raw or canonical spelling) → Override
//! 4. `learned[base model id]`, unless the session already held more tokens than that → Learned
//! 5. More tokens seen than the default below → the smallest learned size larger than what was
//!    seen, preferring this model's tagged variants, then any model's; else 1M, or the next
//!    doubling of 1M above what was seen; Heuristic
//! 6. The default: the learned plain size of the newest version of the same model family
//!    (`model_names::model_family`, a newer `claude-opus-6` starts from what `claude-opus-5-5`
//!    reported), else [`DEFAULT_CTX`]; Default
//!
//! Learned sizes ([`learn_sizes`]) are the `context_window_size` statusline captures reported for
//! a model id, from any session, kept by the app across restarts (the newest [`MAX_LEARNED`]
//! models, see [`cap_learned`]). They are keyed by [`learned_key`]: the canonical id
//! (`model_names::canonical_model_id`, so Bedrock/Vertex/gateway spellings and dated ids share an
//! entry) plus the context tag (`claude-opus-5-5` and `claude-opus-5-5[1m]` are different
//! windows), so a new model or window size is picked up once Claude Code reports it; the fixed
//! 200k/1M values are only the fallback when nothing was learned.
//!
//! Percentage: if a same-session capture has `context.used_percentage` and its `changed_at_ms` is
//! not older than `tail.last_assistant_ms - 5 s` (or there is no tail) → use it, not an estimate.
//! Otherwise `tail.ctx_tokens / size * 100` (clamped 0..=100) with `is_estimate = true`.
//! No tail and no capture pct → `pct: None`.
//!
//! Details beyond the list above: a size of 0 (capture or override) never applies; the base
//! model id for rule 3 is taken from the tail, else the capture, else the Desktop session.
//! Override keys match by [`learned_key`] (trimmed, ASCII case ignored, canonical spelling), so a
//! key typed with a tag applies only to that tagged variant and a plain key only to rules 3+.
//! A capture with `exceeds_200k_tokens: true` counts as more than 200k seen (rules 4 and 5). When
//! the capture's % is used, `tokens` is still the tail's count (if any). The tail's `identity_1m`
//! and `max_ctx_tokens_seen` only speak for its current model, so after `/model` switches away
//! from a long-context model rules 2 and 5 no longer fire on the old model's behalf. Learned
//! sizes and overrides outside [`MIN_CTX_TOKENS`]..=[`MAX_CTX_TOKENS`] never apply.

use std::collections::BTreeMap;

use crate::capture::CaptureRecord;
use crate::engine::types::CtxBasis;
use crate::model_names::{canonical_model_id, ctx_tag_text, model_family, model_version, split_ctx_tag};
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
/// At most this many learned sizes (and learned names) are kept: the most recently reported.
pub const MAX_LEARNED: usize = 64;
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

/// The long-context variant a session runs: its tag text (`[1m]`, lower case; `None` when only
/// known to be tagged), the size the tag names, and the rule that found it.
struct Tagged {
    tag: Option<String>,
    hint: Option<u64>,
    basis: CtxBasis,
}

/// Rules 1–6. Always returns a non-zero size.
fn resolve_size(inputs: &ContextInputs<'_>) -> (u64, CtxBasis) {
    let capture_ctx = inputs.capture.and_then(|c| c.context.as_ref());

    if let Some(size) = capture_ctx.and_then(|c| c.context_window_size).filter(|&s| s > 0) {
        return (size, CtxBasis::Statusline);
    }
    let base = base_model_id(inputs);
    let learned = |tag: Option<&str>| base.and_then(|b| learned_size(inputs.learned, b, tag));

    if let Some(t) = tagged(inputs) {
        let exact = base.zip(t.tag.as_deref()).and_then(|(b, tag)| override_for(inputs.overrides, b, Some(tag)));
        if let Some(size) = exact {
            return (size, CtxBasis::Override);
        }
        let size = learned(t.tag.as_deref()).or(t.hint.filter(|&h| plausible_size(h))).unwrap_or(ONE_M_CTX);
        return (size, t.basis);
    }
    if let Some(size) = base.and_then(|id| override_for(inputs.overrides, id, None)) {
        return (size, CtxBasis::Override);
    }
    let mut seen = inputs.tail.map_or(0, |t| t.max_ctx_tokens_seen.max(t.ctx_tokens));
    // A capture saying the session is past 200K counts as having seen more than 200K.
    if capture_ctx.is_some_and(|c| c.exceeds_200k == Some(true)) {
        seen = seen.max(DEFAULT_CTX + 1);
    }
    if let Some(size) = learned(None).filter(|&s| seen <= s) {
        return (size, CtxBasis::Learned);
    }
    let default = base.and_then(|b| family_default(inputs.learned, b)).unwrap_or(DEFAULT_CTX);
    if seen > default {
        return (larger_than(inputs.learned, base, seen), CtxBasis::Heuristic);
    }
    (default, CtxBasis::Default)
}

/// Rule 2's sources, in order: the transcript identity, the Desktop session, the capture.
fn tagged(inputs: &ContextInputs<'_>) -> Option<Tagged> {
    if let Some(t) = inputs.tail.filter(|t| t.identity_1m == Some(true)) {
        let tag = t.identity_tag.clone();
        let hint = tag.as_deref().and_then(|tag| split_ctx_tag(tag).1).and_then(|t| t.hint);
        return Some(Tagged { tag, hint, basis: CtxBasis::Identity });
    }
    let from_id = |id: &str, basis: CtxBasis| {
        let id = id.trim();
        let (_, tag) = split_ctx_tag(id);
        tag.map(|t| Tagged { tag: ctx_tag_text(id), hint: t.hint, basis })
    };
    let desktop = inputs.desktop_session.and_then(|d| d.model.as_deref());
    let capture = inputs.capture.and_then(|c| c.model.as_ref()).and_then(|m| m.id.as_deref());
    desktop
        .and_then(|m| from_id(m, CtxBasis::DesktopModel))
        .or_else(|| capture.and_then(|m| from_id(m, CtxBasis::Statusline)))
}

/// The learned size of `base` (+ `tag`): by [`learned_key`], else by the raw spelling (entries
/// learned before keys were canonical).
fn learned_size(learned: &BTreeMap<String, u64>, base: &str, tag: Option<&str>) -> Option<u64> {
    let raw = format!("{}{}", base.trim(), tag.unwrap_or(""));
    learned_key(&raw)
        .and_then(|k| learned.get(&k))
        .or_else(|| learned.get(&raw))
        .copied()
        .filter(|&s| plausible_size(s))
}

/// Rule 5: the smallest learned size above `seen` (this model's tagged variants first, then any
/// model's), else 1M, else the first doubling of 1M above `seen` (capped at [`MAX_CTX_TOKENS`]).
fn larger_than(learned: &BTreeMap<String, u64>, base: Option<&str>, seen: u64) -> u64 {
    let above = |s: &u64| plausible_size(*s) && *s > seen;
    let own_prefix = base.and_then(learned_key);
    let own = learned
        .iter()
        .filter(|(k, _)| own_prefix.as_deref().is_some_and(|p| k.starts_with(p) && split_ctx_tag(k).0 == p))
        .map(|(_, s)| *s)
        .filter(above)
        .min();
    if let Some(size) = own.or_else(|| learned.values().copied().filter(above).min()) {
        return size;
    }
    let mut size = ONE_M_CTX;
    while size <= seen && size < MAX_CTX_TOKENS {
        size = size.saturating_mul(2).min(MAX_CTX_TOKENS);
    }
    size
}

/// Rule 6: the learned plain (untagged) size of the newest other version of `base`'s family.
fn family_default(learned: &BTreeMap<String, u64>, base: &str) -> Option<u64> {
    let family = model_family(base)?;
    let own = canonical_model_id(base);
    learned
        .iter()
        .filter(|(k, s)| split_ctx_tag(k).1.is_none() && plausible_size(**s))
        .filter(|(k, _)| !canonical_model_id(k).eq_ignore_ascii_case(own))
        .filter(|(k, _)| model_family(k).as_deref() == Some(family.as_str()))
        .max_by(|a, b| model_version(a.0).cmp(&model_version(b.0)).then_with(|| b.0.cmp(a.0)))
        .map(|(_, s)| *s)
}

/// True for a size inside [`MIN_CTX_TOKENS`]..=[`MAX_CTX_TOKENS`].
pub fn plausible_size(size: u64) -> bool {
    (MIN_CTX_TOKENS..=MAX_CTX_TOKENS).contains(&size)
}

/// The key a model id's learned size is stored under: the canonical id
/// (`model_names::canonical_model_id`) plus its context tag in lower case, if any
/// (`us.anthropic.claude-x-v1:0[1M]` → `claude-x[1m]`). `None` for an empty, overlong or
/// control-character id.
pub fn learned_key(id: &str) -> Option<String> {
    let id = id.trim();
    let base = canonical_model_id(id).trim();
    if base.is_empty() || base.len() > MAX_LEARNED_ID_LEN || base.chars().any(char::is_control) {
        return None;
    }
    let base_part = split_ctx_tag(id).0;
    if base_part.trim().is_empty() {
        return None;
    }
    Some(format!("{base}{}", ctx_tag_text(id).unwrap_or_default()))
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

/// Records when each captured model id was last reported, under both its [`learned_key`] (sizes)
/// and its canonical id (names). True if `seen` changed.
pub fn note_seen(seen: &mut BTreeMap<String, Ms>, captures: &[CaptureRecord]) -> bool {
    let mut changed = false;
    for cap in captures {
        let Some(id) = cap.model.as_ref().and_then(|m| m.id.as_deref()) else { continue };
        let Some(key) = learned_key(id) else { continue };
        let plain = split_ctx_tag(&key).0.to_owned();
        for k in [key, plain] {
            let entry = seen.entry(k).or_insert(Ms::MIN);
            if cap.changed_at_ms > *entry {
                *entry = cap.changed_at_ms;
                changed = true;
            }
        }
    }
    changed
}

/// Keeps the `max` most recently seen entries of `map` (by `seen`; entries never seen count as
/// oldest, ties keep the smaller key). True if anything was removed.
pub fn cap_learned<V>(map: &mut BTreeMap<String, V>, seen: &BTreeMap<String, Ms>, max: usize) -> bool {
    if map.len() <= max {
        return false;
    }
    let mut ranked: Vec<(Ms, String)> =
        map.keys().map(|k| (seen.get(k).copied().unwrap_or(Ms::MIN), k.clone())).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, key) in ranked.into_iter().skip(max) {
        map.remove(&key);
    }
    true
}

/// The session's model id without its context tag: tail first, then capture, then Desktop metadata.
fn base_model_id<'a>(inputs: &ContextInputs<'a>) -> Option<&'a str> {
    let tail = inputs.tail.and_then(|t| t.model_id.as_deref());
    let capture = inputs.capture.and_then(|c| c.model.as_ref()).and_then(|m| m.id.as_deref());
    let desktop = inputs.desktop_session.and_then(|d| d.model.as_deref());
    [tail, capture, desktop].into_iter().flatten().map(|id| split_ctx_tag(id.trim()).0).find(|id| !id.is_empty())
}

/// The user's override for `base` (+ `tag`): a key naming exactly that variant, compared by
/// [`learned_key`] (trimmed, canonical, case-insensitive), or typed exactly as the raw id.
fn override_for(overrides: &BTreeMap<String, u64>, base: &str, tag: Option<&str>) -> Option<u64> {
    let raw = format!("{}{}", base.trim(), tag.unwrap_or(""));
    let want = learned_key(&raw)?;
    overrides
        .iter()
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(&raw))
        .or_else(|| overrides.iter().find(|(k, _)| learned_key(k).is_some_and(|k| k.eq_ignore_ascii_case(&want))))
        .map(|(_, &v)| v)
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
            entrypoint_raw: None,
            model_id: Some("claude-opus-5-5".into()),
            ctx_tokens,
            max_ctx_tokens_seen: max_seen,
            identity_1m,
            identity_tag: (identity_1m == Some(true)).then(|| "[1m]".to_owned()),
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

        // A key written with a tag names only that tagged variant, not the plain model.
        let o = overrides(&[("claude-opus-5-5[1m]", 800_000)]);
        let r = run(Some(&t), None, None, &o);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Heuristic));
        // Keys are matched trimmed, case-insensitively and by canonical spelling.
        for key in [" claude-opus-5-5 ", "Claude-Opus-5-5", "us.anthropic.claude-opus-5-5-v1:0"] {
            let o = overrides(&[(key, 450_000)]);
            assert_eq!(run(Some(&t), None, None, &o).size, 450_000, "{key}");
        }

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
        assert_eq!(learned_key("us.anthropic.claude-x-20260101-v1:0[2M]").as_deref(), Some("claude-x[2m]"));
        assert_eq!(learned_key("claude-x@20260101").as_deref(), Some("claude-x"));
        assert_eq!(learned_key("claude-x").as_deref(), Some("claude-x"));
        assert_eq!(learned_key("[1m]"), None);
        assert_eq!(learned_key(""), None);
    }

    fn tail_of(model: &str, ctx_tokens: u64, identity_tag: Option<&str>) -> TranscriptTail {
        let mut t = tail(ctx_tokens, ctx_tokens, identity_tag.map(|_| true));
        t.model_id = Some(model.into());
        t.identity_tag = identity_tag.map(str::to_owned);
        t
    }

    #[test]
    fn any_context_tag_sizes_the_session() {
        let none = BTreeMap::new();
        // A tag that names a size is used directly; one that does not falls back to 1M.
        let r = run(Some(&tail_of("claude-x-1", 10_000, Some("[2m]"))), None, None, &none);
        assert_eq!((r.size, r.basis), (2_000_000, CtxBasis::Identity));
        let r = run(Some(&tail_of("claude-x-1", 10_000, Some("[long]"))), None, None, &none);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Identity));
        let r = run(Some(&tail_of("claude-x-1", 10_000, None)), None, Some(&desktop("claude-x-1[500k]")), &none);
        assert_eq!((r.size, r.basis), (500_000, CtxBasis::DesktopModel));
        // A size learned for the tagged id beats the tag's own number.
        let l = learned(&[("claude-x-1[2m]", 2_500_000)]);
        let r = run_learned(Some(&tail_of("claude-x-1", 10_000, Some("[2m]"))), None, None, &none, &l);
        assert_eq!(r.size, 2_500_000);
    }

    #[test]
    fn an_override_for_the_exact_tagged_id_wins_over_the_tag() {
        let t = tail_of("claude-opus-5-5", 10_000, Some("[1m]"));
        let l = learned(&[("claude-opus-5-5[1m]", 1_000_000)]);
        let o = overrides(&[("claude-opus-5-5[1m]", 750_000)]);
        let r = run_learned(Some(&t), None, None, &o, &l);
        assert_eq!((r.size, r.basis), (750_000, CtxBasis::Override));
        // Also for a Desktop or capture tag, and typed in another case.
        let o = overrides(&[("Claude-Opus-5-5[1M]", 700_000)]);
        let plain = tail_of("claude-opus-5-5", 10_000, None);
        let r = run(Some(&plain), None, Some(&desktop("claude-opus-5-5[1m]")), &o);
        assert_eq!((r.size, r.basis), (700_000, CtxBasis::Override));
        // A base-id override leaves tagged sessions to the tag (today's behaviour).
        let o = overrides(&[("claude-opus-5-5", 300_000)]);
        let r = run_learned(Some(&t), None, None, &o, &l);
        assert_eq!((r.size, r.basis), (ONE_M_CTX, CtxBasis::Identity));
    }

    #[test]
    fn provider_spellings_share_learned_sizes() {
        let l = learned(&[("claude-opus-5-5", 400_000)]);
        for model in ["us.anthropic.claude-opus-5-5-v1:0", "claude-opus-5-5@20260101", "claude-opus-5-5-20260101"] {
            let r = run_learned(Some(&tail_of(model, 10_000, None)), None, None, &BTreeMap::new(), &l);
            assert_eq!((r.size, r.basis), (400_000, CtxBasis::Learned), "{model}");
        }
        // Keys learned before canonical keys (a raw dated id) still apply to that exact id.
        let l = learned(&[("claude-haiku-5-20261001", 300_000)]);
        let r = run_learned(Some(&tail_of("claude-haiku-5-20261001", 10_000, None)), None, None, &BTreeMap::new(), &l);
        assert_eq!((r.size, r.basis), (300_000, CtxBasis::Learned));
    }

    #[test]
    fn heuristic_uses_the_smallest_learned_size_above_what_was_seen() {
        let none = BTreeMap::new();
        // Another model reported 400K: a new model past 200K is sized at 400K, not 1M.
        let l = learned(&[("claude-other-2", 400_000), ("claude-big-1[1m]", 1_000_000)]);
        let r = run_learned(Some(&tail_of("claude-new-1", 300_000, None)), None, None, &none, &l);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Heuristic));
        // This model's own tagged variant comes first.
        let l = learned(&[("claude-other-2", 400_000), ("claude-new-1[2m]", 2_000_000)]);
        let r = run_learned(Some(&tail_of("claude-new-1", 300_000, None)), None, None, &none, &l);
        assert_eq!(r.size, 2_000_000);
        // Past 1M with nothing learned: the next doubling, never clamped to 100%.
        let r = run(Some(&tail_of("claude-new-1", 1_200_000, None)), None, None, &none);
        assert_eq!((r.size, r.basis), (2_000_000, CtxBasis::Heuristic));
        assert!(r.pct.is_some_and(|p| p < 100.0));
        let r = run(Some(&tail_of("claude-new-1", 5_000_000, None)), None, None, &none);
        assert_eq!(r.size, 8_000_000);
    }

    #[test]
    fn a_new_model_starts_from_its_familys_newest_learned_size() {
        let none = BTreeMap::new();
        let l = learned(&[
            ("claude-opus-5-5", 400_000),
            ("claude-opus-4-5", 200_000),
            ("claude-opus-5-5[1m]", 1_000_000),
            ("claude-sonnet-5", 300_000),
        ]);
        let r = run_learned(Some(&tail_of("claude-opus-6", 10_000, None)), None, None, &none, &l);
        assert_eq!((r.size, r.basis), (400_000, CtxBasis::Default));
        // Past that default, the heuristic takes over.
        let r = run_learned(Some(&tail_of("claude-opus-6", 500_000, None)), None, None, &none, &l);
        assert_eq!((r.size, r.basis), (1_000_000, CtxBasis::Heuristic));
        // Another family, or no family: the built-in default.
        let r = run_learned(Some(&tail_of("claude-haiku-6", 10_000, None)), None, None, &none, &l);
        assert_eq!((r.size, r.basis), (DEFAULT_CTX, CtxBasis::Default));
    }

    #[test]
    fn learned_entries_are_capped_to_the_most_recent() {
        let mut seen = BTreeMap::new();
        let caps: Vec<CaptureRecord> =
            (0..5).map(|i| cap_with(Some(&format!("claude-m-{i}")), Some(100_000 + i), T + i as Ms)).collect();
        assert!(note_seen(&mut seen, &caps));
        assert!(!note_seen(&mut seen, &caps), "unchanged");
        let mut sizes = BTreeMap::new();
        learn_sizes(&mut sizes, &caps);
        sizes.insert("claude-never-seen".into(), 5_000);
        assert!(cap_learned(&mut sizes, &seen, 3));
        let keys: Vec<&str> = sizes.keys().map(String::as_str).collect();
        assert_eq!(keys, ["claude-m-2", "claude-m-3", "claude-m-4"], "the newest three stay");
        assert!(!cap_learned(&mut sizes, &seen, 3), "already within the cap");
        // Tagged ids count under their own key and the plain one (for names).
        let mut seen = BTreeMap::new();
        note_seen(&mut seen, &[cap_with(Some("claude-x[1m]"), None, T)]);
        assert_eq!(seen.keys().map(String::as_str).collect::<Vec<_>>(), ["claude-x", "claude-x[1m]"]);
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
