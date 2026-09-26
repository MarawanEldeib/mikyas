//! Connection watchdog: notices when Claude Code's status line stops running the widget's
//! capture after Connect had wrapped it (another tool or the user rewrote `settings.json`).
//!
//! - Runs while `settings.connection_watchdog` is on. Checks at startup, when Claude Code's
//!   `settings.json` changes (its directory is watched read-only and only that file's events
//!   count; bursts are debounced) and every 5 min.
//! - Lost = `wrap.json` says we connected, and `connect::status` is `NotConfigured` or `Foreign`.
//!   An unreadable or non-strict file (`Error`) says nothing either way, so the state is kept.
//!   Disconnect removes `wrap.json`, so a deliberate disconnect never warns.
//! - The "change" is a fingerprint of the status line command now in the file (never the text).
//!   A lost connection sets `UiState.connection_lost` (emits `ui-state`) and raises one
//!   [`Alert::ConnectionLost`] toast per change; the warned fingerprint lives in
//!   `<data_root>/watchdog.json`, so a restart does not toast again.
//! - The card's banner offers Reconnect (the normal connect, no preview; the check that follows
//!   the rewrite clears the flag) and Dismiss ([`dismiss_connection_warning`]), which records the
//!   change in `watchdog.json` so the same change never warns again.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cuw_core::claude_settings::WrapRecord;
use cuw_core::fingerprint::Fnv64;
use cuw_core::paths::Paths;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::connect::{self, ConnectionStatus};
use crate::notify::Alert;
use crate::state::{Shared, load_json, lock, save_json};

/// Periodic re-check (a missed file event, or a watch that could not be set up).
pub const RECHECK: Duration = Duration::from_secs(5 * 60);
/// Quiet time after a `settings.json` event before checking (Connect and Claude Code write it in
/// bursts).
pub const DEBOUNCE: Duration = Duration::from_secs(1);
/// Dismissed changes kept (oldest dropped first).
pub const KEEP_DISMISSED: usize = 32;
const FILE_NAME: &str = "watchdog.json";

/// `<data_root>/watchdog.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchdogFile {
    /// Changes the user dismissed (fingerprints, oldest first).
    pub dismissed: Vec<String>,
    /// The change the toast was already shown for.
    pub warned: Option<String>,
}

impl WatchdogFile {
    /// Records a dismissed change (once), keeping the newest [`KEEP_DISMISSED`].
    pub fn dismiss(&mut self, change: &str) {
        if self.dismissed.iter().any(|d| d == change) {
            return;
        }
        self.dismissed.push(change.to_owned());
        let excess = self.dismissed.len().saturating_sub(KEEP_DISMISSED);
        self.dismissed.drain(..excess);
    }
}

/// What a check sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed {
    /// Connect never wrapped the status line (or Disconnect undid it).
    NotWrapped,
    Connected,
    /// Wrapped before, now something else; the change's fingerprint.
    Changed(String),
    /// The file could not be read or parsed strictly.
    Unknown,
}

/// Fingerprint of the status line command now in the file (`None`: no status line).
pub fn change_id(command: Option<&str>) -> String {
    format!("{:016x}", Fnv64::new().write_str("statusline").write_opt_str(command).finish())
}

pub fn observe(wrapped: bool, status: &ConnectionStatus) -> Observed {
    if !wrapped {
        return Observed::NotWrapped;
    }
    match status {
        ConnectionStatus::Connected { .. } => Observed::Connected,
        ConnectionStatus::NotConfigured => Observed::Changed(change_id(None)),
        ConnectionStatus::Foreign { command } => Observed::Changed(change_id(command.as_deref())),
        ConnectionStatus::Error { .. } => Observed::Unknown,
    }
}

/// What a check does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// New `connection_lost` (`None`: keep it).
    pub lost: Option<bool>,
    pub toast: bool,
    /// New `WatchdogFile::warned`.
    pub warned: Option<String>,
}

