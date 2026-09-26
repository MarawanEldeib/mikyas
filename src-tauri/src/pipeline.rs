//! The data pipeline: one thread that loads the sources, builds snapshots and hands changes to
//! the UI, the tray and the notifier.
//!
//! - File-system events (capture dir, transcripts, Cowork) arrive as [`Msg`]s and are coalesced:
//!   work starts 250 ms after the last event, and at most 1 s after the first one.
//! - The Desktop usage file is polled by mtime every 60 s (Electron churns its data dir, so it is
//!   never watched), Desktop Code-tab sessions every 30 s, and a full transcript rescan runs
//!   every 5 min. Every 30 s the snapshot is recomputed anyway (stale flags, reset phases).
//! - A snapshot is emitted only when it differs from the last one (ignoring `generated_ms`).
//! - Limit alerts (`alerts.json`) and, when enabled, context alerts over `Snapshot::sessions`
//!   (persisted in `state.json`) are evaluated on every tick, then the enabled pace alerts and
//!   weekly recap (hook region [A], state in `state.json`) and finished turns (region [B]).
//!
//! [`Engine`] holds all state and does no Tauri work, so it is unit-tested against temp dirs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use cuw_core::alerts::{AlertSettings, AlertState};
use cuw_core::capture::CaptureRecord;
use cuw_core::engine::snapshot::{self, EngineInputs};
use cuw_core::engine::types::{DesktopHealth, SessionView, Snapshot};
use cuw_core::history::History;
use cuw_core::pace_alerts::PaceSettings;
use cuw_core::paths::Paths;
use cuw_core::saferead::SafeReader;
use cuw_core::sources::desktop_sessions::{self, DesktopSession};
use cuw_core::sources::desktop_usage::{self, DesktopUsage};
use cuw_core::sources::statusline;
use cuw_core::sources::transcript::{self, HeadIdentity, TranscriptTail};
use cuw_core::sources::SourceError;
use cuw_core::time::{DAY_MS, MINUTE_MS, Ms, SECOND_MS, now_ms};
use cuw_core::turns::{FinishedTurn, FinishedTurns, TurnInfo};
use tauri::{AppHandle, Emitter};

use crate::history_view::local_day_starts;
use crate::notify::Alert;
use crate::settings::Settings;
use crate::state::{PersistedState, Shared, load_json, lock, save_json};

pub const COALESCE_QUIET: Duration = Duration::from_millis(250);
pub const COALESCE_MAX: Duration = Duration::from_secs(1);
pub const RECOMPUTE_EVERY_MS: Ms = 30 * SECOND_MS;
pub const DESKTOP_POLL_MS: Ms = 60 * SECOND_MS;
pub const SESSIONS_POLL_MS: Ms = 30 * SECOND_MS;
pub const FULL_SCAN_MS: Ms = 5 * MINUTE_MS;
pub const MAINTENANCE_EVERY_MS: Ms = DAY_MS;
/// Transcripts older than this are not considered for the active session.
pub const TRANSCRIPT_MAX_AGE_MS: Ms = 7 * DAY_MS;
/// At most this many transcripts are tracked.
pub const MAX_TAILS: usize = 40;

/// Pipeline input.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Something changed in the capture dir.
    Captures,
    /// A transcript (or Cowork file) changed.
    Transcript(PathBuf),
    /// Settings that affect the snapshot or alerts changed.
    SettingsChanged,
    Shutdown,
}

/// Which sources to reload on the next tick.
#[derive(Debug, Clone, Default)]
pub struct Dirty {
    pub captures: bool,
    pub transcripts: Vec<PathBuf>,
    pub full_scan: bool,
    pub desktop: bool,
    pub sessions: bool,
}

impl Dirty {
    pub fn all() -> Self {
        Self {
            captures: true,
            transcripts: Vec::new(),
            full_scan: true,
            desktop: true,
            sessions: true,
        }
    }
}

pub struct TickOutput {
    pub snapshot: Snapshot,
    /// Differs from the previous snapshot (ignoring `generated_ms`).
    pub changed: bool,
    /// Limit alerts first, then context, pace, weekly-recap and finished-turn alerts.
    pub alerts: Vec<Alert>,
}

#[derive(Debug, Clone)]
struct CachedTail {
    modified_ms: Ms,
    len: u64,
    tail: Option<TranscriptTail>,
    identity: HeadIdentity,
}

/// All pipeline state; pure except for reading sources and writing its own data files.
pub struct Engine {
    paths: Paths,
    reader: SafeReader,
    history: History,
    persisted: PersistedState,
    alerts: AlertState,
    captures: Vec<CaptureRecord>,
    tails: HashMap<PathBuf, CachedTail>,
    desktop: Option<DesktopUsage>,
    desktop_health: DesktopHealth,
    desktop_stamp: Vec<(PathBuf, Option<SystemTime>, u64)>,
    desktop_sessions: Vec<DesktopSession>,
    mismatch_since: Option<Ms>,
    last: Option<Snapshot>,
    first_eval: bool,
    /// Turns already reported as finished.
    finished: FinishedTurns,
    /// Time of the first tick: only turns that end after it are reported.
    started_ms: Option<Ms>,
}

