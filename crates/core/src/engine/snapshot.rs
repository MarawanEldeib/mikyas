//! Assembles a [`Snapshot`] from already-loaded source data. Pure: no file system access (the
//! [`History`] is an in-memory copy), `now_ms` passed in.
//!
//! Per window kind present in any source (FiveHour, SevenDay, then others in key order):
//! 1. `reset_estimate::estimate_reset` over the Desktop-sourced samples of that kind (Desktop
//!    file series ∪ Desktop rows of the history, samples more than [`FUTURE_SLACK_MS`] in the
//!    future dropped) with the last exact reset (max of the persisted value and the captures).
//! 2. `merge::merge_window` of the CLI observations and the newest Desktop observation.
//! 3. `burn::compute` over the history samples (∪ the Desktop series) of that kind.
//! 4. `History::spark`: 96 buckets over 24 h (five_hour) or 7 d (weekly kinds). The range end is
//!    aligned up to the bucket step so the buckets do not shift on every recompute (which would
//!    make every snapshot look changed).
//!
//! Session: `active_session::pick` over the transcript tails; the capture with the same
//! `session_id` and the Desktop Code-tab session whose `cli_session_id` matches feed
//! `context::resolve`. Without any tail, the newest capture that names a model is shown instead.
//! The display name uses a learned map (persisted names ∪ `model.display_name` of the captures).
//! `project` only when `show_project`.
//!
//! Warnings:
//! - `NoPlanLimits`: captures written within the last hour exist, but none carries rate limits.
//! - `AccountMismatch`: the pipeline tracks since when [`account_mismatch_now`] has held
//!   (`account_mismatch_since_ms`); after [`ACCOUNT_MISMATCH_AFTER_MS`] the warning is shown.

use std::collections::{BTreeMap, BTreeSet};

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
use crate::history::History;
use crate::model_names::{display_name, split_1m};
use crate::sources::desktop_sessions::DesktopSession;
use crate::sources::desktop_usage::{self, DesktopUsage};
use crate::sources::statusline;
use crate::sources::transcript::TranscriptTail;
use crate::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms};

/// Samples further in the future than this (clock skew, corrupt files) are ignored.
pub const FUTURE_SLACK_MS: Ms = 5 * MINUTE_MS;
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
    /// Persisted newest exact reset per kind (from earlier captures).
    pub last_exact_resets: &'a BTreeMap<WindowKind, Ms>,
    /// Persisted model display names by base id.
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
    let exact = merged_exact_resets(inputs.last_exact_resets, inputs.captures);

    let mut windows = Vec::new();
    for kind in &kinds {
        let desktop_series = desktop_samples(inputs, kind, now_ms);
        let estimate = reset_estimate::estimate_reset(
            kind,
            &desktop_series,
            exact.get(kind).copied(),
            now_ms,
        );
        let desktop_obs = desktop_latest.iter().find(|o| &o.kind == kind);
        let Some(state) = merge::merge_window(
            kind,
            &cli,
            desktop_obs,
            estimate,
            now_ms,
            inputs.stale_after_ms,
        ) else {
            continue;
        };
        let burn_samples = all_samples(inputs, kind, now_ms);
        let burn = burn::compute(kind, &burn_samples, &state, now_ms);
        let spark = spark(inputs.history, kind, now_ms);
        windows.push(WindowView { state, burn, spark });
    }

    let session = session_view(inputs, now_ms);
    let health = SourceHealth {
        desktop: inputs.desktop_health.clone(),
        cli_last_capture_ms: statusline::last_capture_ms(inputs.captures),
        transcripts_last_activity_ms: inputs.tails.iter().map(|t| t.last_assistant_ms).max(),
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

fn merged_exact_resets(
    persisted: &BTreeMap<WindowKind, Ms>,
    captures: &[CaptureRecord],
) -> BTreeMap<WindowKind, Ms> {
    let mut map = persisted.clone();
    learn_exact_resets(&mut map, captures);
    map
}

/// Desktop samples of `kind`: the file's series ∪ Desktop rows of the history.
fn desktop_samples(inputs: &EngineInputs<'_>, kind: &WindowKind, now_ms: Ms) -> Vec<Sample> {
    let since = now_ms.saturating_sub(SAMPLE_LOOKBACK_MS);
    let from_history = inputs
        .history
        .rows()
        .iter()
        .filter(|r| r.s == Source::Desktop && r.t >= since && WindowKind::from_short(&r.w) == *kind)
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

fn session_view(inputs: &EngineInputs<'_>, now_ms: Ms) -> Option<SessionView> {
    let mut learned = inputs.learned_names.clone();
    learn_model_names(&mut learned, inputs.captures);
    let name_of = |id: &str| Some(display_name(id, &learned)).filter(|n| !n.is_empty());

    let Some(pick) = active_session::pick(inputs.tails, inputs.desktop_sessions, now_ms) else {
        return capture_only_session(inputs, &learned);
    };
    let tail = &inputs.tails[pick.index];
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
    Some(SessionView {
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
        concurrent: pick.concurrent,
    })
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
        let f = Fixture::new();
        let caps = [capture("s1", NOW - MINUTE_MS, &[])];
        let mut t = tail("s1", NOW - MINUTE_MS);
        t.model_id = Some("claude-opus-5-5".into());
        let s = build_snapshot(&f.inputs(&caps, None, &[t]), NOW);
        assert_eq!(s.session.unwrap().display_name.as_deref(), Some("Opus 5.5"));

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
}
