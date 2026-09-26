//! Assembles a [`Snapshot`] from already-loaded source data. Pure: no file system access (the
//! [`History`] is an in-memory copy), `now_ms` passed in.
//!
//! Per window kind present in any source (FiveHour, SevenDay, then others in key order):
//! 1. `reset_estimate::estimate_reset` over the Desktop-sourced samples of that kind (Desktop
//!    file series ∪ Desktop rows of the history, samples more than [`FUTURE_SLACK_MS`] in the
//!    future dropped) with the last exact reset (`last_exact_resets`). A second estimate uses that
//!    reset only if it has passed: after an early reset, or once Desktop sees a later window, the
//!    live CLI reset belongs to the window that ended.
//! 2. `merge::merge_window_with` of the CLI observations and the newest Desktop observation, with
//!    both estimates.
//! 3. `burn::compute` over the history samples (∪ the Desktop series) of that kind.
//! 4. `History::spark`: 96 buckets over 24 h (five_hour) or 7 d (weekly kinds). The range end is
//!    aligned up to the bucket step so the buckets do not shift on every recompute (which would
//!    make every snapshot look changed).
//! 5. `worked_since`: the newest transcript assistant activity (any session) is at least
//!    [`WORKED_SINCE_MS`] newer than the value's `observed_at_ms` (a Desktop value or an old
//!    capture), so the real % is probably higher.
//!
//! Session: `active_session::pick` over the transcript tails; the capture with the same
//! `session_id` and the Desktop Code-tab session whose `cli_session_id` matches feed
//! `context::resolve`. Without any tail, the newest capture that names a model is shown instead.
//! The display name uses the learned names (`learned_names`).
//!
//! The caller owns learning: `last_exact_resets` and `learned_names` must already include what
//! [`learn_exact_resets`] and [`learn_model_names`] take from `captures` (the pipeline folds them
//! in whenever it reloads the captures), so a build does not repeat that work.
//! `project` only when `show_project`. `key` is [`session_key`] of the session id (an opaque FNV-1a
//! hash, never the id itself).
//!
//! Sessions: every tail with `last_assistant_ms >= now - SESSIONS_WINDOW_MS`, built the same way,
//! newest first (ties keep slice order), one entry per key, at most [`MAX_SESSIONS`]. The header
//! session is always listed as the identical value: it may be older than the window (`pick` has no
//! age cutoff) or tied out of the top entries by the focus tie-break, and then replaces the last
//! entry at its sorted position. The capture-only fallback session is the only entry.
//!
//! Warnings:
//! - `NoPlanLimits`: captures written within the last hour exist, but none carries rate limits.
//! - `AccountMismatch`: the pipeline tracks since when [`account_mismatch_now`] has held
//!   (`account_mismatch_since_ms`); after [`ACCOUNT_MISMATCH_AFTER_MS`] the warning is shown.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::capture::CaptureRecord;
use crate::engine::active_session;
use crate::engine::burn;
use crate::engine::context::{self, ContextInputs};
use crate::engine::merge;
use crate::engine::reset_estimate;
use crate::engine::types::{
    DesktopHealth, Entrypoint, Observation, Sample, SessionView, Snapshot, Source, SourceHealth,
    Warning, WindowKind, WindowView,
};
use crate::fingerprint::Fnv64;
use crate::history::History;
use crate::model_names::{display_name, split_1m};
use crate::sources::desktop_sessions::DesktopSession;
use crate::sources::desktop_usage::{self, DesktopUsage};
use crate::sources::statusline;
use crate::sources::transcript::TranscriptTail;
use crate::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms, SECOND_MS};

pub use crate::time::FUTURE_SLACK_MS;
/// Sparkline resolution.
pub const SPARK_BUCKETS: usize = 96;
/// Sparkline span of the five-hour window.
pub const SPARK_SPAN_FIVE_HOUR_MS: Ms = DAY_MS;
/// Sparkline span of weekly windows.
pub const SPARK_SPAN_WEEKLY_MS: Ms = 7 * DAY_MS;
/// Captures written within this long count as "Claude Code is running" for `NoPlanLimits`.
pub const NO_PLAN_LIMITS_RECENT_MS: Ms = HOUR_MS;
/// A Desktop/CLI disagreement larger than this many points counts as a mismatch.
pub const ACCOUNT_MISMATCH_PCT: f32 = 10.0;
/// Both measurements must be at most this far apart to be compared.
pub const ACCOUNT_MISMATCH_PAIR_MS: Ms = 20 * MINUTE_MS;
/// The mismatch must persist this long before it is shown.
pub const ACCOUNT_MISMATCH_AFTER_MS: Ms = 30 * MINUTE_MS;

/// Sessions active within this long appear in `Snapshot::sessions`.
pub const SESSIONS_WINDOW_MS: Ms = 12 * HOUR_MS;
/// At most this many sessions are listed.
pub const MAX_SESSIONS: usize = 8;

/// A window value this much older than the newest transcript activity is marked `worked_since`.
pub const WORKED_SINCE_MS: Ms = 90 * SECOND_MS;

/// Burn and reset estimation never look further back than this.
const SAMPLE_LOOKBACK_MS: Ms = 8 * DAY_MS;

/// Everything the engine needs, already loaded by the pipeline.
#[derive(Debug, Clone, Copy)]
pub struct EngineInputs<'a> {
    /// Statusline captures (any order).
    pub captures: &'a [CaptureRecord],
    /// Parsed Desktop usage file, if one was readable.
    pub desktop: Option<&'a DesktopUsage>,
    /// Health of the Desktop source, mapped by the caller from the load result.
    pub desktop_health: &'a DesktopHealth,
    /// In-memory history (already backfilled with the Desktop samples).
    pub history: &'a History,
    /// Transcript tails of recently active sessions.
    pub tails: &'a [TranscriptTail],
    pub desktop_sessions: &'a [DesktopSession],
    /// Newest exact reset per kind: the persisted value with `captures` already folded in by
    /// [`learn_exact_resets`].
    pub last_exact_resets: &'a BTreeMap<WindowKind, Ms>,
    /// Model display names by base id: the persisted map with `captures` already folded in by
    /// [`learn_model_names`].
    pub learned_names: &'a BTreeMap<String, String>,
    pub ctx_overrides: &'a BTreeMap<String, u64>,
    pub stale_after_ms: Ms,
    pub show_project: bool,
    /// Since when [`account_mismatch_now`] has been true without interruption.
    pub account_mismatch_since_ms: Option<Ms>,
}

