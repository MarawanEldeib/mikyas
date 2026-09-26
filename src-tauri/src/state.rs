//! Shared runtime state (behind `tauri::State`) and the small persisted engine state
//! (`<data_root>/state.json`).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use cuw_core::ctx_alerts::CtxAlertState;
use cuw_core::engine::types::{Snapshot, WindowKind};
use cuw_core::pace_alerts::PaceAlertState;
use cuw_core::paths::Paths;
use cuw_core::recap::RecapState;
use cuw_core::time::Ms;
use serde::{Deserialize, Serialize};

use crate::pipeline::Msg;
use crate::settings::{Settings, ViewMode};

/// Engine bookkeeping that must survive restarts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    /// Newest exact `resets_at` (ms) Claude Code reported, by window key.
    pub last_exact_resets: BTreeMap<String, Ms>,
    /// Newest Desktop sample already copied into the history.
    pub desktop_watermark_ms: Ms,
    /// Model display names learned from statusline captures, by base model id.
    pub learned_models: BTreeMap<String, String>,
    /// Last time captures were pruned and the history compacted.
    pub last_maintenance_ms: Ms,
    /// Context-alert thresholds already announced, by session key (so restarts do not re-fire).
    pub ctx_alerts: CtxAlertState,
    /// Window instances whose pace forecast / reset heads-up was already shown.
    pub pace_alerts: PaceAlertState,
    /// End of the last weekly window a recap was shown for.
    pub recap: RecapState,
}

impl PersistedState {
    pub fn exact_resets(&self) -> BTreeMap<WindowKind, Ms> {
        self.last_exact_resets
            .iter()
            .map(|(k, v)| (WindowKind::from_key(k), *v))
            .collect()
    }

    pub fn set_exact_resets(&mut self, map: &BTreeMap<WindowKind, Ms>) {
        self.last_exact_resets = map.iter().map(|(k, v)| (k.key().to_owned(), *v)).collect();
    }
}

/// Why the widget is currently hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenReason {
    None,
    /// Hidden with the tray, the show/hide hotkey, or the widget's own × or right-click menu.
    User,
    /// Hidden automatically while a fullscreen video or app is in front.
    Fullscreen,
}

/// The releases newer than this build that the opt-in (notify-only) update checker found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateInfo {
    /// Newest version (`releases[0].version`).
    pub latest: String,
    /// Number of versions newer than this build (`releases.len()`).
    pub count: usize,
    /// Every newer published release, newest first.
    pub releases: Vec<ReleaseInfo>,
    /// The user chose "Later" for `latest`: the banner stays hidden until a newer version appears.
    pub dismissed: bool,
}

/// One release newer than this build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReleaseInfo {
    pub version: String,
    /// Release page of this repository (allowlisted in `updates::is_release_url`).
    pub url: String,
    /// Up to 5 short plain-text bullets from the release notes.
    pub notes: Vec<String>,
}

/// What the UI shows about the window itself (`ui-state` event).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiState {
    pub view: ViewMode,
    pub pinned: bool,
    pub click_through: bool,
    pub hotkey_error: Option<String>,
    /// Error registering the show/hide hotkey.
    pub toggle_hotkey_error: Option<String>,
    /// Docked widget is slid out (only meaningful when `settings.dock != off`).
    pub dock_expanded: bool,
    pub hidden_reason: HiddenReason,
    pub update: Option<UpdateInfo>,
    /// Connect had wrapped Claude Code's status line, and it no longer does (the card shows a
    /// Reconnect / Dismiss banner).
    pub connection_lost: bool,
}

/// Everything commands, tray, hotkey and pipeline share.
pub struct Shared {
    pub paths: Paths,
    pub settings: Mutex<Settings>,
    pub ui: Mutex<UiState>,
    pub snapshot: Mutex<Snapshot>,
    pub pipeline: Mutex<Option<Sender<Msg>>>,
    /// Set once the user chose Quit, so exit requests are no longer prevented.
    pub quitting: AtomicBool,
    /// Serialises Connect / Disconnect.
    pub connect_lock: Mutex<()>,
}

impl Shared {
    pub fn new(paths: Paths, settings: Settings, snapshot: Snapshot) -> Self {
        let ui = UiState {
            view: settings.view,
            pinned: settings.pinned,
            click_through: false,
            hotkey_error: None,
            toggle_hotkey_error: None,
            dock_expanded: false,
            hidden_reason: HiddenReason::None,
            update: None,
            connection_lost: false,
        };
        Self {
            paths,
            settings: Mutex::new(settings),
            ui: Mutex::new(ui),
            snapshot: Mutex::new(snapshot),
            pipeline: Mutex::new(None),
            quitting: AtomicBool::new(false),
            connect_lock: Mutex::new(()),
        }
    }

    pub fn settings(&self) -> MutexGuard<'_, Settings> {
        lock(&self.settings)
    }

    pub fn ui(&self) -> MutexGuard<'_, UiState> {
        lock(&self.ui)
    }

    pub fn send(&self, msg: Msg) {
        if let Some(tx) = lock(&self.pipeline).as_ref() {
            let _ = tx.send(msg);
        }
    }
}

/// Locks, recovering from poisoning (a panicked holder must not take the app down with it).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads a JSON file; missing or malformed → `T::default()`.
pub fn load_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Temp file in the same directory, flushed, then renamed over `path` (retried briefly: Windows
/// reports sharing violations while e.g. a virus scanner holds the target).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        let mut attempt = 0;
        loop {
            match fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(_) if attempt < 5 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(20 * attempt));
                }
                Err(e) => return Err(e),
            }
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_state_roundtrip_and_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("state.json");
        let empty: PersistedState = load_json(&p);
        assert_eq!(empty, PersistedState::default());
        let mut s = PersistedState::default();
        let mut resets = BTreeMap::new();
        resets.insert(WindowKind::FiveHour, 5);
        resets.insert(WindowKind::Other("seven_day_opus".into()), 7);
        s.set_exact_resets(&resets);
        s.desktop_watermark_ms = 9;
        s.recap.last_recapped_end_ms = Some(13);
        s.pace_alerts.kinds.insert(
            "five_hour".into(),
            cuw_core::pace_alerts::KindPaceState {
                forecast_fired_for: Some(17),
                heads_up_fired_for: None,
            },
        );
        s.learned_models.insert("claude-opus-5-5".into(), "Opus 5.5".into());
        s.ctx_alerts.sessions.insert(
            "af63dc4c8601ec8c".into(),
            cuw_core::ctx_alerts::SessionCtxState {
                fired: [80, 90].into(),
                last_seen_ms: 11,
            },
        );
        save_json(&p, &s).unwrap();
        let back: PersistedState = load_json(&p);
        assert_eq!(back, s);
        assert_eq!(back.exact_resets(), resets);
        std::fs::write(&p, b"garbage").unwrap();
        assert_eq!(load_json::<PersistedState>(&p), PersistedState::default());
    }

    #[test]
    fn older_state_files_load_without_ctx_alerts() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("state.json");
        std::fs::write(&p, br#"{"desktop_watermark_ms":5,"learned_models":{}}"#).unwrap();
        let s: PersistedState = load_json(&p);
        assert_eq!(s.desktop_watermark_ms, 5);
        assert_eq!(s.ctx_alerts, CtxAlertState::default());
        assert_eq!(s.pace_alerts, PaceAlertState::default());
        assert_eq!(s.recap, RecapState::default());
    }

    #[test]
    fn atomic_write_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a").join("f.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1, "no temp left");
    }
}
