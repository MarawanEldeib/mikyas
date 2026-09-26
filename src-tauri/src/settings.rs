//! User settings, persisted as `<data_root>/settings.json`. Field names and defaults mirror the
//! `Settings` interface in `src/lib/types.ts`; unknown, missing or invalid fields fall back to
//! their defaults one by one, so an older or hand-edited file never prevents the app from
//! starting and never loses its other fields. A file that is not JSON at all is renamed to
//! `settings.json.bad-<ms>` before the defaults can overwrite it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use cuw_core::time::{MINUTE_MS, Ms, now_ms};

use crate::diag::log;
use crate::state::save_json;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Pill,
    Card,
    Settings,
    /// List of every recent session.
    Sessions,
    /// 14-day usage history.
    History,
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

/// Chrome accent colour. `Auto` = neutral chrome; usage colours always follow the thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    Auto,
    Blue,
    Violet,
    Teal,
    Green,
    Amber,
    Rose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GaugeStyle {
    Ring,
    Bar,
}

/// Screen edge the widget docks to (`Off` = free-floating).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockEdge {
    Off,
    Left,
    Right,
    Top,
}

/// What the widget's own × button does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// Hide to the tray (the app keeps running).
    #[default]
    Hide,
    Quit,
}

/// Which live % the tray icon shows (`Off` = the coloured dot icons).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayNumber {
    /// The highest of the 5-hour and weekly %.
    #[default]
    Worst,
    FiveHour,
    SevenDay,
    Off,
}

/// Which optional rows the card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CardRows {
    pub sparklines: bool,
    pub burn: bool,
    pub session: bool,
    pub sources: bool,
}

impl Default for CardRows {
    fn default() -> Self {
        Self { sparklines: true, burn: true, session: true, sources: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    /// Persisted as `pill` or `card` only (Settings, Sessions and History are transient).
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
    /// Toast when the active session's context crosses a threshold.
    pub ctx_alerts: bool,
    /// Ascending context-% thresholds, e.g. [80, 90].
    pub ctx_thresholds: Vec<u8>,
    /// Global shortcut that shows/hides the widget ("" = none).
    pub toggle_hotkey: String,
    /// Hide automatically while a fullscreen video or app is in front.
    pub auto_hide_fullscreen: bool,
    /// Opt-in daily check of GitHub Releases (the app's only network call).
    pub check_updates: bool,
    pub accent: Accent,
    pub gauge_style: GaugeStyle,
    /// UI and window scale, 0.85..=1.3.
    pub ui_scale: f32,
    pub card_rows: CardRows,
    pub dock: DockEdge,
    pub close_action: CloseAction,
    /// Toast when the current pace reaches a limit before it resets.
    pub pace_alerts: bool,
    /// Toast shortly before a capped limit reopens.
    pub reset_heads_up: bool,
    /// One summary toast when the weekly limit resets.
    pub weekly_recap: bool,
    /// Toast when a Claude turn that ran at least `finished_min_minutes` ends.
    pub finished_alerts: bool,
    /// 1..=60.
    pub finished_min_minutes: u32,
    /// Warn when Claude Code's status line no longer runs the widget's capture.
    pub connection_watchdog: bool,
    /// Remember the widget position per monitor setup.
    pub per_display_position: bool,
    pub tray_number: TrayNumber,
    /// The one-time "still running" toast after the first hide from the widget was shown.
    /// Internal: no UI control, and settings patches never change it.
    pub hide_hint_shown: bool,
}

/// Settings the app keeps for itself: `apply_patch` ignores them.
const INTERNAL_KEYS: &[&str] = &["hide_hint_shown"];

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            view: ViewMode::Card,
            pinned: true,
            opacity: 1.0,
            ghost_opacity: 0.45,
            // Mica/Acrylic look flat while the (non-activating) widget is unfocused (M0 effect spike).
            effect: EffectName::None,
            thresholds: vec![80, 95],
            notify_reset: true,
            hotkey: "Ctrl+Alt+U".into(),
            stale_min: 20,
            ctx_overrides: BTreeMap::new(),
            show_project: false,
            start_with_windows: false,
            ctx_alerts: true,
            ctx_thresholds: vec![80, 90],
            toggle_hotkey: "Ctrl+Alt+H".into(),
            auto_hide_fullscreen: true,
            check_updates: false,
            accent: Accent::Auto,
            gauge_style: GaugeStyle::Ring,
            ui_scale: 1.0,
            card_rows: CardRows::default(),
            dock: DockEdge::Off,
            close_action: CloseAction::Hide,
            pace_alerts: true,
            reset_heads_up: true,
            weekly_recap: true,
            finished_alerts: true,
            finished_min_minutes: 3,
            connection_watchdog: true,
            per_display_position: true,
            tray_number: TrayNumber::Worst,
            hide_hint_shown: false,
        }
    }
}

