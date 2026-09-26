//! Model id → display name.
//!
//! `split_1m("claude-opus-5-5[1m]") == ("claude-opus-5-5", true)`.
//!
//! `display_name(id, learned)`:
//! 1. Strip a `[1m]` suffix. If `learned` has the base id (learned from statusline
//!    `model.display_name`, which is authoritative), return that — but with any "(1M context)" /
//!    " 1M" style suffix removed (the UI shows 1M separately).
//! 2. Heuristic: strip `claude-`; strip a trailing `-YYYYMMDD` date; the first token is the family
//!    (capitalised: opus → Opus, sonnet → Sonnet, haiku → Haiku, fable → Fable, any other word
//!    capitalised); the remaining numeric tokens form the version joined by `.`
//!    (`claude-opus-5-5` → "Opus 5.5", `claude-haiku-4-5-20251001` → "Haiku 4.5",
//!    `claude-sonnet-5` → "Sonnet 5", `claude-3-5-sonnet-20241022` → "Sonnet 3.5" — the old
//!    number-first scheme puts the version before the family).
//! 3. Anything that doesn't fit → the id unchanged. `<synthetic>` → "".

use std::collections::BTreeMap;

/// Placeholder model Claude Code writes for locally generated (non-API) messages.
const SYNTHETIC: &str = "<synthetic>";
const ONE_M_SUFFIX: &str = "[1m]";

/// Splits a trailing `[1m]` (1M-context marker, matched case-insensitively) off a model id.
pub fn split_1m(id: &str) -> (&str, bool) {
    let Some(cut) = id.len().checked_sub(ONE_M_SUFFIX.len()) else {
        return (id, false);
    };
    match id.get(cut..) {
        Some(tail) if tail.eq_ignore_ascii_case(ONE_M_SUFFIX) => (&id[..cut], true),
        _ => (id, false),
    }
}

/// Human-readable name for a model id, e.g. `claude-opus-5-5[1m]` → `Opus 5.5`.
///
/// `learned` maps base model ids (without `[1m]`) to the statusline's `model.display_name`.
pub fn display_name(id: &str, learned: &BTreeMap<String, String>) -> String {
    let id = id.trim();
    if id == SYNTHETIC {
        return String::new();
    }
    let (base, _) = split_1m(id);
    if let Some(name) = learned.get(base).or_else(|| learned.get(id)) {
        let cleaned = strip_1m_label(name);
        if !cleaned.is_empty() {
            return cleaned.to_string();
        }
    }
    heuristic_name(base).unwrap_or_else(|| id.to_string())
}

/// Removes trailing 1M markers such as `(1M context)`, `(1M)`, `[1m]` or ` 1M`, repeatedly.
fn strip_1m_label(name: &str) -> &str {
    let mut s = name.trim();
    loop {
        let before = s;
        s = strip_parenthesised_1m(s);
        if let Some(rest) = strip_suffix_ci(s, ONE_M_SUFFIX) {
            s = rest;
        }
        for suffix in [" 1m context", " 1m"] {
            if let Some(rest) = strip_suffix_ci(s, suffix) {
                s = rest;
            }
        }
        s = s.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '·' | ',' | '–'));
        if s == before {
            return s;
        }
    }
}

/// Strips a trailing `( … )` group whose content starts with `1m` (e.g. `(1M context)`).
fn strip_parenthesised_1m(s: &str) -> &str {
    if !s.ends_with(')') {
        return s;
    }
    let Some(open) = s.rfind('(') else { return s };
    let inner = s[open + 1..s.len() - 1].trim();
    let is_1m = inner.get(..2).is_some_and(|head| head.eq_ignore_ascii_case("1m"))
        && inner[2..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric());
    if is_1m { &s[..open] } else { s }
}

fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = s.len().checked_sub(suffix.len())?;
    let tail = s.get(cut..)?;
    tail.eq_ignore_ascii_case(suffix).then(|| &s[..cut])
}

