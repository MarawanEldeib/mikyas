//! A test double of the app's data pipeline (`src-tauri/src/pipeline.rs`), built only from this
//! crate's public API, over synthetic Claude Code / Claude Desktop / widget data directories in a
//! temp dir.
//!
//! Every tick reloads every source (the app reloads on file events and timers; the result is the
//! same), builds the snapshot, records history and evaluates every alert kind in the app's order.
//! [`Widget::restart`] persists the state files the app persists (`state.json`, `alerts.json`),
//! drops everything held in memory and loads again, as a real restart does.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use mikyas_core::alerts::{AlertEvent, AlertSettings, AlertState};
use mikyas_core::capture::{self, WriteOutcome};
use mikyas_core::ctx_alerts::{CtxAlertEvent, CtxAlertState};
use mikyas_core::engine::snapshot::{self, EngineInputs};
use mikyas_core::engine::types::{DesktopHealth, SessionView, Snapshot, WindowKind, WindowView};
use mikyas_core::history::History;
use mikyas_core::pace_alerts::{PaceAlertEvent, PaceAlertState, PaceSettings};
use mikyas_core::paths::Paths;
use mikyas_core::recap::{RecapState, WeeklyRecap};
use mikyas_core::saferead::SafeReader;
use mikyas_core::sources::SourceError;
use mikyas_core::sources::desktop_usage::{self, DesktopUsage};
use mikyas_core::sources::statusline;
use mikyas_core::sources::transcript::{self, HeadIdentity, TranscriptTail};
use mikyas_core::time::{DAY_MS, MINUTE_MS, Ms};
use mikyas_core::turns::{FinishedTurn, FinishedTurns, TurnInfo};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SID: &str = "00000000-0000-4000-8000-000000000001";
pub const SID2: &str = "22222222-2222-4222-8222-222222222222";
const ORG: &str = "11111111-1111-4111-8111-111111111111";
const PROJECT_DIR: &str = "C--Users-tester-proj";

/// The app's default settings that matter to the pipeline.
pub const THRESHOLDS: [u8; 2] = [80, 95];
pub const CTX_THRESHOLDS: [u8; 2] = [80, 90];
pub const STALE_AFTER_MS: Ms = 20 * MINUTE_MS;
pub const FINISHED_MIN_MS: Ms = 3 * MINUTE_MS;
/// Local days the weekly recap looks at (as the app).
const RECAP_DAYS: i64 = 8;

/// Synthetic data directories: `claude/` (Claude Code home), `desktop/` (Claude Desktop root)
/// and `data/` (the widget's own data root).
pub struct Env {
    _tmp: tempfile::TempDir,
    pub paths: Paths,
}

impl Env {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let claude = tmp.path().join("claude");
        let desktop = tmp.path().join("desktop");
        fs::create_dir_all(claude.join("projects").join(PROJECT_DIR)).unwrap();
        fs::create_dir_all(&desktop).unwrap();
        let paths = Paths::with_roots(claude, vec![desktop], tmp.path().join("data"));
        Self { _tmp: tmp, paths }
    }

    pub fn desktop_file(&self) -> PathBuf {
        self.paths.desktop_roots()[0].join("plan-usage-history.json")
    }

    /// Writes Desktop's usage file: `(t, [(key, pct)])` per sample, all of one account.
    pub fn write_desktop(&self, samples: &[(Ms, &[(&str, f64)])]) {
        let samples: Vec<Value> = samples
            .iter()
            .map(|(t, u)| {
                let u: serde_json::Map<String, Value> = u.iter().map(|(k, p)| ((*k).to_owned(), json!(p))).collect();
                json!({ "t": t, "org": ORG, "u": u })
            })
            .collect();
        self.write_desktop_raw(&serde_json::to_vec(&json!({ "version": 2, "samples": samples })).unwrap());
    }

    pub fn write_desktop_raw(&self, bytes: &[u8]) {
        fs::write(self.desktop_file(), bytes).unwrap();
    }

    /// Runs one statusline payload through the shim's capture path, as `mikyas-capture.exe` does.
    pub fn statusline(&self, payload: &Statusline, now_ms: Ms) -> WriteOutcome {
        let bytes = serde_json::to_vec(&payload.json()).unwrap();
        capture::capture_from_bytes(&bytes, &self.paths.capture_dir(), now_ms).unwrap()
    }

    pub fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.paths.projects_dir().join(PROJECT_DIR).join(format!("{session_id}.jsonl"))
    }

    /// Appends lines to a session's transcript.
    pub fn append_transcript(&self, session_id: &str, lines: &[Value]) {
        let mut file = fs::OpenOptions::new().create(true).append(true).open(self.transcript_path(session_id)).unwrap();
        for line in lines {
            let mut bytes = serde_json::to_vec(line).unwrap();
            bytes.push(b'\n');
            file.write_all(&bytes).unwrap();
        }
    }
}

