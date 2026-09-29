//! Model id → display name, canonical id, context tag and family. Nothing here lists model
//! families, sizes or providers: every rule reads the shape of the id, so models Anthropic ships
//! later (and provider spellings of them) work without a code change.
//!
//! - [`split_ctx_tag`]: a trailing bracket tag is Claude Code's long-context marker (`[1m]` today;
//!   `[2m]`, `[500k]` or any other `[…]` later). `split_ctx_tag("claude-opus-5-5[1m]") ==
//!   ("claude-opus-5-5", Some(CtxTag { hint: Some(1_000_000) }))`. [`split_1m`] is the same test as
//!   a bool.
//! - [`canonical_model_id`]: the id without provider prefixes and suffixes, so the Bedrock, Vertex
//!   and gateway spellings of one model share learned names and sizes:
//!   `us.anthropic.claude-opus-5-5-20260101-v1:0[1m]`, `claude-opus-5-5@20260101` and
//!   `org/claude-opus-5-5` are all `claude-opus-5-5`. A plain id is returned unchanged.
//! - [`display_name`]: `learned` (the statusline's `model.display_name`, authoritative) by
//!   canonical id, then base id, then the id as given — with size markers such as "(1M context)"
//!   removed (the UI shows the size separately). Else [`heuristic_name`] of the canonical id:
//!   an optional `claude` prefix, then words and versions (`5`, `5-5` or `4.5`); the first word is
//!   the family, the numbers the version, further words a qualifier
//!   (`claude-opus-5-5` → "Opus 5.5", `claude-3-5-sonnet-20241022` → "Sonnet 3.5",
//!   `claude-opus-5-5-fast` → "Opus 5.5 Fast"). Ids without any word, or with tokens that are
//!   neither (`gpt-4o`), are shown unchanged. `<synthetic>` → "".
//! - [`model_family`]: the canonical id's words (`claude-opus-5-5` → `opus`), for "a newer
//!   version of the same family" guesses.

use std::collections::BTreeMap;

/// Placeholder model Claude Code writes for locally generated (non-API) messages.
const SYNTHETIC: &str = "<synthetic>";
/// Longest bracket tag accepted as a context marker, brackets included (`[1m]` is 4).
const MAX_TAG_LEN: usize = 16;

/// A trailing `[…]` context tag of a model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtxTag {
    /// The size the tag names (`[1m]` → 1 000 000, `[500k]` → 500 000); `None` for a tag that is
    /// not a size (`[long]`).
    pub hint: Option<u64>,
}

/// Splits a trailing bracket tag (`[1m]`, `[2M]`, `[500k]`, `[long]`) off a model id. The tag's
/// body is 1..=14 characters of `[A-Za-z0-9._-]`; anything else is not a tag.
pub fn split_ctx_tag(id: &str) -> (&str, Option<CtxTag>) {
    let Some(body_end) = id.len().checked_sub(1).filter(|_| id.ends_with(']')) else {
        return (id, None);
    };
    let Some(open) = id[..body_end].rfind('[') else { return (id, None) };
    let body = &id[open + 1..body_end];
    let valid = !body.is_empty()
        && body.len() + 2 <= MAX_TAG_LEN
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !valid {
        return (id, None);
    }
    (&id[..open], Some(CtxTag { hint: parse_size_word(body) }))
}

/// The id's context tag in lower case (`[1M]` → `[1m]`), if it has one: the suffix learned keys
/// keep so each window size of a model is its own entry.
pub fn ctx_tag_text(id: &str) -> Option<String> {
    let (base, tag) = split_ctx_tag(id);
    tag.map(|_| id[base.len()..].to_ascii_lowercase())
}

/// Splits a trailing context tag off a model id; `true` when it had one (see [`split_ctx_tag`]).
pub fn split_1m(id: &str) -> (&str, bool) {
    let (base, tag) = split_ctx_tag(id);
    (base, tag.is_some())
}

/// `1m`, `2M`, `1.5m`, `500k`, `200000` → tokens; anything else → `None`.
pub fn parse_size_word(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.as_bytes().last()? {
        b'k' | b'K' => (&s[..s.len() - 1], 1_000.0),
        b'm' | b'M' => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s, 1.0),
    };
    let digits = num.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if num.is_empty() || num.len() > 12 || !digits || num.starts_with('.') || num.ends_with('.') {
        return None;
    }
    let n: f64 = num.parse().ok()?;
    let tokens = (n * mult).round();
    (1.0..1e15).contains(&tokens).then_some(tokens as u64)
}