impl Engine {
    pub fn new(paths: Paths) -> Self {
        let reader = SafeReader::new(&paths);
        let history = History::open(paths.history_file()).unwrap_or_else(|e| {
            log(&format!("history unreadable ({e}); starting a fresh one"));
            let mut aside = paths.history_file().into_os_string();
            aside.push(".unreadable");
            History::open(PathBuf::from(aside)).unwrap_or_else(|_| {
                History::open(std::env::temp_dir().join("cuw-history-fallback.jsonl"))
                    .expect("fallback history")
            })
        });
        let persisted: PersistedState = load_json(&paths.state_file());
        let alerts: AlertState = load_json(&paths.alerts_file());
        Self {
            reader,
            history,
            persisted,
            alerts,
            captures: Vec::new(),
            tails: HashMap::new(),
            desktop: None,
            desktop_health: DesktopHealth::NotFound,
            desktop_stamp: Vec::new(),
            desktop_sessions: Vec::new(),
            mismatch_since: None,
            last: None,
            first_eval: true,
            finished: FinishedTurns::default(),
            started_ms: None,
            paths,
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Reloads what `dirty` names, rebuilds the snapshot, records history and evaluates alerts.
    pub fn tick(&mut self, now: Ms, settings: &Settings, dirty: &Dirty) -> TickOutput {
        let before = self.persisted.clone();
        if dirty.captures {
            self.reload_captures(now);
        }
        if dirty.full_scan {
            self.full_transcript_scan(now);
        }
        for path in &dirty.transcripts {
            self.update_transcript(path, now);
        }
        if dirty.desktop {
            self.poll_desktop(now);
        }
        if dirty.sessions {
            self.desktop_sessions =
                desktop_sessions::load_all(&self.reader, &self.paths.desktop_sessions_dirs());
        }

        if snapshot::account_mismatch_now(&self.captures, self.desktop.as_ref(), now) {
            self.mismatch_since.get_or_insert(now);
        } else {
            self.mismatch_since = None;
        }

        let tails: Vec<TranscriptTail> = self.tails.values().filter_map(|c| c.tail.clone()).collect();
        let mut snap = self.build(now, settings, &tails);
        let mut recorded = false;
        for w in &snap.windows {
            match self.history.record(&w.state) {
                Ok(appended) => recorded |= appended,
                Err(e) => log(&format!("history append failed: {e}")),
            }
        }
        if recorded {
            // The sparklines should include the value just recorded.
            snap = self.build(now, settings, &tails);
        }

        let states: Vec<_> = snap.windows.iter().map(|w| w.state.clone()).collect();
        let alert_settings = AlertSettings {
            thresholds: settings.thresholds.clone(),
            notify_reset: settings.notify_reset,
        };
        let alerts_before = self.alerts.clone();
        let mut alerts: Vec<Alert> = self
            .alerts
            .evaluate(&states, &alert_settings, self.first_eval)
            .into_iter()
            .map(Alert::Limit)
            .collect();
        self.first_eval = false;
        if self.alerts != alerts_before {
            if let Err(e) = save_json(&self.paths.alerts_file(), &self.alerts) {
                log(&format!("alerts.json write failed: {e}"));
            }
        }
        if settings.ctx_alerts {
            let ctx = self
                .persisted
                .ctx_alerts
                .evaluate(&snap.sessions, &settings.ctx_thresholds, now);
            alerts.extend(ctx.into_iter().map(Alert::Context));
        }

        // [A] pace alerts + weekly recap ------------------------------------------------------
        // Stream alerts owns this region. Their state lives in `self.persisted`, so the
        // `state.json` save below persists it.
        let pace = PaceSettings {
            forecast: settings.pace_alerts,
            heads_up: settings.reset_heads_up,
        };
        if pace.forecast || pace.heads_up {
            let events = self.persisted.pace_alerts.evaluate(&snap.windows, pace, now);
            alerts.extend(events.into_iter().map(Alert::Pace));
        }
        if settings.weekly_recap {
            let day_starts = local_day_starts(now.saturating_sub(8 * DAY_MS), now, &chrono::Local);
            if let Some(recap) = self.persisted.recap.evaluate(&self.history, &day_starts, now) {
                alerts.push(Alert::Recap(recap));
            }
        }
        // [/A] -----------------------------------------------------------------------------------

        if now.saturating_sub(self.persisted.last_maintenance_ms) >= MAINTENANCE_EVERY_MS {
            self.maintenance(now);
        }

        // [B] finished turns -----------------------------------------------------------------
        // Stream turns owns this region (and `finished_inputs`). Dedupe is in memory only.
        let started_ms = *self.started_ms.get_or_insert(now);
        if settings.finished_alerts {
            let min_duration_ms = Ms::from(settings.finished_min_minutes) * MINUTE_MS;
            let sessions = self.finished_inputs(&snap);
            let done = self.finished.observe(&sessions, started_ms, min_duration_ms, now);
            alerts.extend(done.into_iter().map(Alert::Finished));
        }
        // [/B] -----------------------------------------------------------------------------------
        if self.persisted != before {
            if let Err(e) = save_json(&self.paths.state_file(), &self.persisted) {
                log(&format!("state.json write failed: {e}"));
            }
        }

        let changed = self.last.as_ref().is_none_or(|last| !same_content(last, &snap));
        self.last = Some(snap.clone());
        TickOutput {
            snapshot: snap,
            changed,
            alerts,
        }
    }

    fn build(&self, now: Ms, settings: &Settings, tails: &[TranscriptTail]) -> Snapshot {
        let exact = self.persisted.exact_resets();
        let inputs = EngineInputs {
            captures: &self.captures,
            desktop: self.desktop.as_ref(),
            desktop_health: &self.desktop_health,
            history: &self.history,
            tails,
            desktop_sessions: &self.desktop_sessions,
            last_exact_resets: &exact,
            learned_names: &self.persisted.learned_models,
            ctx_overrides: &settings.ctx_overrides,
            stale_after_ms: settings.stale_after_ms(),
            show_project: settings.show_project,
            account_mismatch_since_ms: self.mismatch_since,
        };
        snapshot::build_snapshot(&inputs, now)
    }

    /// Each listed session paired with its latest turn, for [`FinishedTurns::observe`].
    fn finished_inputs(&self, snap: &Snapshot) -> Vec<(FinishedTurn, TurnInfo)> {
        pair_turns(&snap.sessions, self.tails.values().filter_map(|c| c.tail.as_ref()))
    }

    fn reload_captures(&mut self, now: Ms) {
        self.captures = statusline::load_captures(&self.reader, &self.paths.capture_dir(), now);
        let mut exact = self.persisted.exact_resets();
        if snapshot::learn_exact_resets(&mut exact, &self.captures) {
            self.persisted.set_exact_resets(&exact);
        }
        snapshot::learn_model_names(&mut self.persisted.learned_models, &self.captures);
    }

    fn transcript_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.paths.projects_dir()];
        roots.extend(self.paths.cowork_dirs());
        roots
    }