/// A Claude Code statusline payload (the fields the widget reads, plus some it must drop).
#[derive(Debug, Clone)]
pub struct Statusline {
    pub session_id: &'static str,
    pub model: Option<(&'static str, &'static str)>,
    /// `(used_percentage, context_window_size)`.
    pub context: Option<(f64, u64)>,
    /// `(used_percentage, resets_at in ms)`; converted to Claude Code's epoch seconds.
    pub five_hour: Option<(f64, Ms)>,
    pub seven_day: Option<(f64, Ms)>,
    pub api_ms: u64,
}

impl Statusline {
    pub fn new(session_id: &'static str) -> Self {
        Self { session_id, model: None, context: None, five_hour: None, seven_day: None, api_ms: 1 }
    }

    fn json(&self) -> Value {
        let mut rate_limits = serde_json::Map::new();
        for (key, window) in [("five_hour", self.five_hour), ("seven_day", self.seven_day)] {
            if let Some((pct, reset_ms)) = window {
                rate_limits.insert(key.into(), json!({ "used_percentage": pct, "resets_at": reset_ms / 1000 }));
            }
        }
        let mut v = json!({
            "session_id": self.session_id,
            "transcript_path": format!(r"C:\Users\tester\.claude\projects\{PROJECT_DIR}\{}.jsonl", self.session_id),
            "cwd": r"C:\Users\tester\proj",
            "version": "2.3.4",
            "cost": { "total_api_duration_ms": self.api_ms, "total_cost_usd": 1.25 },
            "rate_limits": rate_limits,
        });
        if let Some((id, name)) = self.model {
            v["model"] = json!({ "id": id, "display_name": name });
        }
        if let Some((pct, size)) = self.context {
            v["context_window"] = json!({ "used_percentage": pct, "context_window_size": size });
        }
        v
    }
}

fn iso(t: Ms) -> String {
    chrono::DateTime::from_timestamp_millis(t).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The identity attachment Claude Code writes at the start of a session.
pub fn identity_line(session_id: &str, model_id: &str) -> Value {
    json!({
        "type": "attachment", "sessionId": session_id, "entrypoint": "cli",
        "attachment": { "type": "model", "identity": { "modelId": model_id, "displayName": "Synthetic" } },
    })
}

/// A human prompt.
pub fn prompt_line(session_id: &str, t: Ms) -> Value {
    json!({
        "type": "user", "isSidechain": false, "sessionId": session_id, "entrypoint": "cli",
        "cwd": r"C:\Users\tester\proj", "timestamp": iso(t),
        "message": { "role": "user", "content": "synthetic prompt text" },
    })
}

/// An assistant line whose context (input + cache) is `ctx_tokens`.
pub fn assistant_line(session_id: &str, model: &str, ctx_tokens: u64, t: Ms, end_turn: bool) -> Value {
    json!({
        "type": "assistant", "isSidechain": false, "sessionId": session_id, "entrypoint": "cli",
        "cwd": r"C:\Users\tester\proj", "timestamp": iso(t),
        "message": {
            "model": model, "role": "assistant",
            "stop_reason": if end_turn { "end_turn" } else { "tool_use" },
            "content": [{ "type": "text", "text": "synthetic answer text" }],
            "usage": { "input_tokens": 3, "cache_creation_input_tokens": 0,
                       "cache_read_input_tokens": ctx_tokens - 3, "output_tokens": 10 },
        },
    })
}

/// `state.json`: the part of the app's persisted state the pipeline owns.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    last_exact_resets: BTreeMap<WindowKind, Ms>,
    desktop_watermark_ms: Ms,
    learned_models: BTreeMap<String, String>,
    ctx_alerts: CtxAlertState,
    pace_alerts: PaceAlertState,
    recap: RecapState,
}

