//! Shared data model. These types are serialised to the UI as JSON (see `src/lib/types.ts`,
//! which must stay in sync), so every serde attribute here is part of the UI contract.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::time::{FIVE_HOURS_MS, Ms, SEVEN_DAYS_MS};

/// A drop of at least this many points between two measurements of one window means the window
/// reset. Desktop reports integers that can lag the CLI's one-decimal values a little (80.2, then
/// 79), so a smaller dip is noise. Merge (early reset), reset estimation, alerts, the history view
/// and the weekly recap all use this one rule: see [`is_reset_drop`].
pub const RESET_DROP_PCT: f32 = 2.0;

/// True if going from `before` to `after` is a reset (a drop of at least [`RESET_DROP_PCT`]).
pub fn is_reset_drop(before: f32, after: f32) -> bool {
    before - after >= RESET_DROP_PCT
}

/// A usage-limit window. Serialised as a plain string: `"five_hour"`, `"seven_day"`, or any
/// other key Anthropic adds later (e.g. `"seven_day_opus"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowKind {
    FiveHour,
    SevenDay,
    Other(String),
}

impl WindowKind {
    pub fn key(&self) -> &str {
        match self {
            WindowKind::FiveHour => "five_hour",
            WindowKind::SevenDay => "seven_day",
            WindowKind::Other(k) => k,
        }
    }

    /// Parses statusline keys (`five_hour`, `seven_day`, ...) and Desktop keys (`fh`, `sd`,
    /// `so` = seven_day_opus, `sn` = seven_day_sonnet).
    pub fn from_key(key: &str) -> Self {
        match key {
            "five_hour" | "fh" => WindowKind::FiveHour,
            "seven_day" | "sd" => WindowKind::SevenDay,
            "so" => WindowKind::Other("seven_day_opus".into()),
            "sn" => WindowKind::Other("seven_day_sonnet".into()),
            other => WindowKind::Other(other.to_string()),
        }
    }

    /// Nominal window length, if known.
    pub fn duration_ms(&self) -> Option<Ms> {
        match self {
            WindowKind::FiveHour => Some(FIVE_HOURS_MS),
            WindowKind::SevenDay => Some(SEVEN_DAYS_MS),
            WindowKind::Other(k) if k.starts_with("seven_day") => Some(SEVEN_DAYS_MS),
            WindowKind::Other(_) => None,
        }
    }

    /// Compact label used in history rows and the UI: `5h`, `7d`, or the raw key.
    pub fn short(&self) -> &str {
        match self {
            WindowKind::FiveHour => "5h",
            WindowKind::SevenDay => "7d",
            WindowKind::Other(k) => k,
        }
    }

    /// True if the history label `s` names this kind (as [`Self::from_short`] would parse it),
    /// without allocating.
    pub fn matches_short(&self, s: &str) -> bool {
        match self {
            WindowKind::FiveHour => matches!(s, "5h" | "five_hour" | "fh"),
            WindowKind::SevenDay => matches!(s, "7d" | "seven_day" | "sd"),
            WindowKind::Other(k) => match s {
                "5h" | "five_hour" | "fh" | "7d" | "seven_day" | "sd" => false,
                "so" => k == "seven_day_opus",
                "sn" => k == "seven_day_sonnet",
                other => k == other,
            },
        }
    }

    pub fn from_short(s: &str) -> Self {
        match s {
            "5h" => WindowKind::FiveHour,
            "7d" => WindowKind::SevenDay,
            other => WindowKind::from_key(other),
        }
    }
}

impl fmt::Display for WindowKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl Serialize for WindowKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.key())
    }
}

impl<'de> Deserialize<'de> for WindowKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(WindowKind::from_key(&s))
    }
}

/// Where a usage value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Claude Code statusline capture (exact values and reset times).
    Cli,
    /// Claude Desktop's `plan-usage-history.json` (integer %, no reset times).
    Desktop,
}

/// One measured value of one window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub kind: WindowKind,
    /// 0.0..=100.0 (clamped by the producer).
    pub pct: f32,
    /// Exact reset time (CLI only).
    pub resets_at_ms: Option<Ms>,
    /// When the value was measured (CLI: capture `changed_at_ms`; Desktop: sample `t`).
    pub observed_at_ms: Ms,
    pub source: Source,
}