/// Builds the snapshot (see the module docs).
pub fn build_snapshot(inputs: &EngineInputs<'_>, now_ms: Ms) -> Snapshot {
    let cli = statusline::observations(inputs.captures);
    let desktop_latest: Vec<Observation> = inputs
        .desktop
        .map(desktop_usage::latest_observations)
        .unwrap_or_default();

    let kinds: BTreeSet<WindowKind> = cli
        .iter()
        .chain(desktop_latest.iter())
        .map(|o| o.kind.clone())
        .collect();
    let exact = inputs.last_exact_resets;

    let newest_activity = inputs.tails.iter().map(|t| t.last_assistant_ms).max();
    let mut windows = Vec::new();
    for kind in &kinds {
        let desktop_series = desktop_samples(inputs, kind, now_ms);
        let last_exact = exact.get(kind).copied();
        let estimate = reset_estimate::estimate_reset(kind, &desktop_series, last_exact, now_ms);
        let past_exact = last_exact.filter(|&r| r <= now_ms);
        let desktop_estimate = if past_exact == last_exact {
            estimate.clone()
        } else {
            reset_estimate::estimate_reset(kind, &desktop_series, past_exact, now_ms)
        };
        let desktop_obs = desktop_latest.iter().find(|o| &o.kind == kind);
        let Some(state) = merge::merge_window_with(
            kind,
            &cli,
            desktop_obs,
            estimate,
            desktop_estimate,
            now_ms,
            inputs.stale_after_ms,
        ) else {
            continue;
        };
        let burn_samples = all_samples(inputs, kind, now_ms);
        let burn = burn::compute(kind, &burn_samples, &state, now_ms);
        let spark = spark(inputs.history, kind, now_ms);
        let worked_since = newest_activity
            .is_some_and(|newest| newest.saturating_sub(state.observed_at_ms) >= WORKED_SINCE_MS);
        windows.push(WindowView {
            state,
            burn,
            spark,
            worked_since,
        });
    }

    let (session, sessions) = session_views(inputs, now_ms);
    let health = SourceHealth {
        desktop: inputs.desktop_health.clone(),
        cli_last_capture_ms: statusline::last_capture_ms(inputs.captures),
        transcripts_last_activity_ms: newest_activity,
    };

    let mut warnings = Vec::new();
    if inputs
        .account_mismatch_since_ms
        .is_some_and(|since| now_ms.saturating_sub(since) > ACCOUNT_MISMATCH_AFTER_MS)
    {
        warnings.push(Warning::AccountMismatch);
    }
    if no_plan_limits(inputs.captures, now_ms) {
        warnings.push(Warning::NoPlanLimits);
    }

    Snapshot {
        generated_ms: now_ms,
        windows,
        session,
        sessions,
        health,
        warnings,
    }
}

/// True when, for some window, the newest Desktop observation is newer than the newest live CLI
/// value, both were measured within [`ACCOUNT_MISMATCH_PAIR_MS`] of each other and they differ by
/// more than [`ACCOUNT_MISMATCH_PCT`] points. (Requiring both to be recent keeps legitimate
/// claude.ai usage while Claude Code sits idle from looking like a mismatch.)
pub fn account_mismatch_now(captures: &[CaptureRecord], desktop: Option<&DesktopUsage>, now_ms: Ms) -> bool {
    let Some(desktop) = desktop else { return false };
    let cli = statusline::observations(captures);
    desktop_usage::latest_observations(desktop).iter().any(|d| {
        let live = cli
            .iter()
            .filter(|c| c.kind == d.kind && c.resets_at_ms.is_some_and(|r| r > now_ms));
        let Some(c) = live.max_by_key(|c| c.observed_at_ms) else { return false };
        d.observed_at_ms > c.observed_at_ms
            && d.observed_at_ms - c.observed_at_ms <= ACCOUNT_MISMATCH_PAIR_MS
            && (d.pct - c.pct).abs() > ACCOUNT_MISMATCH_PCT
    })
}

/// Folds the captures' `resets_at` into the persisted per-kind maximum. Returns whether `map`
/// changed.
pub fn learn_exact_resets(map: &mut BTreeMap<WindowKind, Ms>, captures: &[CaptureRecord]) -> bool {
    let mut changed = false;
    for obs in statusline::observations(captures) {
        let Some(r) = obs.resets_at_ms else { continue };
        let entry = map.entry(obs.kind).or_insert(Ms::MIN);
        if r > *entry {
            *entry = r;
            changed = true;
        }
    }
    changed
}

/// Folds the captures' `model.display_name` into `map` (keyed by base id). Returns whether `map`
/// changed.
pub fn learn_model_names(map: &mut BTreeMap<String, String>, captures: &[CaptureRecord]) -> bool {
    let mut changed = false;
    // Oldest first so the newest capture's name wins.
    let mut ordered: Vec<&CaptureRecord> = captures.iter().collect();
    ordered.sort_by_key(|c| c.changed_at_ms);
    for cap in ordered {
        let Some(model) = &cap.model else { continue };
        let (Some(id), Some(name)) = (model.id.as_deref(), model.display_name.as_deref()) else {
            continue;
        };
        let base = split_1m(id.trim()).0;
        let name = name.trim();
        if base.is_empty() || name.is_empty() || name.len() > 64 {
            continue;
        }
        if map.get(base).map(String::as_str) != Some(name) {
            map.insert(base.to_owned(), name.to_owned());
            changed = true;
        }
    }
    changed
}