impl Settings {
    /// Clamps every field into its valid range.
    pub fn sanitized(mut self) -> Self {
        self.schema_version = SCHEMA_VERSION;
        if !matches!(self.view, ViewMode::Pill | ViewMode::Card) {
            self.view = ViewMode::Card;
        }
        self.ctx_thresholds.retain(|t| (1..=100).contains(t));
        self.ctx_thresholds.sort_unstable();
        self.ctx_thresholds.dedup();
        self.toggle_hotkey = self.toggle_hotkey.trim().to_owned();
        self.ui_scale = clamp_or(self.ui_scale, 0.85, 1.3, 1.0);
        self.opacity = clamp_or(self.opacity, 0.3, 1.0, 1.0);
        self.ghost_opacity = clamp_or(self.ghost_opacity, 0.15, 1.0, 0.45);
        self.thresholds.retain(|t| (1..=100).contains(t));
        self.thresholds.sort_unstable();
        self.thresholds.dedup();
        self.stale_min = self.stale_min.clamp(1, 24 * 60);
        self.hotkey = self.hotkey.trim().to_owned();
        self.ctx_overrides.retain(|k, v| !k.trim().is_empty() && *v > 0);
        self.finished_min_minutes = self.finished_min_minutes.clamp(1, 60);
        self
    }

    pub fn stale_after_ms(&self) -> Ms {
        Ms::from(self.stale_min) * MINUTE_MS
    }
}

fn clamp_or(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { fallback }
}

/// Loads settings (see the module docs); a missing or unreadable file yields the defaults.
pub fn load(path: &Path) -> Settings {
    let Ok(bytes) = std::fs::read(path) else {
        return Settings::default().sanitized();
    };
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    match serde_json::from_slice::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(file)) => merge_fields(&file),
        _ => {
            set_aside(path);
            Settings::default()
        }
    }
    .sanitized()
}

/// The defaults with every field of `file` that deserialises on its own applied.
fn merge_fields(file: &serde_json::Map<String, serde_json::Value>) -> Settings {
    let defaults = Settings::default();
    let whole = || -> Option<Settings> {
        let mut value = serde_json::to_value(&defaults).ok()?;
        let obj = value.as_object_mut()?;
        for (k, v) in file {
            obj.insert(k.clone(), v.clone());
        }
        serde_json::from_value(value).ok()
    };
    if let Some(settings) = whole() {
        return settings;
    }
    let Ok(serde_json::Value::Object(mut merged)) = serde_json::to_value(&defaults) else {
        return defaults;
    };
    for (k, v) in file {
        let Some(default) = merged.get(k).cloned() else { continue };
        merged.insert(k.clone(), v.clone());
        if serde_json::from_value::<Settings>(serde_json::Value::Object(merged.clone())).is_err() {
            log(&format!("settings.json: invalid `{k}`, using its default"));
            merged.insert(k.clone(), default);
        }
    }
    serde_json::from_value(serde_json::Value::Object(merged)).unwrap_or(defaults)
}

/// Renames an unparsable settings file so the next save does not overwrite it.
fn set_aside(path: &Path) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".bad-{}", now_ms()));
    match std::fs::rename(path, &aside) {
        Ok(()) => log("settings.json is not valid JSON; kept it as settings.json.bad-<ms> and using defaults"),
        Err(e) => log(&format!("settings.json is not valid JSON and could not be set aside: {e}")),
    }
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    save_json(path, settings)
}