/// The model id without provider prefixes/suffixes and context tag (see the module docs).
pub fn canonical_model_id(id: &str) -> &str {
    let mut s = id.trim();
    // Each pass only shortens the id, so this ends; repeating makes the result a fixed point
    // (`x[1m]-v1` needs two passes).
    loop {
        let next = canonical_pass(s);
        if next == s {
            return s;
        }
        s = next;
    }
}

fn canonical_pass(id: &str) -> &str {
    let trimmed = id.trim();
    let mut s = split_ctx_tag(trimmed).0;
    // Vertex `…@20260101` / `…@latest`.
    if let Some(at) = s.find('@') {
        s = &s[..at];
    }
    // Gateways `org/claude-…`.
    if let Some(slash) = s.rfind('/') {
        s = &s[slash + 1..];
    }
    // Bedrock `us.anthropic.claude-…`: start at the last `claude-`.
    if let Some(at) = s.to_ascii_lowercase().rfind("claude-").filter(|&at| at > 0) {
        s = &s[at..];
    } else {
        // `anthropic.model-…`: drop leading all-letter `word.` segments.
        while let Some((head, rest)) = s.split_once('.') {
            if head.is_empty() || !head.bytes().all(|b| b.is_ascii_alphabetic()) || rest.is_empty() {
                break;
            }
            s = rest;
        }
    }
    // Trailing `-v1:0`, `-v2`, `-YYYYMMDD`, `-latest`, in any order.
    while let Some((rest, last)) = s.rsplit_once('-') {
        if !(is_revision(last) || is_date(last) || last.eq_ignore_ascii_case("latest")) {
            break;
        }
        s = rest;
    }
    if s.is_empty() { trimmed } else { s }
}

/// `v1`, `v1:0`, `v12:3`.
fn is_revision(token: &str) -> bool {
    let Some(rest) = token.strip_prefix(['v', 'V']) else { return false };
    let (major, minor) = rest.split_once(':').map_or((rest, None), |(a, b)| (a, Some(b)));
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
    digits(major) && minor.is_none_or(digits)
}

/// True if `a` and `b` name the same model: equal canonical ids (ASCII case ignored).
pub fn same_model(a: &str, b: &str) -> bool {
    canonical_model_id(a).eq_ignore_ascii_case(canonical_model_id(b))
}

/// Human-readable name for a model id, e.g. `claude-opus-5-5[1m]` → `Opus 5.5`.
///
/// `learned` maps canonical (or older: base) model ids to the statusline's `model.display_name`.
pub fn display_name(id: &str, learned: &BTreeMap<String, String>) -> String {
    let id = id.trim();
    if id == SYNTHETIC {
        return String::new();
    }
    let (base, _) = split_ctx_tag(id);
    let canonical = canonical_model_id(id);
    let found = learned.get(canonical).or_else(|| learned.get(base)).or_else(|| learned.get(id));
    if let Some(name) = found {
        let cleaned = strip_size_label(name);
        if !cleaned.is_empty() {
            return cleaned.to_string();
        }
    }
    heuristic_name(canonical).unwrap_or_else(|| id.to_string())
}

/// The canonical id's words without version numbers (`claude-opus-5-5` → `opus`,
/// `claude-opus-5-5-fast` → `opus-fast`); `None` when it has none.
pub fn model_family(id: &str) -> Option<String> {
    let tokens = name_tokens(canonical_model_id(id))?;
    let words: Vec<String> = tokens.words.iter().map(|w| w.to_ascii_lowercase()).collect();
    (!words.is_empty()).then(|| words.join("-"))
}

/// The canonical id's version numbers (`claude-opus-5-5` → `[5, 5]`, `claude-opus-4.5` →
/// `[4, 5]`), for ordering versions of one family; empty when it has none.
pub fn model_version(id: &str) -> Vec<u32> {
    name_tokens(canonical_model_id(id))
        .map(|t| t.version.iter().flat_map(|v| v.split('.')).filter_map(|p| p.parse().ok()).collect())
        .unwrap_or_default()
}

/// Removes trailing size markers such as `(1M context)`, `(2M)`, `[1m]`, ` 1M context` or a
/// bare ` 1M` after a version number, repeatedly.
fn strip_size_label(name: &str) -> &str {
    let mut s = name.trim();
    loop {
        let before = s;
        s = strip_parenthesised_size(s);
        let (base, tag) = split_ctx_tag(s);
        if tag.is_some_and(|t| t.hint.is_some()) {
            s = base;
        }
        s = strip_trailing_size_words(s);
        s = s.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '·' | ',' | '–'));
        if s == before {
            return s;
        }
    }
}

