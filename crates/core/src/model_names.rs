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

pub fn split_1m(id: &str) -> (&str, bool) {
    let _ = id;
    todo!("model_names::split_1m")
}

pub fn display_name(id: &str, learned: &BTreeMap<String, String>) -> String {
    let _ = (id, learned);
    todo!("model_names::display_name")
}