/// A point of a usage time series (Desktop samples or history rows).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub t_ms: Ms,
    pub pct: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// When the window resets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResetInfo {
    /// From Claude Code's `resets_at`.
    Exact {
        at_ms: Ms,
    },
    /// Inferred from Desktop samples / history; shown with a "~".
    Estimated {
        at_ms: Ms,
        plus_minus_ms: Ms,
        confidence: Confidence,
    },
    Unknown,
}

impl ResetInfo {
    pub fn at_ms(&self) -> Option<Ms> {
        match self {
            ResetInfo::Exact { at_ms } | ResetInfo::Estimated { at_ms, .. } => Some(*at_ms),
            ResetInfo::Unknown => None,
        }
    }
    pub fn is_exact(&self) -> bool {
        matches!(self, ResetInfo::Exact { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Active,
    /// The known reset time has passed and no newer measurement exists yet; shown as 0%.
    ResetAwaitingData,
}

/// Merged, display-ready state of one window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub kind: WindowKind,
    pub pct: f32,
    pub reset: ResetInfo,
    pub source: Source,
    pub observed_at_ms: Ms,
    /// `now - observed_at_ms > stale_after`.
    pub stale: bool,
    /// pct >= 99.5 (Desktop reports integers and does reach 100).
    pub limit_reached: bool,
    pub phase: Phase,
}

/// Burn-rate forecast for one window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Burn {
    /// Percentage points per hour (> 0).
    pub slope_pct_per_h: f32,
    /// When 100% will be reached at this pace (always known: a burn exists only for a positive
    /// slope).
    pub t100_ms: Ms,
    /// Projected % at the reset time (may exceed 100 before clamping in the UI).
    pub pct_at_reset: Option<f32>,
    /// True if 100% is projected before the reset.
    pub hits_limit_before_reset: bool,
}

/// Sparkline point; `pct: None` marks a gap (the UI breaks the line there).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SparkPoint {
    pub t_ms: Ms,
    pub pct: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowView {
    #[serde(flatten)]
    pub state: WindowState,
    pub burn: Option<Burn>,
    pub spark: Vec<SparkPoint>,
    /// The value is older than Claude activity seen since in the transcripts, so the real % is
    /// probably higher (the UI marks it "▲").
    #[serde(default)]
    pub worked_since: bool,
}

/// Which Claude surface a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entrypoint {
    /// Claude Code in a terminal.
    Cli,
    /// Claude Desktop's Code tab.
    Desktop,
    /// Claude Desktop Cowork (local agent mode).
    Cowork,
    Unknown,
}

impl Entrypoint {
    /// Maps the transcript `entrypoint` field (`cli`, `claude-desktop`, `local-agent`, ...).
    pub fn from_transcript(s: &str) -> Self {
        match s {
            "cli" | "sdk-cli" => Entrypoint::Cli,
            "claude-desktop" => Entrypoint::Desktop,
            "local-agent" => Entrypoint::Cowork,
            _ => Entrypoint::Unknown,
        }
    }
}

/// How the context-window size was determined (priority order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CtxBasis {
    /// statusline `context_window.context_window_size` for the same session.
    Statusline,
    /// transcript identity attachment whose modelId ends with `[1m]`.
    Identity,
    /// Desktop Code-tab session metadata `model` ending with `[1m]`.
    DesktopModel,
    /// user override for this model id.
    Override,
    /// the size Claude Code's status line last reported for this model id (any session).
    Learned,
    /// a turn larger than 200k tokens was seen, so the window must be 1M.
    Heuristic,
    /// 200k default.
    Default,
}