/// Strips a trailing `( … )` group whose first word is a size (e.g. `(1M context)`, `(2M)`).
fn strip_parenthesised_size(s: &str) -> &str {
    if !s.ends_with(')') {
        return s;
    }
    let Some(open) = s.rfind('(') else { return s };
    let inner = s[open + 1..s.len() - 1].trim();
    let first = inner.split_whitespace().next().unwrap_or("");
    if is_size_marker(first) { &s[..open] } else { s }
}

/// A size with a unit (`1M`, `500k`), the only kind of word a size marker is made of.
fn is_size_marker(word: &str) -> bool {
    word.ends_with(['k', 'K', 'm', 'M']) && parse_size_word(word).is_some()
}

/// ` 1M context`, or a bare ` 1M` right after a version number (`Sonnet 5 1M`, not `Model 11M`).
fn strip_trailing_size_words(s: &str) -> &str {
    let words: Vec<&str> = s.split_whitespace().collect();
    let n = words.len();
    let cut = if n >= 2 && words[n - 1].eq_ignore_ascii_case("context") && is_size_marker(words[n - 2]) {
        2
    } else if n >= 2 && is_size_marker(words[n - 1]) {
        let prev = words[..n - 1].iter().rev().find(|w| !matches!(**w, "·" | "-" | "–" | ","));
        usize::from(prev.is_some_and(|p| p.ends_with(|c: char| c.is_ascii_digit())))
    } else {
        0
    };
    if cut == 0 {
        return s;
    }
    let first_cut = words[n - cut];
    // Every word is a slice of `s`: cut right before the first removed one.
    &s[..first_cut.as_ptr() as usize - s.as_ptr() as usize]
}

struct NameTokens<'a> {
    words: Vec<&'a str>,
    version: Vec<&'a str>,
}

/// Splits a canonical id into words and version numbers (see the module docs).
fn name_tokens(id: &str) -> Option<NameTokens<'_>> {
    let mut tokens: Vec<&str> = id.split('-').collect();
    if tokens.first().is_some_and(|t| t.eq_ignore_ascii_case("claude")) {
        tokens.remove(0);
    }
    let mut words = Vec::new();
    let mut version = Vec::new();
    for token in tokens {
        if !token.is_empty() && token.bytes().all(|b| b.is_ascii_alphabetic()) {
            words.push(token);
        } else if is_version(token) {
            version.push(token);
        } else {
            return None;
        }
    }
    Some(NameTokens { words, version })
}