/// Sparkline range `(from, to)` for a kind: `to` is `now` rounded up to the bucket step.
pub fn spark_range(kind: &WindowKind, now_ms: Ms) -> (Ms, Ms) {
    let span = if *kind == WindowKind::FiveHour {
        SPARK_SPAN_FIVE_HOUR_MS
    } else {
        SPARK_SPAN_WEEKLY_MS
    };
    let step = span / SPARK_BUCKETS as Ms;
    let to = (now_ms.div_euclid(step) + 1).saturating_mul(step);
    (to - span, to)
}

fn spark(history: &History, kind: &WindowKind, now_ms: Ms) -> Vec<crate::engine::types::SparkPoint> {
    let (from, to) = spark_range(kind, now_ms);
    history.spark(kind, from, to, SPARK_BUCKETS)
}

/// Desktop samples of `kind`: the file's series ∪ Desktop rows of the history.
fn desktop_samples(inputs: &EngineInputs<'_>, kind: &WindowKind, now_ms: Ms) -> Vec<Sample> {
    let since = now_ms.saturating_sub(SAMPLE_LOOKBACK_MS);
    let rows = inputs.history.rows();
    let from_history = rows[rows.partition_point(|r| r.t < since)..]
        .iter()
        .filter(|r| r.s == Source::Desktop && kind.matches_short(&r.w))
        .map(|r| Sample { t_ms: r.t, pct: r.p });
    combine(from_history, desktop_series(inputs, kind), since, now_ms)
}

/// All samples of `kind`: the history (any source) ∪ the Desktop file's series.
fn all_samples(inputs: &EngineInputs<'_>, kind: &WindowKind, now_ms: Ms) -> Vec<Sample> {
    let since = now_ms.saturating_sub(SAMPLE_LOOKBACK_MS);
    combine(
        inputs.history.samples(kind, since).into_iter(),
        desktop_series(inputs, kind),
        since,
        now_ms,
    )
}

fn desktop_series<'a>(inputs: &EngineInputs<'a>, kind: &WindowKind) -> &'a [Sample] {
    inputs
        .desktop
        .and_then(|d| d.series.get(kind))
        .map_or(&[], Vec::as_slice)
}

/// Merges two sample sources, keeps `since <= t <= now + FUTURE_SLACK_MS`, sorts and removes
/// duplicate timestamps (the Desktop file's value wins, it is the original).
fn combine(
    first: impl Iterator<Item = Sample>,
    second: &[Sample],
    since: Ms,
    now_ms: Ms,
) -> Vec<Sample> {
    let limit = now_ms.saturating_add(FUTURE_SLACK_MS);
    let mut out: Vec<Sample> = first
        .chain(second.iter().copied())
        .filter(|s| s.t_ms >= since && s.t_ms <= limit && s.pct.is_finite())
        .collect();
    // Stable: for equal t the second source (Desktop file) comes last and wins the dedup below.
    out.sort_by_key(|s| s.t_ms);
    out.dedup_by(|later, kept| {
        let same = later.t_ms == kept.t_ms;
        if same {
            *kept = *later;
        }
        same
    });
    out
}

/// Opaque, stable list key for a session: the FNV-1a 64-bit hash of its id as 16 hex digits.
pub fn session_key(session_id: &str) -> String {
    format!("{:016x}", Fnv64::new().write(session_id.as_bytes()).finish())
}

/// A tail's key; a tail without a session id is keyed by its file path instead.
fn tail_key(tail: &TranscriptTail) -> String {
    if tail.session_id.is_empty() {
        session_key(&tail.path.to_string_lossy())
    } else {
        session_key(&tail.session_id)
    }
}

/// The header session and the recent-sessions list (see the module docs).
fn session_views(inputs: &EngineInputs<'_>, now_ms: Ms) -> (Option<SessionView>, Vec<SessionView>) {
    let learned = inputs.learned_names;

    let Some(pick) = active_session::pick(inputs.tails, inputs.desktop_sessions, now_ms) else {
        let session = capture_only_session(inputs, learned);
        let sessions = session.iter().cloned().collect();
        return (session, sessions);
    };
    let active = tail_view(inputs, &inputs.tails[pick.index], learned, pick.concurrent);

    let floor = now_ms.saturating_sub(SESSIONS_WINDOW_MS);
    let mut recent: Vec<&TranscriptTail> = inputs
        .tails
        .iter()
        .filter(|t| t.last_assistant_ms >= floor)
        .collect();
    // Stable, so equal timestamps keep slice order (as `active_session::pick` does).
    recent.sort_by_key(|t| std::cmp::Reverse(t.last_assistant_ms));

    let mut seen = HashSet::new();
    let mut sessions: Vec<SessionView> = Vec::with_capacity(MAX_SESSIONS);
    for tail in recent {
        if sessions.len() == MAX_SESSIONS {
            break;
        }
        let key = tail_key(tail);
        if !seen.insert(key.clone()) {
            continue;
        }
        sessions.push(if key == active.key {
            active.clone()
        } else {
            tail_view(inputs, tail, learned, pick.concurrent)
        });
    }
    if !seen.contains(&active.key) {
        if sessions.len() == MAX_SESSIONS {
            sessions.pop();
        }
        let at = sessions.partition_point(|s| s.last_active_ms >= active.last_active_ms);
        sessions.insert(at, active.clone());
    }
    (Some(active), sessions)
}