/// The active Claude Code / Cowork session shown in the widget header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    /// Opaque, stable identifier for UI list keys (a hash of the session id; never the id itself).
    #[serde(default)]
    pub key: String,
    /// Model id without the `[1m]` suffix, e.g. `claude-opus-5-5`.
    pub model_id: Option<String>,
    /// Human name, e.g. `Opus 5.5`.
    pub display_name: Option<String>,
    /// 0..=100; `None` if unknown.
    pub ctx_pct: Option<f32>,
    pub ctx_tokens: Option<u64>,
    pub ctx_size: u64,
    pub ctx_basis: CtxBasis,
    /// True when computed from the transcript rather than reported by the statusline.
    pub ctx_is_estimate: bool,
    pub entrypoint: Entrypoint,
    pub last_active_ms: Ms,
    /// Folder name of the session's working directory (only the last path component).
    pub project: Option<String>,
    /// Number of sessions with activity in the last 10 minutes.
    pub concurrent: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DesktopHealth {
    NotFound,
    Ok {
        last_sample_ms: Option<Ms>,
        /// The file's version when it is newer than the one this build knows and was read
        /// best-effort (the UI says so instead of staying silent).
        #[serde(default)]
        newer_version: Option<u32>,
    },
    SchemaChanged {
        version: u32,
    },
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceHealth {
    pub desktop: DesktopHealth,
    /// Newest statusline capture (any session), if any.
    pub cli_last_capture_ms: Option<Ms>,
    /// Newest transcript activity seen, if any.
    pub transcripts_last_activity_ms: Option<Ms>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Warning {
    /// Desktop and CLI disagree persistently; they may be signed into different accounts.
    AccountMismatch,
    /// Claude Code is running but reports no plan limits (API-key user or non-Pro/Max plan).
    NoPlanLimits,
}

/// Everything the UI renders. Emitted as the `snapshot` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub generated_ms: Ms,
    /// FiveHour first, then SevenDay, then any others in key order.
    pub windows: Vec<WindowView>,
    pub session: Option<SessionView>,
    /// Every session with activity in the last [`crate::engine::snapshot::SESSIONS_WINDOW_MS`],
    /// newest first, at most [`crate::engine::snapshot::MAX_SESSIONS`]; includes `session`.
    #[serde(default)]
    pub sessions: Vec<SessionView>,
    pub health: SourceHealth,
    pub warnings: Vec<Warning>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_kind_serde_is_plain_string() {
        assert_eq!(serde_json::to_string(&WindowKind::FiveHour).unwrap(), "\"five_hour\"");
        let k: WindowKind = serde_json::from_str("\"seven_day_opus\"").unwrap();
        assert_eq!(k, WindowKind::Other("seven_day_opus".into()));
        assert_eq!(WindowKind::from_key("sd"), WindowKind::SevenDay);
        assert_eq!(WindowKind::from_short("5h"), WindowKind::FiveHour);
    }

    #[test]
    fn matches_short_agrees_with_from_short() {
        let kinds = [
            WindowKind::FiveHour,
            WindowKind::SevenDay,
            WindowKind::Other("seven_day_opus".into()),
            WindowKind::Other("seven_day_sonnet".into()),
            WindowKind::Other("x".into()),
            WindowKind::Other("fh".into()),
        ];
        for s in ["5h", "7d", "five_hour", "seven_day", "fh", "sd", "so", "sn", "seven_day_opus", "x", ""] {
            for k in &kinds {
                assert_eq!(k.matches_short(s), WindowKind::from_short(s) == *k, "{k:?} vs {s}");
            }
        }
    }

    #[test]
    fn reset_drop_is_two_points_inclusive() {
        assert!(is_reset_drop(80.0, 78.0));
        assert!(!is_reset_drop(80.2, 79.0));
        assert!(!is_reset_drop(80.0, 79.0));
        assert!(!is_reset_drop(10.0, 12.0));
    }

    #[test]
    fn reset_info_is_tagged() {
        let v = serde_json::to_value(ResetInfo::Estimated { at_ms: 5, plus_minus_ms: 1, confidence: Confidence::Low })
            .unwrap();
        assert_eq!(v["type"], "estimated");
        assert_eq!(v["confidence"], "low");
    }

    #[test]
    fn window_view_flattens_state() {
        let view = WindowView {
            state: WindowState {
                kind: WindowKind::FiveHour,
                pct: 22.0,
                reset: ResetInfo::Unknown,
                source: Source::Cli,
                observed_at_ms: 1,
                stale: false,
                limit_reached: false,
                phase: Phase::Active,
            },
            burn: None,
            spark: vec![],
            worked_since: false,
        };
        let v = serde_json::to_value(&view).unwrap();
        assert_eq!(v["kind"], "five_hour");
        assert_eq!(v["phase"], "active");
        assert_eq!(v["worked_since"], false);
        assert!(v.get("state").is_none());
        // Older serialised views (no marker) still load.
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("worked_since");
        let back: WindowView = serde_json::from_value(old).unwrap();
        assert_eq!(back, view);
    }
}