    fn full_transcript_scan(&mut self, now: Ms) {
        let roots = self.transcript_roots();
        let recent = transcript::find_recent(
            &self.reader,
            &roots,
            now.saturating_sub(TRANSCRIPT_MAX_AGE_MS),
            MAX_TAILS,
        );
        let mut next = HashMap::with_capacity(recent.len());
        for file in recent {
            let cached = self.tails.remove(&file.path);
            let entry = self.scan(&file.path, file.modified_ms, file.len, cached);
            next.insert(file.path, entry);
        }
        self.tails = next;
    }

    /// Rescans one transcript named by a file-system event.
    fn update_transcript(&mut self, path: &Path, now: Ms) {
        let eligible = transcript_event_path(path) && self.reader.allows(path);
        let meta = std::fs::metadata(path).ok().filter(|m| m.is_file());
        let (Some(meta), true) = (meta, eligible) else {
            self.tails.remove(path);
            return;
        };
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as Ms);
        if now.saturating_sub(modified_ms) > TRANSCRIPT_MAX_AGE_MS {
            return;
        }
        let cached = self.tails.remove(path);
        let entry = self.scan(path, modified_ms, meta.len(), cached);
        self.tails.insert(path.to_path_buf(), entry);
        if self.tails.len() > MAX_TAILS {
            // Drop the least recently modified.
            if let Some(oldest) = self
                .tails
                .iter()
                .min_by_key(|(_, c)| c.modified_ms)
                .map(|(p, _)| p.clone())
            {
                self.tails.remove(&oldest);
            }
        }
    }

    fn scan(&self, path: &Path, modified_ms: Ms, len: u64, cached: Option<CachedTail>) -> CachedTail {
        if let Some(c) = cached.as_ref().filter(|c| c.modified_ms == modified_ms && c.len == len) {
            return c.clone();
        }
        let mut identity = cached.as_ref().and_then(|c| c.identity.clone());
        let tail = match transcript::scan_tail(&self.reader, path, &mut identity) {
            Ok(Some(tail)) => Some(tail),
            // A line longer than the tail scan reads (a huge paste or tool result) can follow the
            // last assistant line. While the file only grows, the last tail seen still describes
            // the session; it keeps its timestamp, so the session ages out of the list as usual.
            Ok(None) => cached.filter(|c| len > c.len).and_then(|c| c.tail),
            Err(_) => None,
        };
        CachedTail {
            modified_ms,
            len,
            tail,
            identity,
        }
    }

    /// Reloads the Desktop usage file when any candidate's mtime or length changed.
    /// Samples dated more than `FUTURE_SLACK_MS` after `now` (a clock that ran ahead) are ignored.
    fn poll_desktop(&mut self, now: Ms) {
        let stamp: Vec<(PathBuf, Option<SystemTime>, u64)> = self
            .paths
            .desktop_usage_files()
            .into_iter()
            .map(|p| {
                let meta = std::fs::metadata(&p).ok();
                let modified = meta.as_ref().and_then(|m| m.modified().ok());
                let len = meta.map_or(0, |m| m.len());
                (p, modified, len)
            })
            .collect();
        if stamp == self.desktop_stamp && !self.desktop_stamp.is_empty() {
            return;
        }
        self.desktop_stamp = stamp;
        let max_t_ms = now.saturating_add(snapshot::FUTURE_SLACK_MS);
        match desktop_usage::load(&self.reader, &self.paths, max_t_ms) {
            Ok(Some(usage)) => {
                self.desktop_health = DesktopHealth::Ok {
                    last_sample_ms: usage.last_sample_ms,
                };
                match self
                    .history
                    .backfill_desktop(&usage, self.persisted.desktop_watermark_ms)
                {
                    Ok(w) => self.persisted.desktop_watermark_ms = w,
                    Err(e) => log(&format!("history backfill failed: {e}")),
                }
                self.desktop = Some(usage);
            }
            Ok(None) => {
                self.desktop_health = DesktopHealth::NotFound;
                self.desktop = None;
            }
            Err(SourceError::SchemaChanged(version)) => {
                self.desktop_health = DesktopHealth::SchemaChanged { version };
                self.desktop = None;
            }
            Err(_) => {
                // Mid-write or otherwise unreadable: keep the previous good value, retry next poll.
                self.desktop_stamp.clear();
                if self.desktop.is_none() {
                    self.desktop_health = DesktopHealth::Unreadable;
                }
            }
        }
    }

    fn maintenance(&mut self, now: Ms) {
        if let Err(e) = statusline::prune(&self.paths.capture_dir(), now) {
            log(&format!("capture prune failed: {e}"));
        }
        if let Err(e) = self.history.compact(now) {
            log(&format!("history compact failed: {e}"));
        }
        self.persisted.last_maintenance_ms = now;
    }
}