/// A session built from its transcript tail, the capture with the same `session_id` and the
/// Desktop Code-tab session whose `cli_session_id` matches.
fn tail_view(
    inputs: &EngineInputs<'_>,
    tail: &TranscriptTail,
    learned: &BTreeMap<String, String>,
    concurrent: u8,
) -> SessionView {
    let name_of = |id: &str| Some(display_name(id, learned)).filter(|n| !n.is_empty());
    let capture = inputs
        .captures
        .iter()
        .filter(|c| c.session_id == tail.session_id)
        .max_by_key(|c| c.changed_at_ms);
    let desktop_session = inputs
        .desktop_sessions
        .iter()
        .find(|d| d.cli_session_id.as_deref() == Some(tail.session_id.as_str()));
    let ctx = context::resolve(&ContextInputs {
        tail: Some(tail),
        capture,
        desktop_session,
        overrides: inputs.ctx_overrides,
    });
    let raw_id = tail
        .model_id
        .as_deref()
        .or_else(|| capture.and_then(|c| c.model.as_ref()?.id.as_deref()))
        .or_else(|| desktop_session.and_then(|d| d.model.as_deref()));
    SessionView {
        key: tail_key(tail),
        model_id: raw_id.map(|id| split_1m(id.trim()).0.to_owned()).filter(|s| !s.is_empty()),
        display_name: raw_id.and_then(name_of),
        ctx_pct: ctx.pct,
        ctx_tokens: ctx.tokens,
        ctx_size: ctx.size,
        ctx_basis: ctx.basis,
        ctx_is_estimate: ctx.is_estimate,
        entrypoint: tail.entrypoint,
        last_active_ms: tail.last_assistant_ms,
        project: if inputs.show_project { tail.project.clone() } else { None },
        concurrent,
    }
}

/// No transcript found (e.g. `CLAUDE_CONFIG_DIR` points elsewhere): show the newest capture that
/// names a model.
fn capture_only_session(inputs: &EngineInputs<'_>, learned: &BTreeMap<String, String>) -> Option<SessionView> {
    let capture = inputs
        .captures
        .iter()
        .filter(|c| c.model.as_ref().is_some_and(|m| m.id.is_some()))
        .max_by_key(|c| c.changed_at_ms)?;
    let raw_id = capture.model.as_ref()?.id.as_deref()?;
    let ctx = context::resolve(&ContextInputs {
        tail: None,
        capture: Some(capture),
        desktop_session: None,
        overrides: inputs.ctx_overrides,
    });
    Some(SessionView {
        key: session_key(&capture.session_id),
        model_id: Some(split_1m(raw_id.trim()).0.to_owned()),
        display_name: Some(display_name(raw_id, learned)).filter(|n| !n.is_empty()),
        ctx_pct: ctx.pct,
        ctx_tokens: ctx.tokens,
        ctx_size: ctx.size,
        ctx_basis: ctx.basis,
        ctx_is_estimate: ctx.is_estimate,
        entrypoint: Entrypoint::Cli,
        last_active_ms: capture.changed_at_ms,
        project: None,
        concurrent: 1,
    })
}