/// `claude-<family>-<n>-<n>[-YYYYMMDD]` or `claude-<n>-<n>-<family>[-YYYYMMDD]` → `Family n.n`.
fn heuristic_name(base: &str) -> Option<String> {
    let rest = base.strip_prefix("claude-")?;
    let mut tokens: Vec<&str> = rest.split('-').collect();
    if tokens.last().is_some_and(|t| *t == "latest" || is_date(t)) {
        tokens.pop();
    }
    let mut family = None;
    let mut version = Vec::new();
    for token in tokens {
        if !token.is_empty() && token.bytes().all(|b| b.is_ascii_alphabetic()) {
            if family.replace(token).is_some() {
                return None; // two words: not a shape we understand
            }
        } else if !token.is_empty() && token.len() <= 3 && token.bytes().all(|b| b.is_ascii_digit()) {
            version.push(token);
        } else {
            return None;
        }
    }
    let family = capitalise(family?);
    if version.is_empty() { Some(family) } else { Some(format!("{family} {}", version.join("."))) }
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

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn split_1m_suffix() {
        assert_eq!(split_1m("claude-opus-5-5[1m]"), ("claude-opus-5-5", true));
        assert_eq!(split_1m("claude-opus-5-5[1M]"), ("claude-opus-5-5", true));
        assert_eq!(split_1m("claude-opus-5-5"), ("claude-opus-5-5", false));
        assert_eq!(split_1m("[1m]"), ("", true));
        assert_eq!(split_1m("1m]"), ("1m]", false));
        assert_eq!(split_1m(""), ("", false));
        // Multi-byte chars before the suffix position must not panic.
        assert_eq!(split_1m("é€"), ("é€", false));
        assert_eq!(split_1m("mod€l[1m]"), ("mod€l", true));
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
    }

    #[test]
    fn other_families_and_shapes() {
        let l = none();
        assert_eq!(display_name("claude-3-opus-20240229", &l), "Opus 3");
        assert_eq!(display_name("claude-sonnet-4-5-latest", &l), "Sonnet 4.5");
        assert_eq!(display_name("claude-nova-2-1", &l), "Nova 2.1");
        assert_eq!(display_name("claude-opus", &l), "Opus");
    }

    #[test]
    fn unknown_ids_unchanged() {
        let l = none();
        for id in [
            "gpt-4o",
            "opus",
            "claude-2",
            "claude-",
            "claude-opus-5-5-fast-mode",
            "claude-opus-5--5",
            "claude-opus-5-v2",
            "claude-opus-1234",
            "",
        ] {
            assert_eq!(display_name(id, &l), id, "{id}");
        }
        // Unknown 1M ids keep the full id, suffix included.
        assert_eq!(display_name("mystery[1m]", &l), "mystery[1m]");
    }

    #[test]
    fn synthetic_is_empty() {
        let mut l = none();
        assert_eq!(display_name("<synthetic>", &l), "");
        l.insert("<synthetic>".into(), "Synthetic".into());
        assert_eq!(display_name("<synthetic>", &l), "", "synthetic never shows a name");
    }

    #[test]
    fn learned_wins_and_strips_1m_labels() {
        let mut l = none();
        l.insert("claude-opus-5-5".into(), "Opus 5.5 (1M context)".into());
        l.insert("claude-sonnet-5".into(), "Sonnet 5 1M".into());
        l.insert("claude-haiku-4-5".into(), "Haiku Next".into());
        l.insert("claude-fable-1".into(), "Fable 1 [1m]".into());
        l.insert("claude-nova-1".into(), "Nova 1 (1m)".into());
        l.insert("claude-empty-1".into(), " (1M context) ".into());
        assert_eq!(display_name("claude-opus-5-5[1m]", &l), "Opus 5.5");
        assert_eq!(display_name("claude-opus-5-5", &l), "Opus 5.5");
        assert_eq!(display_name("claude-sonnet-5[1m]", &l), "Sonnet 5");
        assert_eq!(display_name("claude-haiku-4-5", &l), "Haiku Next", "learned beats heuristic");
        assert_eq!(display_name("claude-fable-1", &l), "Fable 1");
        assert_eq!(display_name("claude-nova-1", &l), "Nova 1");
        assert_eq!(display_name("claude-empty-1", &l), "Empty 1", "empty learned name falls back");
    }

    #[test]
    fn strip_label_keeps_unrelated_parentheses() {
        assert_eq!(strip_1m_label("Opus 5.5 (preview)"), "Opus 5.5 (preview)");
        assert_eq!(strip_1m_label("Opus 5.5 (1M context)"), "Opus 5.5");
        assert_eq!(strip_1m_label("Opus 5.5 (1Mx)"), "Opus 5.5 (1Mx)");
        assert_eq!(strip_1m_label("Model 11M"), "Model 11M");
        assert_eq!(strip_1m_label("Opus 5.5 - 1M context"), "Opus 5.5");
        assert_eq!(strip_1m_label("Opus 5.5 · 1M"), "Opus 5.5");
    }
}
