//! User settings, persisted as `<data_root>/settings.json`. Field names and defaults mirror the
//! `Settings` interface in `src/lib/types.ts`; unknown or missing fields fall back to defaults so
//! an older or hand-edited file never prevents the app from starting.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::state::write_atomic;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Pill,
    Card,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectName {
    Auto,
    Mica,
    Acrylic,
    Blur,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    /// Persisted as `pill` or `card` only (Settings is transient).
    pub view: ViewMode,
    pub pinned: bool,
    pub opacity: f32,
    pub ghost_opacity: f32,
    pub effect: EffectName,
    pub thresholds: Vec<u8>,
    pub notify_reset: bool,
    pub hotkey: String,
    pub stale_min: u32,
    pub ctx_overrides: BTreeMap<String, u64>,
    pub show_project: bool,
    pub start_with_windows: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            view: ViewMode::Card,
            pinned: true,
            opacity: 1.0,
            ghost_opacity: 0.45,
            // Mica/Acrylic look flat while the (non-activating) widget is unfocused; see docs/spikes.
            effect: EffectName::None,
            thresholds: vec![80, 95],
            notify_reset: true,
            hotkey: "Ctrl+Alt+U".into(),
            stale_min: 20,
            ctx_overrides: BTreeMap::new(),
            show_project: false,
            start_with_windows: false,
        }
    }
}

impl Settings {
    /// Clamps every field into its valid range.
    pub fn sanitized(mut self) -> Self {
        self.schema_version = SCHEMA_VERSION;
        if self.view == ViewMode::Settings {
            self.view = ViewMode::Card;
        }
        self.opacity = clamp_or(self.opacity, 0.3, 1.0, 1.0);
        self.ghost_opacity = clamp_or(self.ghost_opacity, 0.15, 1.0, 0.45);
        self.thresholds.retain(|t| (1..=100).contains(t));
        self.thresholds.sort_unstable();
        self.thresholds.dedup();
        self.stale_min = self.stale_min.clamp(1, 24 * 60);
        self.hotkey = self.hotkey.trim().to_owned();
        self.ctx_overrides.retain(|k, v| !k.trim().is_empty() && *v > 0);
        self
    }

    pub fn stale_after_ms(&self) -> i64 {
        i64::from(self.stale_min) * 60_000
    }
}

fn clamp_or(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { fallback }
}

/// Loads settings; a missing or unreadable file yields the defaults.
pub fn load(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| {
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes).to_vec();
            serde_json::from_slice::<Settings>(&bytes).ok()
        })
        .unwrap_or_default()
        .sanitized()
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Applies a partial update (`Partial<Settings>` from the UI). Unknown keys are ignored; a value
/// of the wrong type is an error.
pub fn apply_patch(current: &Settings, patch: &serde_json::Value) -> Result<Settings, String> {
    let serde_json::Value::Object(patch) = patch else {
        return Err("settings patch must be an object".into());
    };
    let mut value = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let obj = value.as_object_mut().ok_or("settings did not serialise to an object")?;
    for (k, v) in patch {
        if obj.contains_key(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
    let next: Settings =
        serde_json::from_value(value).map_err(|e| format!("invalid settings value: {e}"))?;
    Ok(next.sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_ui_contract() {
        let v = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(v["view"], "card");
        assert_eq!(v["pinned"], true);
        assert_eq!(v["opacity"], 1.0);
        assert!((v["ghost_opacity"].as_f64().unwrap() - 0.45).abs() < 1e-6);
        assert_eq!(v["effect"], "none");
        assert_eq!(v["thresholds"], serde_json::json!([80, 95]));
        assert_eq!(v["notify_reset"], true);
        assert_eq!(v["hotkey"], "Ctrl+Alt+U");
        assert_eq!(v["stale_min"], 20);
        assert_eq!(v["ctx_overrides"], serde_json::json!({}));
        assert_eq!(v["show_project"], false);
        assert_eq!(v["start_with_windows"], false);
        assert_eq!(v["schema_version"], 1);
    }

    #[test]
    fn missing_file_and_partial_file_use_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        assert_eq!(load(&p), Settings::default());
        std::fs::write(&p, br#"{"view":"pill","opacity":7,"bogus":1}"#).unwrap();
        let s = load(&p);
        assert_eq!(s.view, ViewMode::Pill);
        assert_eq!(s.opacity, 1.0);
        assert_eq!(s.hotkey, "Ctrl+Alt+U");
        std::fs::write(&p, b"{not json").unwrap();
        assert_eq!(load(&p), Settings::default());
    }

    #[test]
    fn save_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sub").join("settings.json");
        let mut s = Settings {
            effect: EffectName::Mica,
            ..Settings::default()
        };
        s.ctx_overrides.insert("claude-opus-5-5".into(), 1_000_000);
        save(&p, &s).unwrap();
        assert_eq!(load(&p), s);
    }

    #[test]
    fn patch_applies_and_sanitises() {
        let s = Settings::default();
        let next = apply_patch(
            &s,
            &serde_json::json!({"thresholds":[95, 80, 80, 0], "view":"settings", "ghost_opacity": 0.01, "nope": 3}),
        )
        .unwrap();
        assert_eq!(next.thresholds, vec![80, 95]);
        assert_eq!(next.view, ViewMode::Card, "settings view is never persisted");
        assert_eq!(next.ghost_opacity, 0.15);
        assert!(apply_patch(&s, &serde_json::json!({"pinned": "yes"})).is_err());
        assert!(apply_patch(&s, &serde_json::json!([1])).is_err());
    }
}