/// `5`, `45`, `4.5`, `2.0.1` (parts of 1–3 digits).
fn is_version(token: &str) -> bool {
    !token.is_empty()
        && token.split('.').all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `Family n.n Qualifier` from a canonical id (see the module docs).
fn heuristic_name(canonical: &str) -> Option<String> {
    let tokens = name_tokens(canonical)?;
    let (family, qualifiers) = tokens.words.split_first()?;
    let mut name = capitalise(family);
    if !tokens.version.is_empty() {
        name.push(' ');
        name.push_str(&tokens.version.join("."));
    }
    for q in qualifiers {
        name.push(' ');
        name.push_str(&capitalise(q));
    }
    Some(name)
}

fn is_date(token: &str) -> bool {
    token.len() == 8 && token.bytes().all(|b| b.is_ascii_digit())
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn split_ctx_tags() {
        assert_eq!(split_1m("claude-opus-5-5[1m]"), ("claude-opus-5-5", true));
        assert_eq!(split_1m("claude-opus-5-5[1M]"), ("claude-opus-5-5", true));
        assert_eq!(split_1m("claude-opus-5-5"), ("claude-opus-5-5", false));
        assert_eq!(split_1m("[1m]"), ("", true));
        assert_eq!(split_1m("1m]"), ("1m]", false));
        assert_eq!(split_1m(""), ("", false));
        assert_eq!(split_1m("é€"), ("é€", false));
        assert_eq!(split_1m("mod€l[1m]"), ("mod€l", true));
        // Any bracket tag is a long-context marker; a size tag also says how large.
        assert_eq!(split_ctx_tag("claude-x[2m]"), ("claude-x", Some(CtxTag { hint: Some(2_000_000) })));
        assert_eq!(split_ctx_tag("claude-x[500k]"), ("claude-x", Some(CtxTag { hint: Some(500_000) })));
        assert_eq!(split_ctx_tag("claude-x[1.5m]"), ("claude-x", Some(CtxTag { hint: Some(1_500_000) })));
        assert_eq!(split_ctx_tag("claude-x[long]"), ("claude-x", Some(CtxTag { hint: None })));
        assert_eq!(split_ctx_tag("claude-x[]"), ("claude-x[]", None));
        assert_eq!(split_ctx_tag("claude-x[a b]"), ("claude-x[a b]", None));
        assert_eq!(split_ctx_tag("claude-x[waytoolongtagtext]"), ("claude-x[waytoolongtagtext]", None));
        assert_eq!(ctx_tag_text("claude-x[2M]").as_deref(), Some("[2m]"));
        assert_eq!(ctx_tag_text("claude-x"), None);
    }

    #[test]
    fn size_words() {
        assert_eq!(parse_size_word("1m"), Some(1_000_000));
        assert_eq!(parse_size_word("500K"), Some(500_000));
        assert_eq!(parse_size_word("200000"), Some(200_000));
        for bad in ["", "m", ".5m", "5.m", "x1m", "1mm", "0k", "1e3"] {
            assert_eq!(parse_size_word(bad), None, "{bad}");
        }
    }

    #[test]
    fn canonical_ids_drop_provider_spellings() {
        for (id, want) in [
            ("claude-opus-5-5", "claude-opus-5-5"),
            ("claude-opus-5-5[1m]", "claude-opus-5-5"),
            ("claude-haiku-5-20261001", "claude-haiku-5"),
            ("us.anthropic.claude-opus-5-5-20260101-v1:0", "claude-opus-5-5"),
            ("us.anthropic.claude-opus-5-5-v1:0[1m]", "claude-opus-5-5"),
            ("anthropic.claude-opus-5-5-v2", "claude-opus-5-5"),
            ("claude-opus-5-5@20260101", "claude-opus-5-5"),
            ("claude-opus-5-5@latest", "claude-opus-5-5"),
            ("anthropic/claude-opus-5.5", "claude-opus-5.5"),
            ("org/team/claude-sonnet-5-latest", "claude-sonnet-5"),
            ("anthropic.nextgen-2-v1:0", "nextgen-2"),
            ("claude-opus-4.5", "claude-opus-4.5"),
            ("opus", "opus"),
            ("", ""),
            ("-v1", "-v1"),
        ] {
            assert_eq!(canonical_model_id(id), want, "{id}");
        }
        assert!(same_model("us.anthropic.claude-opus-5-5-v1:0", "claude-opus-5-5-20260101"));
        assert!(!same_model("claude-opus-5-5", "claude-opus-5"));
    }

    #[test]
    fn doc_examples() {
        let l = none();
        assert_eq!(display_name("claude-opus-5-5", &l), "Opus 5.5");
        assert_eq!(display_name("claude-haiku-4-5-20251001", &l), "Haiku 4.5");
        assert_eq!(display_name("claude-sonnet-5", &l), "Sonnet 5");
        assert_eq!(display_name("claude-3-5-sonnet-20241022", &l), "Sonnet 3.5");
        assert_eq!(display_name("claude-opus-5-5[1m]", &l), "Opus 5.5");
        assert_eq!(display_name("claude-fable-1", &l), "Fable 1");
        assert_eq!(display_name("claude-opus-5-5-fast", &l), "Opus 5.5 Fast");
    }

    #[test]
    fn other_families_and_shapes() {
        let l = none();
        assert_eq!(display_name("claude-3-opus-20240229", &l), "Opus 3");
        assert_eq!(display_name("claude-sonnet-4-5-latest", &l), "Sonnet 4.5");
        assert_eq!(display_name("claude-nova-2-1", &l), "Nova 2.1");
        assert_eq!(display_name("claude-opus", &l), "Opus");
        assert_eq!(display_name("claude-opus-4.5", &l), "Opus 4.5");
        assert_eq!(display_name("claude-opus-5-5-fast-mode", &l), "Opus 5.5 Fast Mode");
        assert_eq!(display_name("claude-opus-5-v2", &l), "Opus 5");
        assert_eq!(display_name("us.anthropic.claude-opus-5-5-20260101-v1:0", &l), "Opus 5.5");
        assert_eq!(display_name("claude-opus-5-5@20260101", &l), "Opus 5.5");
        assert_eq!(display_name("anthropic/claude-opus-5.5", &l), "Opus 5.5");
        assert_eq!(display_name("nextgen-2", &l), "Nextgen 2", "a family without the claude- prefix");
        assert_eq!(display_name("opus", &l), "Opus");
    }

    #[test]
    fn unknown_ids_unchanged() {
        let l = none();
        for id in ["gpt-4o", "claude-2", "claude-", "claude-opus-5--5", "claude-opus-1234", ""] {
            assert_eq!(display_name(id, &l), id, "{id}");
        }
        assert_eq!(display_name("mystery_model[1m]", &l), "mystery_model[1m]");
    }

    #[test]
    fn families() {
        assert_eq!(model_family("claude-opus-5-5").as_deref(), Some("opus"));
        assert_eq!(model_family("us.anthropic.claude-opus-6-v1:0").as_deref(), Some("opus"));
        assert_eq!(model_family("claude-opus-5-5-fast").as_deref(), Some("opus-fast"));
        assert_eq!(model_family("claude-2"), None);
        assert_eq!(model_family("gpt-4o"), None);
        assert_eq!(model_version("claude-opus-5-5"), [5, 5]);
        assert_eq!(model_version("claude-opus-4.5-20260101"), [4, 5]);
        assert!(model_version("claude-opus-6") > model_version("claude-opus-5-5"));
        assert!(model_version("gpt-4o").is_empty());
    }

    #[test]
    fn synthetic_is_empty() {
        let mut l = none();
        assert_eq!(display_name("<synthetic>", &l), "");
        l.insert("<synthetic>".into(), "Synthetic".into());
        assert_eq!(display_name("<synthetic>", &l), "", "synthetic never shows a name");
    }

    #[test]
    fn learned_wins_and_strips_size_labels() {
        let mut l = none();
        l.insert("claude-opus-5-5".into(), "Opus 5.5 (1M context)".into());
        l.insert("claude-sonnet-5".into(), "Sonnet 5 1M".into());
        l.insert("claude-haiku-4-5".into(), "Haiku Next".into());
        l.insert("claude-fable-1".into(), "Fable 1 [1m]".into());
        l.insert("claude-nova-1".into(), "Nova 1 (1m)".into());
        l.insert("claude-empty-1".into(), " (1M context) ".into());
        l.insert("claude-big-1".into(), "Big 1 (2M context)".into());
        assert_eq!(display_name("claude-opus-5-5[1m]", &l), "Opus 5.5");
        assert_eq!(display_name("claude-opus-5-5", &l), "Opus 5.5");
        assert_eq!(display_name("claude-sonnet-5[1m]", &l), "Sonnet 5");
        assert_eq!(display_name("claude-haiku-4-5", &l), "Haiku Next", "learned beats heuristic");
        assert_eq!(display_name("claude-fable-1", &l), "Fable 1");
        assert_eq!(display_name("claude-nova-1", &l), "Nova 1");
        assert_eq!(display_name("claude-empty-1", &l), "Empty 1", "empty learned name falls back");
        assert_eq!(display_name("claude-big-1[2m]", &l), "Big 1");
        // A provider spelling finds the name learned for the plain id.
        assert_eq!(display_name("us.anthropic.claude-haiku-4-5-v1:0", &l), "Haiku Next");
        assert_eq!(display_name("claude-haiku-4-5-20251001", &l), "Haiku Next");
    }

    #[test]
    fn strip_label_keeps_unrelated_parentheses() {
        assert_eq!(strip_size_label("Opus 5.5 (preview)"), "Opus 5.5 (preview)");
        assert_eq!(strip_size_label("Opus 5.5 (1M context)"), "Opus 5.5");
        assert_eq!(strip_size_label("Opus 5.5 (1Mx)"), "Opus 5.5 (1Mx)");
        assert_eq!(strip_size_label("Model 11M"), "Model 11M");
        assert_eq!(strip_size_label("Opus 5.5 - 1M context"), "Opus 5.5");
        assert_eq!(strip_size_label("Opus 5.5 · 1M"), "Opus 5.5");
        assert_eq!(strip_size_label("Opus 5.5 [long]"), "Opus 5.5 [long]");
    }

    proptest! {
        #[test]
        fn never_panics_and_canonical_is_idempotent(id in "\\PC{0,40}") {
            let c = canonical_model_id(&id);
            prop_assert_eq!(canonical_model_id(c), c);
            let _ = display_name(&id, &BTreeMap::new());
            let _ = split_ctx_tag(&id);
            let _ = model_family(&id);
        }

        #[test]
        fn provider_spellings_share_the_canonical_id(
            family in "[a-z]{3,8}", major in 1u8..20, minor in 0u8..10, date in 20200101u32..20291231,
        ) {
            let plain = format!("claude-{family}-{major}-{minor}");
            for spelled in [
                format!("us.anthropic.{plain}-{date}-v1:0[1m]"),
                format!("{plain}@{date}"),
                format!("gateway/{plain}"),
                format!("{plain}-{date}"),
            ] {
                prop_assert_eq!(canonical_model_id(&spelled), plain.as_str());
            }
        }
    }
}