/// Applies a partial update (`Partial<Settings>` from the UI). Unknown and internal keys are
/// ignored; a value of the wrong type is an error.
pub fn apply_patch(current: &Settings, patch: &serde_json::Value) -> Result<Settings, String> {
    let serde_json::Value::Object(patch) = patch else {
        return Err("settings patch must be an object".into());
    };
    let mut value = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let obj = value.as_object_mut().ok_or("settings did not serialise to an object")?;
    for (k, v) in patch {
        if obj.contains_key(k) && !INTERNAL_KEYS.contains(&k.as_str()) {
            obj.insert(k.clone(), v.clone());
        }
    }
    let next: Settings = serde_json::from_value(value).map_err(|e| format!("invalid settings value: {e}"))?;
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
        assert_eq!(v["ctx_alerts"], true);
        assert_eq!(v["ctx_thresholds"], serde_json::json!([80, 90]));
        assert_eq!(v["toggle_hotkey"], "Ctrl+Alt+H");
        assert_eq!(v["auto_hide_fullscreen"], true);
        assert_eq!(v["check_updates"], false);
        assert_eq!(v["accent"], "auto");
        assert_eq!(v["gauge_style"], "ring");
        assert_eq!(v["ui_scale"], 1.0);
        assert_eq!(
            v["card_rows"],
            serde_json::json!({"sparklines": true, "burn": true, "session": true, "sources": true})
        );
        assert_eq!(v["dock"], "off");
        assert_eq!(v["close_action"], "hide");
        assert_eq!(v["pace_alerts"], true);
        assert_eq!(v["reset_heads_up"], true);
        assert_eq!(v["weekly_recap"], true);
        assert_eq!(v["finished_alerts"], true);
        assert_eq!(v["finished_min_minutes"], 3);
        assert_eq!(v["connection_watchdog"], true);
        assert_eq!(v["per_display_position"], true);
        assert_eq!(v["tray_number"], "worst");
        assert_eq!(v["hide_hint_shown"], false);
    }

    #[test]
    fn automation_settings_patch_clamp_and_default_for_older_files() {
        let s = apply_patch(&Settings::default(), &serde_json::json!({"finished_min_minutes": 0})).unwrap();
        assert_eq!(s.finished_min_minutes, 1);
        let s = apply_patch(&s, &serde_json::json!({"finished_min_minutes": 999})).unwrap();
        assert_eq!(s.finished_min_minutes, 60);
        let s = apply_patch(&s, &serde_json::json!({"tray_number": "five_hour", "pace_alerts": false})).unwrap();
        assert_eq!(s.tray_number, TrayNumber::FiveHour);
        assert!(!s.pace_alerts);
        assert!(apply_patch(&s, &serde_json::json!({"tray_number": "best"})).is_err());
        assert!(apply_patch(&s, &serde_json::json!({"finished_min_minutes": -1})).is_err());
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        std::fs::write(&p, br#"{"view":"card"}"#).unwrap();
        let old = load(&p);
        assert!(old.pace_alerts && old.reset_heads_up && old.weekly_recap && old.finished_alerts);
        assert!(old.connection_watchdog && old.per_display_position);
        assert_eq!(old.finished_min_minutes, 3);
        assert_eq!(old.tray_number, TrayNumber::Worst);
    }

    #[test]
    fn close_action_patches_and_older_files_default_to_hide() {
        let s = apply_patch(&Settings::default(), &serde_json::json!({"close_action": "quit"})).unwrap();
        assert_eq!(s.close_action, CloseAction::Quit);
        assert!(apply_patch(&s, &serde_json::json!({"close_action": "minimize"})).is_err());
        // A settings.json from before the setting (and before the hint flag).
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        std::fs::write(&p, br#"{"view":"pill","dock":"left"}"#).unwrap();
        let old = load(&p);
        assert_eq!(old.close_action, CloseAction::Hide);
        assert!(!old.hide_hint_shown);
    }

    #[test]
    fn patches_never_change_the_hide_hint_flag() {
        let shown = Settings { hide_hint_shown: true, ..Settings::default() };
        let next = apply_patch(&shown, &serde_json::json!({"hide_hint_shown": false, "opacity": 0.8})).unwrap();
        assert!(next.hide_hint_shown, "a UI patch cannot reset it");
        assert_eq!(next.opacity, 0.8, "the rest of the patch still applies");
        let fresh = apply_patch(&Settings::default(), &serde_json::json!({"hide_hint_shown": true})).unwrap();
        assert!(!fresh.hide_hint_shown, "nor set it");
        // Any other patch keeps it, and it survives a save.
        let kept = apply_patch(&shown, &serde_json::json!({"close_action": "quit"})).unwrap();
        assert!(kept.hide_hint_shown);
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        save(&p, &kept).unwrap();
        assert!(load(&p).hide_hint_shown);
    }

    #[test]
    fn transient_views_are_never_persisted() {
        for view in ["settings", "sessions", "history"] {
            let next = apply_patch(&Settings::default(), &serde_json::json!({ "view": view })).unwrap();
            assert_eq!(next.view, ViewMode::Card, "{view}");
        }
        let s = apply_patch(&Settings::default(), &serde_json::json!({"ui_scale": 9, "ctx_thresholds": [90, 0, 80]}))
            .unwrap();
        assert_eq!(s.ui_scale, 1.3);
        assert_eq!(s.ctx_thresholds, vec![80, 90]);
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
    }

    #[test]
    fn one_invalid_field_keeps_the_others() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        std::fs::write(
            &p,
            br#"{"view":"pill","opacity":"loud","hotkey":"Ctrl+Alt+K","hide_hint_shown":true,"card_rows":{"burn":false}}"#,
        )
        .unwrap();
        let s = load(&p);
        assert_eq!(s.view, ViewMode::Pill);
        assert_eq!(s.opacity, 1.0, "the bad field falls back alone");
        assert_eq!(s.hotkey, "Ctrl+Alt+K");
        assert!(s.hide_hint_shown, "internal fields load too");
        assert!(!s.card_rows.burn && s.card_rows.sparklines);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1, "a readable file is kept as is");
    }

    #[test]
    fn an_unparsable_file_is_set_aside_before_it_can_be_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        std::fs::write(&p, b"{not json").unwrap();
        assert_eq!(load(&p), Settings::default());
        let names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(names[0].starts_with("settings.json.bad-"), "{names:?}");
        assert_eq!(std::fs::read(tmp.path().join(&names[0])).unwrap(), b"{not json");
    }

    #[test]
    fn save_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sub").join("settings.json");
        let mut s = Settings { effect: EffectName::Mica, ..Settings::default() };
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