/// Everything one tick produced.
#[derive(Debug)]
pub struct Tick {
    pub snap: Snapshot,
    pub limit: Vec<AlertEvent>,
    pub ctx: Vec<CtxAlertEvent>,
    pub pace: Vec<PaceAlertEvent>,
    pub recap: Option<WeeklyRecap>,
    pub finished: Vec<FinishedTurn>,
}

impl Tick {
    pub fn window(&self, kind: &WindowKind) -> Option<&WindowView> {
        self.snap.windows.iter().find(|w| &w.state.kind == kind)
    }

    pub fn five_hour(&self) -> &WindowView {
        self.window(&WindowKind::FiveHour).expect("five_hour window")
    }

    pub fn seven_day(&self) -> &WindowView {
        self.window(&WindowKind::SevenDay).expect("seven_day window")
    }

    /// Number of toasts of any kind.
    pub fn toasts(&self) -> usize {
        self.limit.len() + self.ctx.len() + self.pace.len() + usize::from(self.recap.is_some()) + self.finished.len()
    }
}

struct CachedTail {
    len: u64,
    tail: Option<TranscriptTail>,
    identity: HeadIdentity,
}

/// The app's pipeline over an [`Env`].
pub struct Widget {
    paths: Paths,
    reader: SafeReader,
    pub history: History,
    persisted: Persisted,
    alerts: AlertState,
    captures: Vec<capture::CaptureRecord>,
    tails: HashMap<PathBuf, CachedTail>,
    desktop: Option<DesktopUsage>,
    pub desktop_health: DesktopHealth,
    mismatch_since: Option<Ms>,
    first_eval: bool,
    finished: FinishedTurns,
    started_ms: Option<Ms>,
}

impl Widget {
    pub fn start(env: &Env) -> Self {
        let paths = env.paths.clone();
        Self {
            reader: SafeReader::new(&paths),
            history: History::open(paths.history_file()).unwrap(),
            persisted: load_json(&paths.state_file()),
            alerts: load_json(&paths.alerts_file()),
            captures: Vec::new(),
            tails: HashMap::new(),
            desktop: None,
            desktop_health: DesktopHealth::NotFound,
            mismatch_since: None,
            first_eval: true,
            finished: FinishedTurns::default(),
            started_ms: None,
            paths,
        }
    }

    /// Quits (the state files are already saved by every tick) and starts again.
    pub fn restart(self, env: &Env) -> Self {
        drop(self);
        Self::start(env)
    }

    pub fn tick(&mut self, now: Ms) -> Tick {
        let before = self.persisted.clone();
        self.reload_captures(now);
        self.scan_transcripts();
        self.poll_desktop(now);

        if snapshot::account_mismatch_now(&self.captures, self.desktop.as_ref(), now) {
            self.mismatch_since.get_or_insert(now);
        } else {
            self.mismatch_since = None;
        }

        let mut snap = self.build(now);
        let mut recorded = false;
        for w in &snap.windows {
            recorded |= self.history.record(&w.state).unwrap();
        }
        if recorded {
            snap = self.build(now);
        }

        let states: Vec<_> = snap.windows.iter().map(|w| w.state.clone()).collect();
        let alert_settings = AlertSettings { thresholds: THRESHOLDS.to_vec(), notify_reset: true };
        let alerts_before = self.alerts.clone();
        let limit = self.alerts.evaluate(&states, &alert_settings, self.first_eval);
        self.first_eval = false;
        if self.alerts != alerts_before {
            save_json(&self.paths.alerts_file(), &self.alerts);
        }
        let ctx = self.persisted.ctx_alerts.evaluate(&snap.sessions, &CTX_THRESHOLDS, now);
        let pace =
            self.persisted.pace_alerts.evaluate(&snap.windows, PaceSettings { forecast: true, heads_up: true }, now);
        let first_day = (now - (RECAP_DAYS - 1) * DAY_MS).div_euclid(DAY_MS) * DAY_MS;
        let day_starts: Vec<Ms> = (0..RECAP_DAYS).map(|i| first_day + i * DAY_MS).collect();
        let recap = self.persisted.recap.evaluate(&self.history, &day_starts, now);

        let started_ms = *self.started_ms.get_or_insert(now);
        let sessions = pair_turns(&snap.sessions, self.tails.values().filter_map(|c| c.tail.as_ref()));
        let finished = self.finished.observe(&sessions, started_ms, FINISHED_MIN_MS, now);

        if self.persisted != before {
            save_json(&self.paths.state_file(), &self.persisted);
        }
        Tick { snap, limit, ctx, pace, recap, finished }
    }

