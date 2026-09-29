//! Models, window keys and context sizes that do not exist today: everything must be derived
//! from the data and degrade gracefully (shown by the raw id or key).

use std::collections::BTreeMap;
use std::path::PathBuf;

use mikyas_core::capture::{CaptureRecord, CtxInfo, ModelInfo};
use mikyas_core::engine::context::{self, ContextInputs, DEFAULT_CTX, ONE_M_CTX};
use mikyas_core::engine::types::{CtxBasis, Entrypoint, WindowKind, main_kinds};
use mikyas_core::model_names::{canonical_model_id, display_name, model_family, split_ctx_tag};
use mikyas_core::sources::transcript::TranscriptTail;
use mikyas_core::time::{DAY_MS, HOUR_MS};

const T: i64 = 1_790_208_000_000;

fn tail(model: &str, tokens: u64, tag: Option<&str>) -> TranscriptTail {
    TranscriptTail {
        path: PathBuf::from("s.jsonl"),
        session_id: "00000000-0000-4000-8000-000000000001".into(),
        entrypoint: Entrypoint::Cli,
        entrypoint_raw: None,
        model_id: Some(model.into()),
        ctx_tokens: tokens,
        max_ctx_tokens_seen: tokens,
        identity_1m: tag.map(|_| true),
        identity_tag: tag.map(str::to_owned),
        last_assistant_ms: T,
        project: None,
        turn: Default::default(),
    }
}

fn capture(model: &str, size: Option<u64>) -> CaptureRecord {
    CaptureRecord {
        v: 1,
        session_id: "00000000-0000-4000-8000-000000000001".into(),
        written_at_ms: T,
        changed_at_ms: T,
        fingerprint: 0,
        model: Some(ModelInfo { id: Some(model.into()), display_name: None }),
        context: size.map(|s| CtxInfo { used_percentage: None, context_window_size: Some(s), exceeds_200k: None }),
        rate_limits: BTreeMap::new(),
        api_ms: None,
    }
}

fn size_of(
    t: Option<&TranscriptTail>,
    cap: Option<&CaptureRecord>,
    learned: &BTreeMap<String, u64>,
) -> (u64, CtxBasis) {
    let overrides = BTreeMap::new();
    let r = context::resolve(&ContextInputs {
        tail: t,
        capture: cap,
        desktop_session: None,
        overrides: &overrides,
        learned,
    });
    (r.size, r.basis)
}

#[test]
fn unseen_model_ids_are_named_and_canonicalised() {
    let none = BTreeMap::new();
    for (id, canonical, name) in [
        ("claude-nova-7-2", "claude-nova-7-2", "Nova 7.2"),
        ("claude-opus-6[2m]", "claude-opus-6", "Opus 6"),
        ("us.anthropic.claude-x-9-v2:0", "claude-x-9", "X 9"),
        ("vertex/claude-y@20271001", "claude-y", "Y"),
        ("eu.anthropic.claude-zephyr-8-5-20280101-v3:1[5m]", "claude-zephyr-8-5", "Zephyr 8.5"),
    ] {
        assert_eq!(canonical_model_id(id), canonical, "{id}");
        assert_eq!(display_name(id, &none), name, "{id}");
    }
    // A shape nothing understands is shown as reported.
    assert_eq!(display_name("mystery!model", &none), "mystery!model");
    assert_eq!(model_family("vertex/claude-y@20271001").as_deref(), Some("y"));
    assert_eq!(split_ctx_tag("claude-opus-6[2m]").1.and_then(|t| t.hint), Some(2_000_000));
}

#[test]
fn unseen_context_sizes_are_learned_not_assumed() {
    // Reported sizes win, whatever they are.
    let none = BTreeMap::new();
    for size in [400_000, 2_000_000, 5_000_000] {
        assert_eq!(size_of(None, Some(&capture("claude-nova-7-2", Some(size))), &none), (size, CtxBasis::Statusline));
    }
    // Learned once from a capture (any provider spelling), then used for the plain id.
    let mut learned = BTreeMap::new();
    context::learn_sizes(&mut learned, &[capture("us.anthropic.claude-x-9-v2:0", Some(400_000))]);
    context::learn_sizes(&mut learned, &[capture("vertex/claude-y@20271001[5m]", Some(5_000_000))]);
    assert_eq!(size_of(Some(&tail("claude-x-9", 10_000, None)), None, &learned), (400_000, CtxBasis::Learned));
    // The tagged variant of claude-y uses its learned 5M, not the tag's number or 1M.
    let t = tail("claude-y", 10_000, Some("[5m]"));
    assert_eq!(size_of(Some(&t), None, &learned).0, 5_000_000);
    // A new tag nothing was learned for: the size the tag names.
    let t = tail("claude-opus-6", 10_000, Some("[2m]"));
    assert_eq!(size_of(Some(&t), None, &BTreeMap::new()), (2_000_000, CtxBasis::Identity));
    // A new version of a family starts from its sibling's learned size.
    let mut learned = BTreeMap::new();
    context::learn_sizes(&mut learned, &[capture("claude-nova-7-2", Some(400_000))]);
    assert_eq!(size_of(Some(&tail("claude-nova-8", 10_000, None)), None, &learned), (400_000, CtxBasis::Default));
    // Nothing known at all: the fallback default; past it, never a 100 %+ guess.
    let none = BTreeMap::new();
    assert_eq!(size_of(Some(&tail("claude-brand-new-1", 10_000, None)), None, &none), (DEFAULT_CTX, CtxBasis::Default));
    let (size, basis) = size_of(Some(&tail("claude-brand-new-1", 3_000_000, None)), None, &none);
    assert!(size > 3_000_000, "{size}");
    assert_eq!(basis, CtxBasis::Heuristic);
    assert_eq!(size % ONE_M_CTX, 0, "a doubling of the 1M fallback, not a fixed list");
}

#[test]
fn unseen_window_keys_get_lengths_and_names() {
    let k = WindowKind::from_key;
    assert_eq!(k("three_day_haiku").duration_ms(), Some(3 * DAY_MS));
    assert_eq!(k("three_day_haiku").label(), "3-day Haiku");
    assert_eq!(k("monthly").duration_ms(), Some(30 * DAY_MS));
    assert_eq!(k("twelve_hour").short_label(), "12h");
    assert_eq!(k("twelve_hour").duration_ms(), Some(12 * HOUR_MS));
    // No length: shown by the key, never dropped.
    assert_eq!(k("credit_pool").duration_ms(), None);
    assert_eq!(k("credit_pool").label(), "credit pool");
    // Main windows follow the data, and a window set with no known lengths still shows.
    let kinds = [k("three_day_haiku"), k("twelve_hour"), k("monthly")];
    assert_eq!(main_kinds(&kinds), [k("twelve_hour"), k("monthly")]);
    assert_eq!(main_kinds(&[k("credit_pool")]), [k("credit_pool")]);
}