/// Snapshots equal apart from their generation time.
fn same_content(a: &Snapshot, b: &Snapshot) -> bool {
    a.windows == b.windows
        && a.session == b.session
        && a.sessions == b.sessions
        && a.health == b.health
        && a.warnings == b.warnings
}

/// A `.jsonl` file outside any `subagents` directory.
fn transcript_event_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
        && !path
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("subagents"))
}

pub fn log(msg: &str) {
    if cfg!(debug_assertions) {
        eprintln!("[cuw] {msg}");
    }
}

// ---------------------------------------------------------------------------------------------
// Thread

/// Runs the pipeline until [`Msg::Shutdown`] or the channel closes.
pub fn run(app: AppHandle, shared: Arc<Shared>, mut engine: Engine, rx: Receiver<Msg>) {
    let mut watcher = crate::watcher::Watcher::new(shared.clone(), engine.paths().clone());
    let mut dirty = Dirty::default();
    let mut first_event: Option<Instant> = None;
    let mut last_event = Instant::now();
    let start = now_ms();
    let mut next_recompute = start + RECOMPUTE_EVERY_MS;
    let mut next_desktop = start + DESKTOP_POLL_MS;
    let mut next_sessions = start + SESSIONS_POLL_MS;
    let mut next_full_scan = start + FULL_SCAN_MS;

    loop {
        let now = now_ms();
        let periodic_in = [next_recompute, next_desktop, next_sessions, next_full_scan]
            .into_iter()
            .min()
            .unwrap_or(now)
            .saturating_sub(now)
            .max(0);
        let mut timeout = Duration::from_millis(periodic_in as u64);
        if let Some(first) = first_event {
            let due = (last_event + COALESCE_QUIET).min(first + COALESCE_MAX);
            timeout = timeout.min(due.saturating_duration_since(Instant::now()));
        }

        match rx.recv_timeout(timeout) {
            Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(msg) => {
                match msg {
                    Msg::Captures => dirty.captures = true,
                    Msg::Transcript(p) => {
                        if !dirty.transcripts.contains(&p) {
                            dirty.transcripts.push(p);
                        }
                    }
                    Msg::SettingsChanged | Msg::Shutdown => {}
                }
                last_event = Instant::now();
                first_event.get_or_insert(last_event);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        let now = now_ms();
        let coalesced_due = first_event.is_some_and(|first| {
            let n = Instant::now();
            n >= last_event + COALESCE_QUIET || n >= first + COALESCE_MAX
        });
        let periodic_due = now >= next_recompute;
        if now >= next_desktop {
            dirty.desktop = true;
            next_desktop = now + DESKTOP_POLL_MS;
        }
        if now >= next_sessions {
            dirty.sessions = true;
            next_sessions = now + SESSIONS_POLL_MS;
        }
        if now >= next_full_scan {
            dirty.full_scan = true;
            next_full_scan = now + FULL_SCAN_MS;
            watcher.ensure();
        }
        if !(coalesced_due || periodic_due || dirty.desktop || dirty.sessions || dirty.full_scan) {
            continue;
        }
        if periodic_due {
            next_recompute = now + RECOMPUTE_EVERY_MS;
            watcher.ensure();
        }
        first_event = None;

        let settings = shared.settings().clone();
        let out = engine.tick(now, &settings, &std::mem::take(&mut dirty));
        deliver(&app, &shared, out);
    }
}

/// Publishes a tick's results: snapshot event, tray, notifications.
pub fn deliver(app: &AppHandle, shared: &Shared, out: TickOutput) {
    for event in &out.alerts {
        crate::notify::show_alert(app, event);
    }
    if out.changed {
        *lock(&shared.snapshot) = out.snapshot.clone();
        crate::tray::update(app, &out.snapshot);
        let _ = app.emit("snapshot", &out.snapshot);
    }
}

// ---- [B] helpers

/// Pairs each session view with the turn of its transcript (the newest one when several files
/// share the session id). The view supplies the toast's project (only set while `show_project`
/// is on), model and surface; sessions without a transcript are left out.
fn pair_turns<'a>(
    sessions: &[SessionView],
    tails: impl Iterator<Item = &'a TranscriptTail>,
) -> Vec<(FinishedTurn, TurnInfo)> {
    let mut by_key: HashMap<String, &TranscriptTail> = HashMap::new();
    for tail in tails {
        let slot = by_key.entry(snapshot::session_key(&tail.session_id)).or_insert(tail);
        if tail.last_assistant_ms > slot.last_assistant_ms {
            *slot = tail;
        }
    }
    sessions
        .iter()
        .filter_map(|view| {
            let tail = by_key.get(&view.key)?;
            let turn = FinishedTurn {
                key: view.key.clone(),
                duration_ms: 0,
                model: view.display_name.clone(),
                project: view.project.clone(),
                entrypoint: view.entrypoint,
            };
            Some((turn, tail.turn.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuw_core::engine::types::{Source, WindowKind};

    fn setup() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_roots(
            tmp.path().join(".claude"),
            vec![tmp.path().join("Roaming").join("Claude")],
            tmp.path().join("data"),
        );
        (tmp, paths)
    }

    fn write_desktop(paths: &Paths, samples: &[(Ms, u32, u32)]) {
        let root = &paths.desktop_roots()[0];
        std::fs::create_dir_all(root).unwrap();
        let samples: Vec<String> = samples
            .iter()
            .map(|(t, fh, sd)| format!(r#"{{"t":{t},"org":"o","u":{{"fh":{fh},"sd":{sd}}}}}"#))
            .collect();
        std::fs::write(
            root.join("plan-usage-history.json"),
            format!(r#"{{"version":2,"samples":[{}]}}"#, samples.join(",")),
        )
        .unwrap();
    }

    fn write_capture(paths: &Paths, session: &str, changed: Ms, fh: f32, reset_ms: Ms) {
        std::fs::create_dir_all(paths.capture_dir()).unwrap();
        let json = format!(
            r#"{{"v":1,"session_id":"{session}","written_at_ms":{changed},"changed_at_ms":{changed},"fingerprint":1,
            "model":{{"id":"claude-opus-5-5","display_name":"Opus 5.5"}},
            "rate_limits":{{"five_hour":{{"used_percentage":{fh},"resets_at":{}}}}}}}"#,
            reset_ms / 1000
        );
        std::fs::write(paths.capture_dir().join(format!("{session}.json")), json).unwrap();
    }

    fn write_transcript(paths: &Paths, session: &str, ts: &str) -> PathBuf {
        write_transcript_ctx(paths, session, ts, 99_990)
    }

    /// A one-turn transcript whose context is `cache_read + 10` tokens.
    fn write_transcript_ctx(paths: &Paths, session: &str, ts: &str, cache_read: u64) -> PathBuf {
        let dir = paths.projects_dir().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let line = format!(
            r#"{{"type":"assistant","isSidechain":false,"sessionId":"{session}","entrypoint":"cli","cwd":"C:\\p\\proj","timestamp":"{ts}","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":{cache_read}}}}}}}"#
        );
        let path = dir.join(format!("{session}.jsonl"));
        std::fs::write(&path, format!("{line}\n")).unwrap();
        path
    }

    #[test]
    fn engine_builds_from_all_sources_and_persists() {
        let (_t, paths) = setup();
        let now = now_ms();
        write_desktop(&paths, &[(now - 20 * MINUTE_MS, 30, 60), (now - 5 * MINUTE_MS, 31, 61)]);
        write_capture(&paths, "s1", now - 10 * MINUTE_MS, 30.5, now + 3_600_000);
        let ts = chrono_like(now - 60_000);
        write_transcript(&paths, "s1", &ts);

        let mut engine = Engine::new(paths.clone());
        let settings = Settings::default();
        let out = engine.tick(now, &settings, &Dirty::all());
        assert!(out.changed);
        let s = &out.snapshot;
        assert_eq!(s.windows.len(), 2);
        assert_eq!(s.windows[0].state.kind, WindowKind::FiveHour);
        assert_eq!(s.windows[0].state.source, Source::Desktop);
        assert_eq!(s.windows[0].state.pct, 31.0);
        assert!(s.windows[0].state.reset.is_exact(), "CLI reset kept");
        let session = s.session.as_ref().expect("session");
        assert_eq!(session.display_name.as_deref(), Some("Opus 5.5"));
        assert_eq!(session.ctx_tokens, Some(100_000));
        assert!(matches!(s.health.desktop, DesktopHealth::Ok { .. }));

        // Unchanged inputs at the same instant → not changed (burn forecasts move with time).
        let again = engine.tick(now, &settings, &Dirty::default());
        assert!(!again.changed);

        // State and history were written.
        let persisted: PersistedState = load_json(&paths.state_file());
        assert_eq!(persisted.learned_models.get("claude-opus-5-5").map(String::as_str), Some("Opus 5.5"));
        assert!(persisted.desktop_watermark_ms > 0);
        assert!(persisted.last_exact_resets.contains_key("five_hour"));
        assert!(paths.history_file().exists());
    }

    #[test]
    fn transcript_events_update_the_session_and_alerts_fire_once() {
        let (_t, paths) = setup();
        let now = now_ms();
        let mut engine = Engine::new(paths.clone());
        let settings = Settings::default();
        let first = engine.tick(now, &settings, &Dirty::all());
        assert!(first.snapshot.session.is_none());
        assert!(first.snapshot.windows.is_empty());

        let path = write_transcript(&paths, "s2", &chrono_like(now - 1_000));
        let dirty = Dirty {
            transcripts: vec![path],
            ..Dirty::default()
        };
        let out = engine.tick(now + 1, &settings, &dirty);
        assert!(out.changed);
        assert!(out.snapshot.session.is_some());

        write_capture(&paths, "s2", now, 85.0, now + 3_600_000);
        let dirty = Dirty {
            captures: true,
            ..Dirty::default()
        };
        let out = engine.tick(now + 2, &settings, &dirty);
        assert_eq!(out.alerts.len(), 1, "80% threshold fires");
        let out = engine.tick(now + 3, &settings, &dirty);
        assert!(out.alerts.is_empty(), "only once");
    }

    fn context_alerts(out: &TickOutput) -> Vec<&cuw_core::ctx_alerts::CtxAlertEvent> {
        out.alerts
            .iter()
            .filter_map(|a| match a {
                Alert::Context(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn context_alerts_fire_once_and_survive_restarts() {
        let (_t, paths) = setup();
        let now = now_ms();
        // 900K tokens: a 1M window (more than 200K were seen), 90% full.
        write_transcript_ctx(&paths, "s1", &chrono_like(now - 1_000), 899_990);
        let settings = Settings::default();
        let mut engine = Engine::new(paths.clone());
        let out = engine.tick(now, &settings, &Dirty::all());
        let fired = context_alerts(&out);
        assert_eq!(fired.len(), 1, "{:?}", out.alerts);
        assert_eq!(fired[0].threshold, 90, "only the highest crossed threshold");
        assert_eq!(fired[0].key, out.snapshot.sessions[0].key);
        assert_eq!(fired[0].project, None, "project names are off by default");
        assert!(engine.tick(now + 1, &settings, &Dirty::default()).alerts.is_empty());

        let persisted: PersistedState = load_json(&paths.state_file());
        assert_eq!(persisted.ctx_alerts.sessions.len(), 1);
        let mut restarted = Engine::new(paths.clone());
        assert!(restarted.tick(now + 2, &settings, &Dirty::all()).alerts.is_empty(), "no re-fire");

        // Turned off: a second session at 90% stays quiet.
        write_transcript_ctx(&paths, "s2", &chrono_like(now), 899_990);
        let off = Settings {
            ctx_alerts: false,
            ..Settings::default()
        };
        let out = restarted.tick(now + 3, &off, &Dirty::all());
        assert!(out.alerts.is_empty());
        assert_eq!(out.snapshot.sessions.len(), 2);
        // Back on: only the new session alerts.
        let out = restarted.tick(now + 4, &settings, &Dirty::default());
        let fired = context_alerts(&out);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].key, out.snapshot.sessions[0].key);
    }

    #[test]
    fn context_alerts_skip_guesses_over_the_default_size() {
        let (_t, paths) = setup();
        let now = now_ms();
        // 180K tokens and no capture: 90% of the 200K default, but it may well be a 1M session.
        write_transcript_ctx(&paths, "s1", &chrono_like(now - 1_000), 179_990);
        let mut engine = Engine::new(paths.clone());
        let out = engine.tick(now, &Settings::default(), &Dirty::all());
        assert!(out.snapshot.sessions[0].ctx_pct.is_some_and(|p| p >= 89.9));
        assert!(context_alerts(&out).is_empty(), "{:?}", out.alerts);
        // Once a statusline capture confirms the 200K size, the same estimate alerts.
        std::fs::create_dir_all(paths.capture_dir()).unwrap();
        let capture = format!(
            r#"{{"v":1,"session_id":"s1","written_at_ms":{now},"changed_at_ms":{now},"fingerprint":1,
            "context":{{"context_window_size":200000}}}}"#
        );
        std::fs::write(paths.capture_dir().join("s1.json"), capture).unwrap();
        let dirty = Dirty {
            captures: true,
            ..Dirty::default()
        };
        let out = engine.tick(now + 1, &Settings::default(), &dirty);
        assert_eq!(context_alerts(&out).iter().map(|e| e.threshold).collect::<Vec<_>>(), vec![90]);
    }

    #[test]
    fn desktop_samples_from_the_future_are_ignored() {
        let (_t, paths) = setup();
        let now = now_ms();
        // The last sample was written while the clock ran three days ahead.
        write_desktop(
            &paths,
            &[(now - 20 * MINUTE_MS, 30, 60), (now - 5 * MINUTE_MS, 31, 61), (now + 3 * DAY_MS, 99, 99)],
        );
        let mut engine = Engine::new(paths.clone());
        let out = engine.tick(now, &Settings::default(), &Dirty::all());
        assert_eq!(out.snapshot.windows[0].state.pct, 31.0);
        assert_eq!(
            out.snapshot.health.desktop,
            DesktopHealth::Ok { last_sample_ms: Some(now - 5 * MINUTE_MS) }
        );
        let persisted: PersistedState = load_json(&paths.state_file());
        assert_eq!(persisted.desktop_watermark_ms, now - 5 * MINUTE_MS);
    }

    #[test]
    fn context_thresholds_come_from_settings() {
        let (_t, paths) = setup();
        let now = now_ms();
        write_transcript_ctx(&paths, "s1", &chrono_like(now - 1_000), 499_990); // 50% of 1M
        let mut engine = Engine::new(paths.clone());
        assert!(engine.tick(now, &Settings::default(), &Dirty::all()).alerts.is_empty());
        let low = Settings {
            ctx_thresholds: vec![40, 45],
            ..Settings::default()
        };
        let fired = engine.tick(now + 1, &low, &Dirty::default());
        assert_eq!(context_alerts(&fired).iter().map(|e| e.threshold).collect::<Vec<_>>(), vec![45]);
    }

    #[test]
    fn session_list_changes_are_emitted() {
        let (_t, paths) = setup();
        let now = now_ms();
        write_transcript(&paths, "s1", &chrono_like(now - 1_000));
        let mut engine = Engine::new(paths.clone());
        assert!(engine.tick(now, &Settings::default(), &Dirty::all()).changed);
        // An older second session changes only the list, not the header session.
        let path = write_transcript(&paths, "s2", &chrono_like(now - 3_600_000));
        let dirty = Dirty {
            transcripts: vec![path],
            ..Dirty::default()
        };
        let out = engine.tick(now, &Settings::default(), &dirty);
        assert!(out.changed);
        assert_eq!(out.snapshot.sessions.len(), 2);
        assert_eq!(out.snapshot.session.as_ref(), out.snapshot.sessions.first());
    }

    fn append(path: &Path, text: &str) {
        let mut file = std::fs::File::options().append(true).open(path).unwrap();
        std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
    }

    #[test]
    fn huge_line_after_the_last_assistant_line_keeps_the_session() {
        let (_t, paths) = setup();
        let now = now_ms();
        let path = write_transcript(&paths, "s1", &chrono_like(now - 60_000));
        let settings = Settings::default();
        let mut engine = Engine::new(paths.clone());
        let before = engine.tick(now, &settings, &Dirty::all()).snapshot.session.expect("session");
        let dirty = Dirty {
            transcripts: vec![path.clone()],
            ..Dirty::default()
        };

        // A pasted prompt longer than the tail scan reads now follows the assistant line.
        let pad = "x".repeat(transcript::TAIL_RETRY_BYTES as usize + 1024);
        append(&path, &format!(r#"{{"type":"user","message":{{"content":"{pad}"}}}}"#));
        let out = engine.tick(now + 1, &settings, &dirty);
        assert_eq!(out.snapshot.session.as_ref(), Some(&before), "the file only grew");
        append(&path, "\n");
        let out = engine.tick(now + 2, &settings, &Dirty::all());
        assert_eq!(out.snapshot.session.as_ref(), Some(&before), "the full rescan keeps it too");

        // A rewritten (shorter) file without an assistant line drops it.
        std::fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
        assert!(engine.tick(now + 3, &settings, &dirty).snapshot.session.is_none());
    }

    #[test]
    fn cached_identity_follows_a_model_switch() {
        let (_t, paths) = setup();
        let now = now_ms();
        let dir = paths.projects_dir().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s1.jsonl");
        let line = |model: &str, ts: Ms| {
            format!(
                r#"{{"type":"assistant","sessionId":"s1","timestamp":"{}","message":{{"model":"{model}","usage":{{"input_tokens":50000}}}}}}"#,
                chrono_like(ts)
            ) + "\n"
        };
        let identity = r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-opus-5-5[1m]"}}}"#;
        std::fs::write(&path, format!("{identity}\n{}", line("claude-opus-5-5", now - 2_000))).unwrap();
        let settings = Settings::default();
        let mut engine = Engine::new(paths.clone());
        let out = engine.tick(now, &settings, &Dirty::all());
        assert_eq!(out.snapshot.session.map(|s| s.ctx_size), Some(1_000_000));

        append(&path, &line("claude-sonnet-5", now - 1_000));
        let dirty = Dirty {
            transcripts: vec![path.clone()],
            ..Dirty::default()
        };
        let out = engine.tick(now + 1, &settings, &dirty);
        assert_eq!(out.snapshot.session.map(|s| s.ctx_size), Some(200_000), "identity was for opus");
    }

    #[test]
    fn transcript_event_filter() {
        assert!(transcript_event_path(Path::new("C:/x/projects/p/a.jsonl")));
        assert!(!transcript_event_path(Path::new("C:/x/projects/p/subagents/a.jsonl")));
        assert!(!transcript_event_path(Path::new("C:/x/projects/p/a.json")));
    }

    /// RFC 3339 UTC timestamp for epoch ms (test helper without a chrono dependency here).
    fn chrono_like(ms: Ms) -> String {
        let secs = ms.div_euclid(1000);
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60,
            ms.rem_euclid(1000)
        )
    }

    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    fn turn_line(kind: &str, ts: Ms, stop: &str, content: &str) -> String {
        let usage = if kind == "assistant" { r#","model":"claude-opus-5-5","usage":{"input_tokens":1}"# } else { "" };
        format!(
            r#"{{"type":"{kind}","sessionId":"s1","entrypoint":"cli","cwd":"C:\\x\\demo-app","timestamp":"{}","message":{{"stop_reason":{stop},"content":{content}{usage}}}}}"#,
            chrono_like(ts)
        ) + "\n"
    }

    #[test]
    fn finished_turns_are_reported_once() {
        let (_t, paths) = setup();
        let now = now_ms();
        let dir = paths.projects_dir().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s1.jsonl");
        let prompt = turn_line("user", now - 5 * MINUTE_MS, "null", r#""fix the build""#);
        let working = turn_line("assistant", now - 4 * MINUTE_MS, r#""tool_use""#, r#"[{"type":"tool_use"}]"#);
        let dirty = Dirty {
            transcripts: vec![path.clone()],
            ..Dirty::default()
        };

        for show_project in [false, true] {
            let settings = Settings {
                show_project,
                ..Settings::default()
            };
            let mut engine = Engine::new(paths.clone());
            let finished = |out: TickOutput| -> Vec<FinishedTurn> {
                out.alerts
                    .into_iter()
                    .filter_map(|a| if let Alert::Finished(t) = a { Some(t) } else { None })
                    .collect()
            };
            std::fs::write(&path, format!("{prompt}{working}")).unwrap();
            assert!(finished(engine.tick(now, &settings, &Dirty::all())).is_empty(), "still working");

            append(&path, &turn_line("assistant", now + 30_000, r#""end_turn""#, r#"[{"type":"text"}]"#));
            let done = finished(engine.tick(now + 60_000, &settings, &dirty));
            assert_eq!(done.len(), 1, "show_project={show_project}");
            assert_eq!(done[0].duration_ms, 5 * MINUTE_MS + 30_000);
            assert_eq!(done[0].entrypoint, cuw_core::engine::types::Entrypoint::Cli);
            assert_eq!(done[0].project.as_deref(), show_project.then_some("demo-app"));
            assert!(finished(engine.tick(now + 90_000, &settings, &dirty)).is_empty(), "reported once");
        }

        // Turned off: nothing is reported.
        let settings = Settings {
            finished_alerts: false,
            ..Settings::default()
        };
        let mut engine = Engine::new(paths.clone());
        std::fs::write(&path, format!("{prompt}{working}")).unwrap();
        engine.tick(now, &settings, &Dirty::all());
        append(&path, &turn_line("assistant", now + 30_000, r#""end_turn""#, r#"[{"type":"text"}]"#));
        let out = engine.tick(now + 60_000, &settings, &dirty);
        assert!(!out.alerts.iter().any(|a| matches!(a, Alert::Finished(_))));
    }

    #[test]
    fn turns_pair_with_the_newest_tail_of_a_session() {
        let tail = |path: &str, session: &str, last: Ms, ended: Ms| TranscriptTail {
            path: PathBuf::from(path),
            session_id: session.into(),
            entrypoint: cuw_core::engine::types::Entrypoint::Cowork,
            model_id: Some("claude-opus-5-5".into()),
            ctx_tokens: 1,
            max_ctx_tokens_seen: 1,
            identity_1m: None,
            last_assistant_ms: last,
            project: Some("from-transcript".into()),
            turn: TurnInfo {
                ended_ms: Some(ended),
                started_ms: Some(0),
                start_is_lower_bound: false,
            },
        };
        let tails = [tail("a1", "a", 10, 10), tail("a2", "a", 20, 20), tail("b", "b", 5, 5)];
        let view = |session: &str| SessionView {
            key: snapshot::session_key(session),
            model_id: Some("claude-opus-5-5".into()),
            display_name: Some("Opus 5.5".into()),
            ctx_pct: None,
            ctx_tokens: None,
            ctx_size: 200_000,
            ctx_basis: cuw_core::engine::types::CtxBasis::Default,
            ctx_is_estimate: true,
            entrypoint: cuw_core::engine::types::Entrypoint::Cowork,
            last_active_ms: 20,
            project: None,
            concurrent: 1,
        };
        let pairs = pair_turns(&[view("a"), view("gone")], tails.iter());
        assert_eq!(pairs.len(), 1, "sessions without a tail are left out");
        let (turn, info) = &pairs[0];
        assert_eq!(turn.key, snapshot::session_key("a"));
        assert_eq!(info.ended_ms, Some(20), "the newest file of the session");
        assert_eq!(turn.project, None, "the project comes from the view");
        assert_eq!(turn.model.as_deref(), Some("Opus 5.5"));
    }
}