pub fn decide(observed: &Observed, enabled: bool, file: &WatchdogFile) -> Outcome {
    let keep = |lost| Outcome {
        lost,
        toast: false,
        warned: file.warned.clone(),
    };
    if !enabled {
        return keep(Some(false));
    }
    match observed {
        // Connected again (or disconnected on purpose): the next change warns again.
        Observed::NotWrapped | Observed::Connected => Outcome {
            lost: Some(false),
            toast: false,
            warned: None,
        },
        Observed::Unknown => keep(None),
        Observed::Changed(id) if file.dismissed.contains(id) => keep(Some(false)),
        Observed::Changed(id) => Outcome {
            lost: Some(true),
            toast: file.warned.as_ref() != Some(id),
            warned: Some(id.clone()),
        },
    }
}

fn wrapped(paths: &Paths) -> bool {
    std::fs::read(paths.wrap_file())
        .ok()
        .is_some_and(|b| serde_json::from_slice::<WrapRecord>(&b).is_ok())
}

/// Observes under the connect lock, so a Connect or Disconnect in progress is never half-seen.
fn observe_now(shared: &Shared) -> Observed {
    let _guard = lock(&shared.connect_lock);
    observe(wrapped(&shared.paths), &connect::status(&shared.paths))
}

/// Serialises the load → change → save of `watchdog.json` (the watchdog thread and a Dismiss
/// would otherwise drop each other's write). Never held while taking the connect lock.
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn file_path(paths: &Paths) -> PathBuf {
    paths.data_root().join(FILE_NAME)
}

fn save(path: &Path, file: &WatchdogFile) {
    if let Err(e) = save_json(path, file) {
        crate::pipeline::log(&format!("watchdog.json write failed: {e}"));
    }
}

/// One check: updates the flag (emitting `ui-state` when it changes) and toasts a new change.
fn check(app: &AppHandle, shared: &Shared) {
    let enabled = shared.settings().connection_watchdog;
    let observed = if enabled { observe_now(shared) } else { Observed::Unknown };
    let path = file_path(&shared.paths);
    let outcome = {
        let _file_guard = lock(&FILE_LOCK);
        let mut file: WatchdogFile = load_json(&path);
        let outcome = decide(&observed, enabled, &file);
        if outcome.warned != file.warned {
            file.warned.clone_from(&outcome.warned);
            save(&path, &file);
        }
        outcome
    };
    if let Some(lost) = outcome.lost {
        let changed = {
            let mut ui = shared.ui();
            std::mem::replace(&mut ui.connection_lost, lost) != lost
        };
        if changed {
            crate::window::emit_ui(app, shared);
        }
    }
    if outcome.toast {
        crate::notify::show_alert(app, &Alert::ConnectionLost);
    }
}

struct Waker(Mutex<Sender<()>>);

/// Starts the watchdog thread (first check at once: the file may have changed while the widget
/// was closed).
pub fn start(app: &AppHandle, shared: Arc<Shared>) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    app.manage(Waker(Mutex::new(tx.clone())));
    let app = app.clone();
    std::thread::Builder::new()
        .name("cuw-watchdog".into())
        .spawn(move || run(&app, &shared, &tx, &rx))?;
    Ok(())
}

/// Checks at once (the setting changed).
pub fn wake(app: &AppHandle) {
    if let Some(waker) = app.try_state::<Waker>() {
        let _ = lock(&waker.0).send(());
    }
}

/// Is this a change to Claude Code's `settings.json` (in the watched directory)?
fn is_settings_event(settings_file: &Path, event: &notify::Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
        && event.paths.iter().any(|p| {
            p.file_name()
                .zip(settings_file.file_name())
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        })
}

/// Watches the directory holding `settings.json` (editors and Connect replace the file by
/// rename). Only events are read here; the file itself is read by the check.
fn watch(paths: &Paths, tx: &Sender<()>) -> Option<RecommendedWatcher> {
    let settings_file = paths.claude_settings();
    let dir = settings_file.parent()?.to_path_buf();
    if !dir.is_dir() {
        return None;
    }
    let tx = tx.clone();
    let handler = move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| is_settings_event(&settings_file, &e)) {
            let _ = tx.send(());
        }
    };
    let mut watcher = notify::recommended_watcher(handler)
        .inspect_err(|e| crate::pipeline::log(&format!("watchdog: file watch unavailable ({e})")))
        .ok()?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some(watcher)
}