fn no_plan_limits(captures: &[CaptureRecord], now_ms: Ms) -> bool {
    let floor = now_ms.saturating_sub(NO_PLAN_LIMITS_RECENT_MS);
    let mut recent = captures
        .iter()
        .filter(|c| c.written_at_ms.max(c.changed_at_ms) >= floor)
        .peekable();
    recent.peek().is_some() && recent.all(|c| c.rate_limits.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureRecord, CtxInfo, ModelInfo, RateLimit};
    use crate::engine::types::{CtxBasis, Phase, ResetInfo};
    use crate::history::History;
    use crate::sources::desktop_usage::synth;
    use crate::time::SECOND_MS;
    use crate::turns::TurnInfo;
    use pretty_assertions::assert_eq;

    const NOW: Ms = 1_790_000_000_000;

    struct Fixture {
        _tmp: tempfile::TempDir,
        history: History,
        health: DesktopHealth,
        exact: BTreeMap<WindowKind, Ms>,
        names: BTreeMap<String, String>,
        overrides: BTreeMap<String, u64>,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let history = History::open(tmp.path().join("history.jsonl")).unwrap();
            Self {
                _tmp: tmp,
                history,
                health: DesktopHealth::Ok { last_sample_ms: None },
                exact: BTreeMap::new(),
                names: BTreeMap::new(),
                overrides: BTreeMap::new(),
            }
        }

        fn inputs<'a>(
            &'a self,
            captures: &'a [CaptureRecord],
            desktop: Option<&'a DesktopUsage>,
            tails: &'a [TranscriptTail],
        ) -> EngineInputs<'a> {
            EngineInputs {
                captures,
                desktop,
                desktop_health: &self.health,
                history: &self.history,
                tails,
                desktop_sessions: &[],
                last_exact_resets: &self.exact,
                learned_names: &self.names,
                ctx_overrides: &self.overrides,
                stale_after_ms: 20 * MINUTE_MS,
                show_project: false,
                account_mismatch_since_ms: None,
            }
        }
    }

    fn capture(session: &str, changed: Ms, limits: &[(&str, f32, Ms)]) -> CaptureRecord {
        CaptureRecord {
            v: 1,
            session_id: session.into(),
            written_at_ms: changed,
            changed_at_ms: changed,
            fingerprint: 0,
            transcript_path: None,
            model: Some(ModelInfo {
                id: Some("claude-opus-5-5[1m]".into()),
                display_name: Some("Opus 5.5 (1M context)".into()),
            }),
            context: Some(CtxInfo {
                used_percentage: Some(12.0),
                context_window_size: Some(1_000_000),
                exceeds_200k: Some(false),
            }),
            rate_limits: limits
                .iter()
                .map(|&(k, p, reset_ms)| {
                    (
                        k.to_string(),
                        RateLimit {
                            used_percentage: p,
                            resets_at: reset_ms / 1000,
                        },
                    )
                })
                .collect(),
            api_ms: None,
            cc_version: None,
        }
    }

    fn tail(session: &str, last_ms: Ms) -> TranscriptTail {
        TranscriptTail {
            path: format!("C:/x/{session}.jsonl").into(),
            session_id: session.into(),
            entrypoint: Entrypoint::Cli,
            model_id: Some("claude-sonnet-5".into()),
            ctx_tokens: 50_000,
            max_ctx_tokens_seen: 50_000,
            identity_1m: None,
            last_assistant_ms: last_ms,
            project: Some("secret-project".into()),
            turn: TurnInfo::default(),
        }
    }

    fn desktop(fh: &[(Ms, f32)], sd: &[(Ms, f32)]) -> DesktopUsage {
        let mut series = BTreeMap::new();
        let to = |v: &[(Ms, f32)]| v.iter().map(|&(t_ms, pct)| Sample { t_ms, pct }).collect::<Vec<_>>();
        if !fh.is_empty() {
            series.insert(WindowKind::FiveHour, to(fh));
        }
        if !sd.is_empty() {
            series.insert(WindowKind::SevenDay, to(sd));
        }
        let last = series.values().filter_map(|s| s.last()).map(|s| s.t_ms).max();
        DesktopUsage {
            version: 2,
            series,
            last_sample_ms: last,
        }
    }

    #[test]
    fn empty_inputs_give_empty_snapshot() {
        let f = Fixture::new();
        let s = build_snapshot(&f.inputs(&[], None, &[]), NOW);
        assert!(s.windows.is_empty());
        assert!(s.session.is_none());
        assert!(s.warnings.is_empty());
        assert_eq!(s.health.cli_last_capture_ms, None);
    }

    #[test]
    fn cli_windows_are_exact_and_ordered() {
        let f = Fixture::new();
        let caps = [capture(
            "s1",
            NOW - MINUTE_MS,
            &[
                ("seven_day", 61.0, NOW + 3 * DAY_MS),
                ("five_hour", 22.0, NOW + 2 * HOUR_MS),
                ("seven_day_opus", 5.0, NOW + 3 * DAY_MS),
            ],
        )];
        let s = build_snapshot(&f.inputs(&caps, None, &[]), NOW);
        let kinds: Vec<&str> = s.windows.iter().map(|w| w.state.kind.key()).collect();
        assert_eq!(kinds, vec!["five_hour", "seven_day", "seven_day_opus"]);
        let fh = &s.windows[0];
        assert_eq!(fh.state.pct, 22.0);
        assert_eq!(fh.state.source, Source::Cli);
        assert_eq!(fh.state.reset, ResetInfo::Exact { at_ms: NOW + 2 * HOUR_MS });
        assert_eq!(fh.spark.len(), SPARK_BUCKETS);
        assert_eq!(s.health.cli_last_capture_ms, Some(NOW - MINUTE_MS));
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn desktop_only_windows_get_estimates() {
        let f = Fixture::new();
        // 5h: reset seen as a drop 73 -> 13 two hours ago, rising since.
        let t0 = NOW - 4 * HOUR_MS;
        let fh: Vec<(Ms, f32)> = (0..16)
            .map(|i| {
                let t = t0 + i * 15 * MINUTE_MS;
                (t, if i < 8 { 73.0 } else { 13.0 + (i - 8) as f32 })
            })
            .collect();
        let d = desktop(&fh, &[(NOW - 10 * MINUTE_MS, 40.0)]);
        let s = build_snapshot(&f.inputs(&[], Some(&d), &[]), NOW);
        assert_eq!(s.windows.len(), 2);
        let w = &s.windows[0].state;
        assert_eq!(w.source, Source::Desktop);
        assert_eq!(w.pct, 20.0);
        match w.reset {
            ResetInfo::Estimated { at_ms, .. } => {
                let start = t0 + 8 * 15 * MINUTE_MS;
                assert!(at_ms > start && at_ms <= start + 5 * HOUR_MS, "{at_ms}");
            }
            ref other => panic!("expected estimate, got {other:?}"),
        }
    }

    #[test]
    fn future_samples_are_ignored_for_estimates() {
        let f = Fixture::new();
        // A corrupt sample far in the future must not become the "newest" of the estimate.
        let d_ok = desktop(&[(NOW - 30 * MINUTE_MS, 10.0), (NOW - 15 * MINUTE_MS, 12.0)], &[]);
        let mut d_bad = d_ok.clone();
        d_bad
            .series
            .get_mut(&WindowKind::FiveHour)
            .unwrap()
            .push(Sample { t_ms: NOW + 3 * DAY_MS, pct: 0.0 });
        let ok = &f.inputs(&[], Some(&d_ok), &[]);
        let mut expected = build_snapshot(ok, NOW).windows[0].state.reset.clone();
        // Merge takes the newest Desktop value (the bad one) — but the estimate stays the same.
        let bad = build_snapshot(&f.inputs(&[], Some(&d_bad), &[]), NOW);
        let got = bad.windows[0].state.reset.clone();
        if let (ResetInfo::Estimated { at_ms: a, .. }, ResetInfo::Estimated { at_ms: b, .. }) = (&mut expected, &got) {
            assert_eq!(*a, *b);
        } else {
            assert_eq!(expected, got);
        }
    }

    #[test]
    fn persisted_exact_reset_is_used_for_desktop_values() {
        let mut f = Fixture::new();
        f.exact.insert(WindowKind::FiveHour, NOW + HOUR_MS);
        let d = desktop(&[(NOW - 5 * MINUTE_MS, 30.0)], &[]);
        let s = build_snapshot(&f.inputs(&[], Some(&d), &[]), NOW);
        assert_eq!(s.windows[0].state.reset, ResetInfo::Exact { at_ms: NOW + HOUR_MS });
    }

    #[test]
    fn newer_agreeing_desktop_keeps_cli_reset() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - 30 * MINUTE_MS, &[("five_hour", 40.2, NOW + HOUR_MS)])];
        let d = desktop(&[(NOW - 5 * MINUTE_MS, 41.0)], &[]);
        let s = build_snapshot(&f.inputs(&caps, Some(&d), &[]), NOW);
        let w = &s.windows[0].state;
        assert_eq!(w.source, Source::Desktop);
        assert_eq!(w.pct, 41.0);
        assert_eq!(w.reset, ResetInfo::Exact { at_ms: NOW + HOUR_MS });
    }

    #[test]
    fn early_reset_seen_by_desktop_gets_an_estimate_not_the_old_cli_reset() {
        let mut f = Fixture::new();
        // The CLI still reports the old window (60 %, resets in 2 h); Desktop saw it reset since.
        let caps = [capture("s1", NOW - 30 * MINUTE_MS, &[("five_hour", 60.0, NOW + 2 * HOUR_MS)])];
        // As the pipeline does: the capture's reset time is learned before the build.
        assert!(learn_exact_resets(&mut f.exact, &caps));
        let d = desktop(&[(NOW - HOUR_MS, 58.0), (NOW - 5 * MINUTE_MS, 12.0)], &[]);
        let s = build_snapshot(&f.inputs(&caps, Some(&d), &[]), NOW);
        let w = &s.windows[0].state;
        assert_eq!((w.pct, w.source), (12.0, Source::Desktop));
        match w.reset {
            ResetInfo::Estimated { at_ms, .. } => {
                assert!(at_ms > NOW - HOUR_MS + 5 * HOUR_MS - MINUTE_MS && at_ms <= NOW + 5 * HOUR_MS, "{at_ms}");
            }
            ref other => panic!("expected an estimate, got {other:?}"),
        }
    }

    #[test]
    fn expired_cli_only_awaits_data() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - 6 * HOUR_MS, &[("five_hour", 90.0, NOW - HOUR_MS)])];
        let s = build_snapshot(&f.inputs(&caps, None, &[]), NOW);
        assert_eq!(s.windows[0].state.phase, Phase::ResetAwaitingData);
        assert_eq!(s.windows[0].state.pct, 0.0);
    }

    #[test]
    fn burn_uses_history() {
        let mut f = Fixture::new();
        // Desktop series climbing 1 point every 3 minutes over the last hour.
        let fh: Vec<(Ms, f32)> = (0..=20)
            .map(|i| (NOW - HOUR_MS + i * 3 * MINUTE_MS, 20.0 + i as f32))
            .collect();
        let d = desktop(&fh, &[]);
        f.history.backfill_desktop(&d, 0).unwrap();
        f.exact.insert(WindowKind::FiveHour, NOW + 4 * HOUR_MS);
        let s = build_snapshot(&f.inputs(&[], Some(&d), &[]), NOW);
        let burn = s.windows[0].burn.as_ref().expect("burn");
        assert!((burn.slope_pct_per_h - 20.0).abs() < 2.0, "{}", burn.slope_pct_per_h);
        assert!(burn.hits_limit_before_reset);
        assert!(s.windows[0].spark.iter().any(|p| p.pct.is_some()));
    }

    #[test]
    fn spark_range_is_stable_within_a_step() {
        let step = DAY_MS / 96;
        let base = 1_790_000_000_000 / step * step;
        let a = spark_range(&WindowKind::FiveHour, base + 1);
        let b = spark_range(&WindowKind::FiveHour, base + step - 1);
        assert_eq!(a, b);
        assert!(a.1 > base && a.1 - a.0 == DAY_MS);
        let (from, to) = spark_range(&WindowKind::SevenDay, base);
        assert_eq!(to - from, 7 * DAY_MS);
        assert!(to > base);
    }

    #[test]
    fn same_inputs_give_equal_snapshots_apart_from_time() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - MINUTE_MS, &[("five_hour", 22.0, NOW + 2 * HOUR_MS)])];
        let a = build_snapshot(&f.inputs(&caps, None, &[]), NOW);
        let mut b = build_snapshot(&f.inputs(&caps, None, &[]), NOW + 1_000);
        b.generated_ms = a.generated_ms;
        assert_eq!(a, b);
    }

    #[test]
    fn session_from_tail_with_matching_capture() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - MINUTE_MS, &[("five_hour", 22.0, NOW + HOUR_MS)])];
        let tails = [tail("s1", NOW - MINUTE_MS), tail("s2", NOW - HOUR_MS)];
        let s = build_snapshot(&f.inputs(&caps, None, &tails), NOW);
        let session = s.session.expect("session");
        assert_eq!(session.model_id.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(session.display_name.as_deref(), Some("Sonnet 5"));
        assert_eq!(session.ctx_basis, CtxBasis::Statusline);
        assert_eq!(session.ctx_size, 1_000_000);
        assert_eq!(session.ctx_pct, Some(12.0));
        assert!(!session.ctx_is_estimate);
        assert_eq!(session.project, None, "project hidden unless show_project");
        assert_eq!(session.concurrent, 1);
        assert_eq!(s.health.transcripts_last_activity_ms, Some(NOW - MINUTE_MS));
    }

    #[test]
    fn learned_names_come_from_captures() {
        let mut f = Fixture::new();
        let caps = [capture("s1", NOW - MINUTE_MS, &[])];
        let mut t = tail("s1", NOW - MINUTE_MS);
        t.model_id = Some("claude-opus-5-5".into());
        assert!(learn_model_names(&mut f.names, &caps));
        let s = build_snapshot(&f.inputs(&caps, None, &[t.clone()]), NOW);
        assert_eq!(s.session.unwrap().display_name.as_deref(), Some("Opus 5.5"));
        // The build uses the learned map it is given; it does not learn from the captures itself.
        f.names.insert("claude-opus-5-5".into(), "Custom".into());
        let s = build_snapshot(&f.inputs(&caps, None, &[t]), NOW);
        assert_eq!(s.session.unwrap().display_name.as_deref(), Some("Custom"));

        let mut map = BTreeMap::new();
        assert!(learn_model_names(&mut map, &caps));
        assert_eq!(map.get("claude-opus-5-5").map(String::as_str), Some("Opus 5.5 (1M context)"));
        assert!(!learn_model_names(&mut map, &caps));
    }

    #[test]
    fn session_shows_project_when_enabled_and_estimates_without_capture() {
        let f = Fixture::new();
        let tails = [tail("s9", NOW - MINUTE_MS)];
        let mut inputs = f.inputs(&[], None, &tails);
        inputs.show_project = true;
        let session = build_snapshot(&inputs, NOW).session.unwrap();
        assert_eq!(session.project.as_deref(), Some("secret-project"));
        assert!(session.ctx_is_estimate);
        assert_eq!(session.ctx_size, 200_000);
        assert_eq!(session.ctx_pct, Some(25.0));
    }

    #[test]
    fn capture_only_session_without_tails() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - MINUTE_MS, &[("five_hour", 1.0, NOW + HOUR_MS)])];
        let session = build_snapshot(&f.inputs(&caps, None, &[]), NOW).session.unwrap();
        assert_eq!(session.model_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(session.display_name.as_deref(), Some("Opus 5.5"));
        assert_eq!(session.ctx_pct, Some(12.0));
    }

    #[test]
    fn session_key_is_opaque_stable_fnv_hex() {
        // FNV-1a 64 of "a" (see fingerprint.rs known vectors).
        assert_eq!(session_key("a"), "af63dc4c8601ec8c");
        let id = "aaaaaaaa-0000-4000-8000-000000000001";
        let key = session_key(id);
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!key.contains("aaaaaaaa"), "never the raw id");
        assert_eq!(key, session_key(id));
        assert_ne!(key, session_key("aaaaaaaa-0000-4000-8000-000000000002"));
    }

    #[test]
    fn sessions_list_recent_tails_newest_first_capped() {
        let f = Fixture::new();
        // Ten sessions within the window (s0 newest) plus one just outside it.
        let mut tails: Vec<TranscriptTail> = (0..10)
            .map(|i| tail(&format!("s{i}"), NOW - i * HOUR_MS))
            .collect();
        tails.push(tail("old", NOW - SESSIONS_WINDOW_MS - 1));
        tails.rotate_left(3); // slice order must not matter
        let s = build_snapshot(&f.inputs(&[], None, &tails), NOW);
        let expected: Vec<String> = (0..MAX_SESSIONS).map(|i| session_key(&format!("s{i}"))).collect();
        let keys: Vec<&str> = s.sessions.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, expected);
        assert_eq!(s.session.as_ref(), s.sessions.first(), "header session is the listed one");
        assert!(s.sessions.windows(2).all(|w| w[0].last_active_ms >= w[1].last_active_ms));
        // The window boundary is inclusive.
        let edge = [tail("edge", NOW - SESSIONS_WINDOW_MS), tail("new", NOW)];
        assert_eq!(build_snapshot(&f.inputs(&[], None, &edge), NOW).sessions.len(), 2);
    }

    #[test]
    fn sessions_are_built_like_the_header_session() {
        let f = Fixture::new();
        let caps = [capture("s2", NOW - 2 * HOUR_MS, &[])];
        let mut other = tail("s2", NOW - 2 * HOUR_MS);
        other.entrypoint = Entrypoint::Cowork;
        other.project = Some("other-project".into());
        let tails = [tail("s1", NOW - MINUTE_MS), other];
        let mut inputs = f.inputs(&caps, None, &tails);
        inputs.show_project = true;
        let s = build_snapshot(&inputs, NOW);
        assert_eq!(s.sessions.len(), 2);
        let [first, second] = [&s.sessions[0], &s.sessions[1]];
        assert_eq!(first.key, session_key("s1"));
        assert!(first.ctx_is_estimate, "s1 has no capture");
        // s2 uses its own capture's context and keeps its entrypoint/project.
        assert_eq!(second.key, session_key("s2"));
        assert_eq!((second.ctx_pct, second.ctx_size), (Some(12.0), 1_000_000));
        assert_eq!(second.ctx_basis, CtxBasis::Statusline);
        assert_eq!(second.entrypoint, Entrypoint::Cowork);
        assert_eq!(second.project.as_deref(), Some("other-project"));
        assert_eq!(second.last_active_ms, NOW - 2 * HOUR_MS);
        assert_eq!(first.concurrent, second.concurrent, "concurrent is a global count");

        inputs.show_project = false;
        let hidden = build_snapshot(&inputs, NOW);
        assert!(hidden.sessions.iter().all(|v| v.project.is_none()));
    }

    #[test]
    fn duplicate_session_ids_are_listed_once() {
        let f = Fixture::new();
        let mut copy = tail("s1", NOW - HOUR_MS);
        copy.path = "C:/x/cowork/s1.jsonl".into();
        let tails = [copy, tail("s1", NOW - MINUTE_MS), tail("s2", NOW - 2 * HOUR_MS)];
        let s = build_snapshot(&f.inputs(&[], None, &tails), NOW);
        let keys: Vec<&str> = s.sessions.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, vec![session_key("s1"), session_key("s2")]);
        assert_eq!(s.sessions[0].last_active_ms, NOW - MINUTE_MS, "newest copy wins");
        assert_eq!(s.session.as_ref(), Some(&s.sessions[0]));
    }

    #[test]
    fn header_session_is_listed_even_when_old() {
        let f = Fixture::new();
        let tails = [tail("s1", NOW - 2 * DAY_MS)];
        let s = build_snapshot(&f.inputs(&[], None, &tails), NOW);
        let session = s.session.expect("pick has no age cutoff");
        assert_eq!(s.sessions, vec![session]);
    }

    #[test]
    fn focused_header_session_replaces_the_last_entry() {
        let f = Fixture::new();
        // Nine sessions within the two-minute focus tie; the focused one is the oldest, so the
        // newest-first cut would drop it.
        let tails: Vec<TranscriptTail> = (0..9)
            .map(|i| tail(&format!("s{i}"), NOW - i * 10 * SECOND_MS))
            .collect();
        let desk = [DesktopSession {
            cli_session_id: Some("s8".into()),
            model: None,
            last_focused_ms: Some(NOW),
            last_activity_ms: Some(NOW),
        }];
        let mut inputs = f.inputs(&[], None, &tails);
        inputs.desktop_sessions = &desk;
        let s = build_snapshot(&inputs, NOW);
        let session = s.session.expect("session");
        assert_eq!(session.key, session_key("s8"));
        assert_eq!(s.sessions.len(), MAX_SESSIONS);
        assert_eq!(s.sessions.last(), Some(&session));
        assert!(!s.sessions.iter().any(|v| v.key == session_key("s7")), "s7 made room");
        assert_eq!(s.sessions.iter().filter(|v| v.key == session.key).count(), 1);
    }

    #[test]
    fn capture_only_session_is_keyed_and_listed() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - MINUTE_MS, &[("five_hour", 1.0, NOW + HOUR_MS)])];
        let s = build_snapshot(&f.inputs(&caps, None, &[]), NOW);
        let session = s.session.expect("session");
        assert_eq!(session.key, session_key("s1"));
        assert_eq!(s.sessions, vec![session]);
        assert!(build_snapshot(&f.inputs(&[], None, &[]), NOW).sessions.is_empty());
    }

    #[test]
    fn tail_without_session_id_is_keyed_by_path() {
        let f = Fixture::new();
        let tails = [tail("", NOW - MINUTE_MS)];
        let s = build_snapshot(&f.inputs(&[], None, &tails), NOW);
        assert_eq!(s.sessions[0].key, session_key("C:/x/.jsonl"));
        assert_ne!(s.sessions[0].key, session_key(""));
    }

    #[test]
    fn no_plan_limits_warning() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - 10 * MINUTE_MS, &[])];
        let s = build_snapshot(&f.inputs(&caps, None, &[]), NOW);
        assert_eq!(s.warnings, vec![Warning::NoPlanLimits]);
        // Old captures do not count.
        let old = [capture("s1", NOW - 2 * HOUR_MS, &[])];
        assert!(build_snapshot(&f.inputs(&old, None, &[]), NOW).warnings.is_empty());
        // One capture with limits clears it.
        let mixed = [
            capture("s1", NOW - 10 * MINUTE_MS, &[]),
            capture("s2", NOW - 5 * MINUTE_MS, &[("five_hour", 1.0, NOW + HOUR_MS)]),
        ];
        assert!(build_snapshot(&f.inputs(&mixed, None, &[]), NOW).warnings.is_empty());
    }

    #[test]
    fn account_mismatch_detection_and_warning() {
        let f = Fixture::new();
        let caps = [capture("s1", NOW - 10 * MINUTE_MS, &[("five_hour", 20.0, NOW + HOUR_MS)])];
        let far = desktop(&[(NOW - 5 * MINUTE_MS, 45.0)], &[]);
        let close = desktop(&[(NOW - 5 * MINUTE_MS, 25.0)], &[]);
        let stale_cli = [capture("s1", NOW - 2 * HOUR_MS, &[("five_hour", 20.0, NOW + HOUR_MS)])];
        assert!(account_mismatch_now(&caps, Some(&far), NOW));
        assert!(!account_mismatch_now(&caps, Some(&close), NOW));
        assert!(!account_mismatch_now(&stale_cli, Some(&far), NOW), "idle CLI is not a mismatch");
        assert!(!account_mismatch_now(&caps, None, NOW));

        let mut inputs = f.inputs(&caps, Some(&far), &[]);
        inputs.account_mismatch_since_ms = Some(NOW - 10 * MINUTE_MS);
        assert!(build_snapshot(&inputs, NOW).warnings.is_empty());
        inputs.account_mismatch_since_ms = Some(NOW - 31 * MINUTE_MS);
        assert_eq!(build_snapshot(&inputs, NOW).warnings, vec![Warning::AccountMismatch]);
    }

    #[test]
    fn learn_exact_resets_keeps_maximum() {
        let mut map = BTreeMap::new();
        let caps = [
            capture("a", NOW, &[("five_hour", 1.0, NOW + HOUR_MS)]),
            capture("b", NOW, &[("five_hour", 1.0, NOW + 2 * HOUR_MS)]),
        ];
        assert!(learn_exact_resets(&mut map, &caps));
        assert_eq!(map.get(&WindowKind::FiveHour), Some(&(NOW + 2 * HOUR_MS / 1000 * 1000)));
        assert!(!learn_exact_resets(&mut map, &caps));
    }

    #[test]
    fn realistic_desktop_history_builds_sane_snapshot() {
        let f = Fixture::new();
        let file = synth::realistic();
        let d = desktop_usage::parse(file.json.as_bytes()).unwrap();
        let now = d.last_sample_ms.unwrap() + 5 * MINUTE_MS;
        let s = build_snapshot(&f.inputs(&[], Some(&d), &[]), now);
        assert!(s.windows.len() >= 2);
        for w in &s.windows {
            assert!((0.0..=100.0).contains(&w.state.pct));
            assert_eq!(w.spark.len(), SPARK_BUCKETS);
            assert!(!w.state.stale);
        }
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains(synth::ORG_A) && !json.contains(synth::ORG_B), "org never serialised");
    }

    #[test]
    fn worked_since_marks_values_older_than_transcript_activity() {
        let f = Fixture::new();
        let seen = NOW - 10 * MINUTE_MS;
        let caps = [capture("s1", seen, &[("five_hour", 22.0, NOW + HOUR_MS)])];
        let d = desktop(&[], &[(seen - HOUR_MS, 40.0)]);
        // [five_hour (CLI, seen), seven_day (Desktop, an hour older)].
        let marked = |tails: &[TranscriptTail]| -> Vec<bool> {
            let s = build_snapshot(&f.inputs(&caps, Some(&d), tails), NOW);
            assert_eq!(s.windows.len(), 2);
            s.windows.iter().map(|w| w.worked_since).collect()
        };
        assert_eq!(marked(&[]), vec![false, false], "no transcripts");
        let just_under = [tail("s1", seen + WORKED_SINCE_MS - 1)];
        assert_eq!(marked(&just_under), vec![false, true]);
        // The newest tail of any session counts.
        let at = [tail("old", seen - HOUR_MS), tail("s2", seen + WORKED_SINCE_MS)];
        assert_eq!(marked(&at), vec![true, true]);
        // Activity older than the value: nothing to add.
        let older = [tail("s1", seen - 2 * HOUR_MS)];
        assert_eq!(marked(&older), vec![false, false]);
    }
}