    fn build(&self, now: Ms) -> Snapshot {
        let tails: Vec<TranscriptTail> = self.tails.values().filter_map(|c| c.tail.clone()).collect();
        let inputs = EngineInputs {
            captures: &self.captures,
            desktop: self.desktop.as_ref(),
            desktop_health: &self.desktop_health,
            history: &self.history,
            tails: &tails,
            desktop_sessions: &[],
            last_exact_resets: &self.persisted.last_exact_resets,
            learned_names: &self.persisted.learned_models,
            ctx_overrides: &BTreeMap::new(),
            stale_after_ms: STALE_AFTER_MS,
            show_project: true,
            account_mismatch_since_ms: self.mismatch_since,
        };
        snapshot::build_snapshot(&inputs, now)
    }

    fn reload_captures(&mut self, now: Ms) {
        self.captures = statusline::load_captures(&self.reader, &self.paths.capture_dir(), now);
        snapshot::learn_exact_resets(&mut self.persisted.last_exact_resets, &self.captures);
        snapshot::learn_model_names(&mut self.persisted.learned_models, &self.captures);
    }

    fn scan_transcripts(&mut self) {
        // The synthetic clock is years away from the files' real modification times, so every
        // transcript counts as recent (the app filters on `now - 7 days`).
        let found = transcript::find_recent(&self.reader, &[self.paths.projects_dir()], 0, 40);
        let mut next = HashMap::new();
        for file in found {
            let cached = self.tails.remove(&file.path);
            let mut identity = cached.as_ref().and_then(|c| c.identity.clone());
            let entry = match transcript::scan_tail(&self.reader, &file.path, &mut identity) {
                Ok(Some(tail)) => CachedTail { len: file.len, tail: Some(tail), identity },
                // As the app: while the file only grows, the last tail seen still describes it.
                Ok(None) => CachedTail {
                    len: file.len,
                    tail: cached.filter(|c| file.len > c.len).and_then(|c| c.tail),
                    identity,
                },
                Err(_) => cached.unwrap_or(CachedTail { len: 0, tail: None, identity }),
            };
            next.insert(file.path, entry);
        }
        self.tails = next;
    }

    fn poll_desktop(&mut self, now: Ms) {
        match desktop_usage::load(&self.reader, &self.paths, now.saturating_add(snapshot::FUTURE_SLACK_MS)) {
            Ok(Some(usage)) => {
                self.desktop_health = usage.health();
                self.persisted.desktop_watermark_ms =
                    self.history.backfill_desktop(&usage, self.persisted.desktop_watermark_ms).unwrap();
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
                // Mid-write or otherwise unreadable: keep the previous good value.
                if self.desktop.is_none() {
                    self.desktop_health = DesktopHealth::Unreadable;
                }
            }
        }
    }
}

/// `src-tauri/src/pipeline.rs::pair_turns`: each listed session with its tail's latest turn.
fn pair_turns<'a>(
    sessions: &[SessionView],
    tails: impl Iterator<Item = &'a TranscriptTail>,
) -> Vec<(FinishedTurn, TurnInfo)> {
    let mut by_key: HashMap<String, &TranscriptTail> = HashMap::new();
    for tail in tails {
        let id = if tail.session_id.is_empty() { tail.path.to_string_lossy() } else { tail.session_id.as_str().into() };
        let slot = by_key.entry(snapshot::session_key(&id)).or_insert(tail);
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

fn load_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_json<T: Serialize>(path: &Path, value: &T) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