fn run(app: &AppHandle, shared: &Shared, tx: &Sender<()>, rx: &Receiver<()>) {
    let mut watcher = None;
    loop {
        if shared.quitting.load(Ordering::SeqCst) {
            break;
        }
        if watcher.is_none() && shared.settings().connection_watchdog {
            watcher = watch(&shared.paths, tx);
        }
        check(app, shared);
        match rx.recv_timeout(RECHECK) {
            Ok(()) => {
                // Let a burst of writes settle.
                while rx.recv_timeout(DEBOUNCE).is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Records the change being dismissed (while the status line is still not ours), so it never
/// warns again, and clears the banner. After a successful Reconnect it only clears.
fn dismiss(shared: &Shared) {
    if let Observed::Changed(id) = observe_now(shared) {
        let path = file_path(&shared.paths);
        let _file_guard = lock(&FILE_LOCK);
        let mut file: WatchdogFile = load_json(&path);
        file.dismiss(&id);
        save(&path, &file);
    }
    shared.ui().connection_lost = false;
}

/// The banner's Dismiss. Async: the connect lock may be held by a Connect's self-test.
#[tauri::command]
pub async fn dismiss_connection_warning(app: AppHandle, shared: State<'_, Arc<Shared>>) -> Result<(), String> {
    dismiss(&shared);
    crate::window::emit_ui(&app, &shared);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuw_core::cmdline::WrapMode;

    fn connected() -> ConnectionStatus {
        ConnectionStatus::Connected {
            mode: WrapMode::Pipe,
            original: None,
        }
    }

    fn foreign(cmd: &str) -> ConnectionStatus {
        ConnectionStatus::Foreign {
            command: Some(cmd.into()),
        }
    }

    #[test]
    fn observes_only_what_connect_had_wrapped() {
        assert_eq!(observe(false, &foreign("x")), Observed::NotWrapped);
        assert_eq!(observe(false, &ConnectionStatus::NotConfigured), Observed::NotWrapped);
        assert_eq!(observe(true, &connected()), Observed::Connected);
        assert_eq!(observe(true, &foreign("x")), Observed::Changed(change_id(Some("x"))));
        assert_eq!(observe(true, &ConnectionStatus::NotConfigured), Observed::Changed(change_id(None)));
        let err = ConnectionStatus::Error { message: "JSONC".into() };
        assert_eq!(observe(true, &err), Observed::Unknown);
    }

    #[test]
    fn change_ids_are_stable_hex_and_never_the_command() {
        let id = change_id(Some("npx ccstatusline"));
        assert_eq!(id, change_id(Some("npx ccstatusline")));
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(id, change_id(Some("npx ccstatusline --v2")));
        assert_ne!(change_id(None), change_id(Some("")));
    }

    #[test]
    fn one_toast_per_change() {
        let mut file = WatchdogFile::default();
        let a = Observed::Changed("a".into());
        let first = decide(&a, true, &file);
        assert_eq!(first, Outcome { lost: Some(true), toast: true, warned: Some("a".into()) });
        file.warned = first.warned;
        // The 5-min re-check (or a restart) keeps the banner but does not toast again.
        assert_eq!(decide(&a, true, &file), Outcome { lost: Some(true), toast: false, warned: Some("a".into()) });
        // A different change toasts again.
        let b = decide(&Observed::Changed("b".into()), true, &file);
        assert!(b.toast && b.lost == Some(true));
    }

    #[test]
    fn reconnecting_clears_and_rearms() {
        let file = WatchdogFile {
            dismissed: vec![],
            warned: Some("a".into()),
        };
        for o in [Observed::Connected, Observed::NotWrapped] {
            assert_eq!(decide(&o, true, &file), Outcome { lost: Some(false), toast: false, warned: None });
        }
    }

    #[test]
    fn dismissed_changes_stay_quiet() {
        let mut file = WatchdogFile::default();
        file.dismiss("a");
        let out = decide(&Observed::Changed("a".into()), true, &file);
        assert_eq!(out.lost, Some(false));
        assert!(!out.toast);
    }

    #[test]
    fn unknown_keeps_the_flag_and_off_clears_it() {
        let file = WatchdogFile {
            dismissed: vec![],
            warned: Some("a".into()),
        };
        assert_eq!(decide(&Observed::Unknown, true, &file), Outcome { lost: None, toast: false, warned: Some("a".into()) });
        let off = decide(&Observed::Changed("b".into()), false, &file);
        assert_eq!(off, Outcome { lost: Some(false), toast: false, warned: Some("a".into()) });
    }

    #[test]
    fn dismissed_list_is_deduplicated_and_capped() {
        let mut file = WatchdogFile::default();
        file.dismiss("x");
        file.dismiss("x");
        assert_eq!(file.dismissed, ["x"]);
        for i in 0..KEEP_DISMISSED + 5 {
            file.dismiss(&format!("{i}"));
        }
        assert_eq!(file.dismissed.len(), KEEP_DISMISSED);
        assert_eq!(file.dismissed.last().map(String::as_str), Some("36"));
        assert!(!file.dismissed.contains(&"x".to_owned()), "oldest dropped first");
    }

    #[test]
    fn watchdog_file_roundtrips_and_tolerates_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(FILE_NAME);
        assert_eq!(load_json::<WatchdogFile>(&p), WatchdogFile::default());
        let file = WatchdogFile {
            dismissed: vec!["00ff".into()],
            warned: Some("00ff".into()),
        };
        save_json(&p, &file).unwrap();
        assert_eq!(load_json::<WatchdogFile>(&p), file);
        std::fs::write(&p, b"{").unwrap();
        assert_eq!(load_json::<WatchdogFile>(&p), WatchdogFile::default());
    }

    fn paths(tmp: &Path) -> Paths {
        Paths::with_roots(tmp.join("claude"), vec![], tmp.join("data"))
    }

    #[test]
    fn a_rewritten_status_line_is_seen_and_a_disconnect_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::create_dir_all(p.claude_home()).unwrap();
        // Never connected: a foreign status line is not our business.
        std::fs::write(p.claude_settings(), br#"{"statusLine":{"type":"command","command":"my-line"}}"#).unwrap();
        assert_eq!(observe(wrapped(&p), &connect::status(&p)), Observed::NotWrapped);
        // Connect had recorded a wrap; the file now holds someone else's command.
        let record = WrapRecord {
            original_command: Some("my-line".into()),
            original_command_raw: Some("\"my-line\"".into()),
            empty_object_ws: None,
            shim_path: "C:/data/bin/cuw-capture.exe".into(),
            mode: WrapMode::PipeGrouped,
            shell: cuw_core::cmdline::ShellKind::Bash,
            at_ms: 1,
        };
        save_json(&p.wrap_file(), &record).unwrap();
        assert_eq!(observe(wrapped(&p), &connect::status(&p)), Observed::Changed(change_id(Some("my-line"))));
        // Disconnect removes wrap.json: quiet again.
        std::fs::remove_file(p.wrap_file()).unwrap();
        assert_eq!(observe(wrapped(&p), &connect::status(&p)), Observed::NotWrapped);
    }

    #[test]
    fn only_settings_json_events_count() {
        let settings = Path::new("C:/h/.claude/settings.json");
        let ev = |kind, path: &str| notify::Event {
            kind,
            paths: vec![path.into()],
            attrs: Default::default(),
        };
        let modify = EventKind::Modify(notify::event::ModifyKind::Any);
        assert!(is_settings_event(settings, &ev(modify, "C:/h/.claude/settings.json")));
        assert!(is_settings_event(settings, &ev(modify, "C:/h/.claude/Settings.JSON")));
        assert!(!is_settings_event(settings, &ev(modify, "C:/h/.claude/settings.local.json")));
        assert!(!is_settings_event(settings, &ev(modify, "C:/h/.claude/history.jsonl")));
        let access = EventKind::Access(notify::event::AccessKind::Any);
        assert!(!is_settings_event(settings, &ev(access, "C:/h/.claude/settings.json")));
    }
}
