//! Opt-in update checker: the app's ONLY network access, off by default (`check_updates`).
//!
//! When enabled it asks the GitHub Releases API for this repository's latest release at most once
//! every 24 h, plus whenever the user clicks "Check now". The request is made by Windows' own
//! `curl.exe` (System32), so the app contains no HTTP client; it sends nothing but the app
//! version (in the User-Agent) and reads only `tag_name` and `html_url` of the answer. A newer
//! version sets `UiState.update` and is announced with one toast per version. The last check (and
//! the release it found, restored at startup) is kept in `<data_root>/update-check.json` (not
//! `state.json`: the pipeline thread rewrites that file from its own copy). Only release pages of
//! this repository can be opened.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fmt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cuw_core::time::{DAY_MS, Ms, now_ms};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::state::{Shared, UpdateInfo, load_json, lock, save_json};

/// The GitHub repository (`owner/name`) whose releases are checked.
macro_rules! repo {
    () => {
        "MarawanEldeib/claude-usage-widget"
    };
}

const LATEST_API: &str = concat!("https://api.github.com/repos/", repo!(), "/releases/latest");
/// Every release page of the repository starts with this; nothing else is ever opened.
pub const RELEASES_PREFIX: &str = concat!("https://github.com/", repo!(), "/releases/");
/// This build's version.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Minimum time between two automatic checks.
pub const CHECK_EVERY_MS: Ms = DAY_MS;
const RECORD_FILE: &str = "update-check.json";
/// The first automatic check waits for the app to settle.
const STARTUP_DELAY: Duration = Duration::from_secs(30);
/// Longest sleep while enabled (sleep/resume, clock changes), and the retry delay after a check
/// that could not reach GitHub.
const RECHECK: Duration = Duration::from_secs(60 * 60);
/// Sleep while disabled (enabling the check wakes the thread at once).
const IDLE: Duration = Duration::from_secs(6 * 60 * 60);
/// `CREATE_NO_WINDOW`: no console window flashes up for curl.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------------------------------------
// Versions

/// A semantic version: `MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]`, with an optional leading "v".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreId>,
}

/// Pre-release identifier: numeric ones sort numerically and below alphanumeric ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreId {
    Num(u64),
    Alpha(String),
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(version, _build)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let number = |part: Option<&str>| {
            part.filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|p| p.parse::<u64>().ok())
        };
        let mut parts = core.split('.');
        let (major, minor, patch) = (number(parts.next())?, number(parts.next())?, number(parts.next())?);
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|id| {
                    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                        None
                    } else if id.bytes().all(|b| b.is_ascii_digit()) {
                        id.parse().ok().map(PreId::Num)
                    } else {
                        Some(PreId::Alpha(id.to_owned()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                // A release sorts above its own pre-releases.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, id) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match id {
                PreId::Num(n) => write!(f, "{n}")?,
                PreId::Alpha(s) => f.write_str(s)?,
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Checking

/// Why a check failed (the text is shown next to "Check now").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    NoReleases,
    RateLimited,
    Http(u16),
    BadResponse,
    BadTag(String),
    Offline,
    Timeout,
    CurlMissing,
    Curl(String),
    NotReady,
}

impl CheckError {
    /// GitHub answered (so the daily check counts as done).
    fn reached_github(&self) -> bool {
        matches!(self, Self::NoReleases | Self::RateLimited | Self::Http(_) | Self::BadResponse | Self::BadTag(_))
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoReleases => f.write_str("No releases published yet"),
            Self::RateLimited => f.write_str("GitHub is rate-limiting requests; try again later"),
            Self::Http(code) => write!(f, "GitHub answered HTTP {code}"),
            Self::BadResponse => f.write_str("Unexpected answer from GitHub"),
            Self::BadTag(tag) => write!(f, "The latest release has an unrecognised version tag ({tag})"),
            Self::Offline => f.write_str("Couldn't reach GitHub; check your connection"),
            Self::Timeout => f.write_str("GitHub didn't answer in time"),
            Self::CurlMissing => f.write_str("curl.exe was not found (it ships with Windows 10 1803 and later)"),
            Self::Curl(detail) => write!(f, "Update check failed: {detail}"),
            Self::NotReady => f.write_str("The update checker is still starting"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
}

/// Interprets the API answer: `Ok(Some)` for a newer release, `Ok(None)` when up to date (or the
/// release page is not one of this repository's).
pub fn interpret(status: u16, body: &[u8], current: &Version) -> Result<Option<UpdateInfo>, CheckError> {
    match status {
        200 => {}
        404 => return Err(CheckError::NoReleases),
        403 | 429 => return Err(CheckError::RateLimited),
        other => return Err(CheckError::Http(other)),
    }
    let release: Release = serde_json::from_slice(body).map_err(|_| CheckError::BadResponse)?;
    let latest = Version::parse(&release.tag_name)
        .ok_or_else(|| CheckError::BadTag(release.tag_name.chars().take(40).collect()))?;
    if release.draft || !is_release_url(&release.html_url) || latest <= *current {
        return Ok(None);
    }
    Ok(Some(UpdateInfo {
        version: latest.to_string(),
        url: release.html_url,
    }))
}

/// A release page of this repository, with nothing that a browser or `explorer.exe` could read as
/// more than a plain path (no query, fragment, escapes, quotes, spaces or dot segments).
pub fn is_release_url(url: &str) -> bool {
    url.len() <= 256
        && url.strip_prefix(RELEASES_PREFIX).is_some_and(|rest| {
            !rest.is_empty()
                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~/+".contains(&b))
                && !rest.split('/').any(|segment| segment == "." || segment == "..")
        })
}

/// Splits curl's output into the body and the status code appended by `--write-out`.
pub fn split_status(output: &[u8]) -> Option<(&[u8], u16)> {
    let newline = output.iter().rposition(|&b| b == b'\n')?;
    let code = std::str::from_utf8(&output[newline + 1..]).ok()?.trim().parse().ok()?;
    Some((&output[..newline], code))
}

/// Maps a failed curl run (exit code, stderr) to an error.
pub fn curl_error(code: Option<i32>, stderr: &[u8]) -> CheckError {
    match code {
        // Could not resolve the proxy / host, could not connect.
        Some(5..=7) => CheckError::Offline,
        Some(28) => CheckError::Timeout,
        _ => {
            let text = String::from_utf8_lossy(stderr);
            let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
            let detail: String = line.chars().take(120).collect();
            CheckError::Curl(match (detail.is_empty(), code) {
                (false, _) => detail,
                (true, Some(c)) => format!("curl exited with code {c}"),
                (true, None) => "curl was stopped".into(),
            })
        }
    }
}

/// The automatic check is due (also when the clock went backwards past the last check).
pub fn due(last_check_ms: Ms, now: Ms) -> bool {
    last_check_ms <= 0 || last_check_ms > now || now - last_check_ms >= CHECK_EVERY_MS
}

/// How long the scheduler sleeps after (possibly) checking.
pub fn next_wait(last_check_ms: Ms, now: Ms) -> Duration {
    if due(last_check_ms, now) {
        return RECHECK;
    }
    let left = (last_check_ms + CHECK_EVERY_MS - now).max(0) as u64;
    Duration::from_millis(left).min(RECHECK)
}

/// curl's arguments: one HTTPS GET of the latest-release endpoint.
pub fn curl_args() -> Vec<String> {
    [
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--max-time",
        "10",
        "--max-filesize",
        "2000000",
        "--header",
        "Accept: application/vnd.github+json",
        "--header",
        concat!("User-Agent: claude-usage-widget/", env!("CARGO_PKG_VERSION")),
        "--header",
        "X-GitHub-Api-Version: 2022-11-28",
        // curl expands the "\n" itself, so the command line stays on one line.
        "--write-out",
        "\\n%{http_code}",
        LATEST_API,
    ]
    .map(String::from)
    .to_vec()
}

/// Windows' own curl (`<system_root>\System32\curl.exe`), never whatever `curl` comes first on
/// the PATH; missing (Windows before 10 1803) → a friendly error.
fn system_curl(system_root: Option<&OsStr>) -> Result<PathBuf, CheckError> {
    let root = system_root.map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let exe = root.join("System32").join("curl.exe");
    if exe.is_file() { Ok(exe) } else { Err(CheckError::CurlMissing) }
}

fn fetch() -> Result<(u16, Vec<u8>), CheckError> {
    let mut cmd = Command::new(system_curl(std::env::var_os("SystemRoot").as_deref())?);
    cmd.args(curl_args()).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CheckError::CurlMissing,
        _ => CheckError::Curl(e.to_string()),
    })?;
    if !out.status.success() {
        return Err(curl_error(out.status.code(), &out.stderr));
    }
    let (body, status) = split_status(&out.stdout).ok_or(CheckError::BadResponse)?;
    Ok((status, body.to_vec()))
}

// ---------------------------------------------------------------------------------------------
// App wiring

/// Persisted in `<data_root>/update-check.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckRecord {
    /// Last check that got an answer from GitHub.
    pub last_check_ms: Ms,
    /// Newest version already announced (toast or "Check now"), so each one is announced once.
    pub notified_version: Option<String>,
    /// The newer release the last answered check found, shown again after a restart.
    pub update: Option<UpdateInfo>,
}

impl CheckRecord {
    /// Records a finished check; returns the release to announce with a toast, if any.
    pub fn record(&mut self, result: &Result<Option<UpdateInfo>, CheckError>, now: Ms, toast: bool) -> Option<UpdateInfo> {
        let reached = match result {
            Ok(_) => true,
            Err(e) => e.reached_github(),
        };
        if reached {
            self.last_check_ms = now;
        }
        if let Ok(update) = result {
            self.update = update.clone();
        }
        let Ok(Some(info)) = result else { return None };
        if self.notified_version.as_deref() == Some(info.version.as_str()) {
            return None;
        }
        self.notified_version = Some(info.version.clone());
        toast.then(|| info.clone())
    }
}

/// The release a previous run found, while it is still newer than this build (it may have been
/// installed since) and its page is one of this repository's (the file could have been edited).
pub fn remembered(record: &CheckRecord, current: &Version) -> Option<UpdateInfo> {
    let info = record.update.as_ref()?;
    let newer = Version::parse(&info.version).is_some_and(|v| v > *current);
    (newer && is_release_url(&info.url)).then(|| info.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Daily,
    Manual,
}

struct Updater {
    waker: Mutex<Sender<()>>,
    record: Mutex<CheckRecord>,
    path: PathBuf,
    /// Serialises the daily check and "Check now".
    busy: Mutex<()>,
}

/// Starts the daily-check thread (it only goes online while `check_updates` is on).
pub fn start(app: &AppHandle, shared: Arc<Shared>) -> std::io::Result<()> {
    let path = shared.paths.data_root().join(RECORD_FILE);
    let record: CheckRecord = load_json(&path);
    // Keeps the banner and the Settings status across restarts (the next check may be a day off).
    // The page has not loaded yet, so it reads this with `get_ui_state`.
    if let Some(current) = Version::parse(CURRENT_VERSION) {
        shared.ui().update = remembered(&record, &current);
    }
    let (tx, rx) = mpsc::channel();
    app.manage(Updater {
        waker: Mutex::new(tx),
        record: Mutex::new(record),
        path,
        busy: Mutex::new(()),
    });
    let app = app.clone();
    std::thread::Builder::new()
        .name("cuw-updates".into())
        .spawn(move || run(&app, &shared, &rx))?;
    Ok(())
}

/// Re-evaluates at once (the setting changed).
pub fn wake(app: &AppHandle) {
    if let Some(updater) = app.try_state::<Updater>() {
        let _ = lock(&updater.waker).send(());
    }
}

fn run(app: &AppHandle, shared: &Shared, rx: &Receiver<()>) {
    // Let the app settle first; switching the check on meanwhile starts it at once.
    if let Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(STARTUP_DELAY) {
        return;
    }
    loop {
        if shared.quitting.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        let wait = if shared.settings().check_updates {
            let last = || app.try_state::<Updater>().map_or(0, |u| lock(&u.record).last_check_ms);
            if due(last(), now_ms()) {
                if let Err(e) = check(app, shared, Trigger::Daily) {
                    crate::pipeline::log(&format!("update check: {e}"));
                }
            }
            next_wait(last(), now_ms())
        } else {
            IDLE
        };
        match rx.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn check(app: &AppHandle, shared: &Shared, trigger: Trigger) -> Result<Option<UpdateInfo>, CheckError> {
    let updater = app.try_state::<Updater>().ok_or(CheckError::NotReady)?;
    let _busy = lock(&updater.busy);
    let current = Version::parse(CURRENT_VERSION).ok_or(CheckError::NotReady)?;
    let result = fetch().and_then(|(status, body)| interpret(status, &body, &current));
    let announce = {
        let mut record = lock(&updater.record);
        let before = record.clone();
        let announce = record.record(&result, now_ms(), trigger == Trigger::Daily);
        if *record != before {
            if let Err(e) = save_json(&updater.path, &*record) {
                crate::pipeline::log(&format!("{RECORD_FILE} write failed: {e}"));
            }
        }
        announce
    };
    if let Ok(update) = &result {
        shared.ui().update = update.clone();
        crate::window::emit_ui(app, shared);
    }
    if let Some(info) = announce {
        crate::notify::show(
            app,
            &format!("Claude Usage Widget {} is available", info.version),
            "Click View on the widget to open the release page.",
        );
    }
    result
}

/// Checks now (explicit user action; allowed even when the daily check is off).
#[tauri::command]
pub async fn check_updates_now(app: AppHandle, shared: State<'_, Arc<Shared>>) -> Result<Option<UpdateInfo>, String> {
    let shared = shared.inner().clone();
    tauri::async_runtime::spawn_blocking(move || check(&app, &shared, Trigger::Manual))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Opens a release page of this repository in the default browser; any other URL is refused.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !is_release_url(&url) {
        return Err("Only this app's GitHub release pages can be opened".into());
    }
    Command::new("explorer")
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s} should parse"))
    }

    #[test]
    fn parses_versions() {
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(v("V0.10.0").to_string(), "0.10.0");
        assert_eq!(v(" 1.2.3+build.7 ").to_string(), "1.2.3");
        assert_eq!(v("1.0.0-beta.2").to_string(), "1.0.0-beta.2");
        assert_eq!(v("1.0.0-alpha-1.x").to_string(), "1.0.0-alpha-1.x");
        assert!(Version::parse(CURRENT_VERSION).is_some(), "this build's own version");
        for garbage in ["", "v", "latest", "1", "1.2", "1.2.3.4", "1..3", "1.2.x", "-1.2.3", "1.2.3-", "1.2.3-a..b", "1.2.3-ä", "v1.2.3 beta", "99999999999999999999.0.0"] {
            assert_eq!(Version::parse(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn orders_like_semver() {
        let ordered = [
            "0.9.9", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11",
            "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.2.3+a").cmp(&v("v1.2.3+b")), Ordering::Equal, "build metadata is ignored");
    }

    #[test]
    fn newer_than_this_build() {
        let page = |tag: &str| body(tag, &format!("{RELEASES_PREFIX}tag/{tag}"));
        let newer = |tag: &str, current: &str| matches!(interpret(200, &page(tag), &v(current)), Ok(Some(_)));
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("0.1.1", "0.1.0"));
        assert!(newer("0.1.0", "0.1.0-beta.1"), "the release after its pre-release");
        assert!(newer("v0.2.0-rc.1", "0.1.0"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
        assert!(!newer("0.2.0-rc.1", "0.2.0"));
        assert!(!newer("garbage", "0.1.0"));
    }

    fn body(tag: &str, url: &str) -> Vec<u8> {
        serde_json::json!({ "tag_name": tag, "html_url": url, "draft": false, "prerelease": false, "body": "notes" })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn interprets_answers() {
        let current = v("0.1.0");
        let page = format!("{RELEASES_PREFIX}tag/v0.2.0");
        assert_eq!(
            interpret(200, &body("v0.2.0", &page), &current),
            Ok(Some(UpdateInfo {
                version: "0.2.0".into(),
                url: page.clone()
            }))
        );
        assert_eq!(interpret(200, &body("v0.1.0", &page), &current), Ok(None), "up to date");
        assert_eq!(interpret(200, &body("v0.0.5", &page), &current), Ok(None));
        // A release page elsewhere is ignored.
        assert_eq!(interpret(200, &body("v9.0.0", "https://example.com/releases/tag/v9.0.0"), &current), Ok(None));
        assert_eq!(interpret(404, b"{\"message\":\"Not Found\"}", &current), Err(CheckError::NoReleases));
        assert_eq!(CheckError::NoReleases.to_string(), "No releases published yet");
        assert_eq!(interpret(403, b"{}", &current), Err(CheckError::RateLimited));
        assert_eq!(interpret(429, b"", &current), Err(CheckError::RateLimited));
        assert_eq!(interpret(502, b"", &current), Err(CheckError::Http(502)));
        assert_eq!(interpret(200, b"<html>", &current), Err(CheckError::BadResponse));
        assert_eq!(interpret(200, b"{\"tag_name\":1}", &current), Err(CheckError::BadResponse));
        assert_eq!(interpret(200, &body("nightly", &page), &current), Err(CheckError::BadTag("nightly".into())));
        let draft = serde_json::json!({ "tag_name": "v0.3.0", "html_url": page, "draft": true }).to_string();
        assert_eq!(interpret(200, draft.as_bytes(), &current), Ok(None));
    }

    #[test]
    fn only_this_repositorys_release_pages_are_allowed() {
        assert_eq!(RELEASES_PREFIX, "https://github.com/MarawanEldeib/claude-usage-widget/releases/");
        for ok in [
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v0.2.0",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1.0.0-beta.1",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/latest",
        ] {
            assert!(is_release_url(ok), "{ok}");
        }
        for bad in [
            "",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases",
            "http://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1.0.0",
            "https://github.com/someone-else/claude-usage-widget/releases/tag/v1.0.0",
            "https://github.com/MarawanEldeib/claude-usage-widget/issues/1",
            "https://github.com.evil.example/MarawanEldeib/claude-usage-widget/releases/tag/v1",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/../../../evil",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/%2e%2e",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1?x=1",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1#top",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1 --flag",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1\"",
            "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v1,/select",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "https://github.com/MARAWANELDEIB/claude-usage-widget/releases/tag/v1.0.0",
        ] {
            assert!(!is_release_url(bad), "{bad}");
        }
        let long = format!("{RELEASES_PREFIX}tag/{}", "a".repeat(300));
        assert!(!is_release_url(&long));
        assert!(open_url("https://example.com/".into()).is_err());
    }

    #[test]
    fn curl_output_and_errors() {
        assert_eq!(split_status(b"{\"a\":1}\n200"), Some((&b"{\"a\":1}"[..], 200)));
        assert_eq!(split_status(b"{\"a\":1}\n\n404"), Some((&b"{\"a\":1}\n"[..], 404)));
        assert_eq!(split_status(b"\n200"), Some((&b""[..], 200)));
        assert_eq!(split_status(b"no status"), None);
        assert_eq!(split_status(b"body\nabc"), None);
        assert_eq!(curl_error(Some(6), b"curl: (6) Could not resolve host: api.github.com"), CheckError::Offline);
        assert_eq!(curl_error(Some(7), b""), CheckError::Offline);
        assert_eq!(curl_error(Some(28), b""), CheckError::Timeout);
        assert_eq!(
            curl_error(Some(35), b"\ncurl: (35) schannel: next InitializeSecurityContext failed\n"),
            CheckError::Curl("curl: (35) schannel: next InitializeSecurityContext failed".into())
        );
        assert_eq!(curl_error(Some(63), b""), CheckError::Curl("curl exited with code 63".into()));
        assert_eq!(curl_error(None, b""), CheckError::Curl("curl was stopped".into()));
    }

    #[test]
    fn curl_makes_one_https_request_with_no_credentials() {
        let args = curl_args();
        let urls: Vec<&String> = args.iter().filter(|a| a.contains("://")).collect();
        assert_eq!(urls, ["https://api.github.com/repos/MarawanEldeib/claude-usage-widget/releases/latest"]);
        assert!(args.windows(2).any(|w| w[0] == "--proto" && w[1] == "=https"));
        assert!(args.contains(&format!("User-Agent: claude-usage-widget/{CURRENT_VERSION}")));
        assert!(args.iter().all(|a| !a.to_ascii_lowercase().contains("authorization")));
        assert!(args.iter().all(|a| !a.contains(['\n', '\r', '"'])), "single-line, unquoted arguments");
        assert!(args.windows(2).any(|w| w[0] == "--write-out" && w[1] == r"\n%{http_code}"));
        assert!(args.iter().all(|a| !["-L", "--location", "--cookie", "-b", "--user", "-u"].contains(&a.as_str())));
    }

    #[test]
    fn only_windows_own_curl_is_run() {
        let tmp = tempfile::tempdir().unwrap();
        // No System32\curl.exe: a friendly error, never some other `curl` from the PATH.
        assert_eq!(system_curl(Some(tmp.path().as_os_str())), Err(CheckError::CurlMissing));
        let exe = tmp.path().join("System32").join("curl.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(system_curl(Some(tmp.path().as_os_str())), Ok(exe));
        assert!(CheckError::CurlMissing.to_string().starts_with("curl.exe was not found"));
    }

    #[test]
    fn daily_schedule() {
        let now = 1_790_000_000_000;
        assert!(due(0, now), "never checked");
        assert!(due(now - CHECK_EVERY_MS, now));
        assert!(!due(now - CHECK_EVERY_MS + 1, now));
        assert!(due(now + 60_000, now), "clock went backwards");
        assert_eq!(next_wait(now - 1_000, now), RECHECK, "capped at an hour");
        assert_eq!(next_wait(now - CHECK_EVERY_MS + 60_000, now), Duration::from_secs(60));
        assert_eq!(next_wait(0, now), RECHECK, "retry after a failure");
    }

    #[test]
    fn records_checks_and_announces_each_version_once() {
        let info = |version: &str| UpdateInfo {
            version: version.into(),
            url: format!("{RELEASES_PREFIX}tag/v{version}"),
        };
        let mut r = CheckRecord::default();
        assert_eq!(r.record(&Err(CheckError::Offline), 5, true), None);
        assert_eq!(r.last_check_ms, 0, "an offline attempt is retried");
        assert_eq!(r.record(&Err(CheckError::NoReleases), 6, true), None);
        assert_eq!(r.last_check_ms, 6);
        assert_eq!(r.record(&Ok(None), 7, true), None);
        assert_eq!(r.record(&Ok(Some(info("0.2.0"))), 8, true), Some(info("0.2.0")));
        assert_eq!(r.record(&Ok(Some(info("0.2.0"))), 9, true), None, "once per version");
        // "Check now" marks a version as seen without a toast.
        assert_eq!(r.record(&Ok(Some(info("0.3.0"))), 10, false), None);
        assert_eq!(r.record(&Ok(Some(info("0.3.0"))), 11, true), None);
        assert_eq!(r.notified_version.as_deref(), Some("0.3.0"));
        assert_eq!(r.last_check_ms, 11);
    }

    #[test]
    fn a_found_update_is_shown_again_after_a_restart_until_installed() {
        let info = UpdateInfo {
            version: "0.2.0".into(),
            url: format!("{RELEASES_PREFIX}tag/v0.2.0"),
        };
        let mut r = CheckRecord::default();
        assert_eq!(remembered(&r, &v("0.1.0")), None, "nothing found yet");
        r.record(&Ok(Some(info.clone())), 5, true);
        // The record is what a restart loads.
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(RECORD_FILE);
        save_json(&p, &r).unwrap();
        let back: CheckRecord = load_json(&p);
        assert_eq!(remembered(&back, &v("0.1.0")), Some(info.clone()));
        assert_eq!(remembered(&back, &v("0.2.0")), None, "installed since");
        assert_eq!(remembered(&back, &v("0.3.0")), None);
        // A check that could not reach GitHub keeps it; an up-to-date answer clears it.
        r.record(&Err(CheckError::Offline), 6, true);
        assert_eq!(remembered(&r, &v("0.1.0")), Some(info));
        r.record(&Ok(None), 7, true);
        assert_eq!(remembered(&r, &v("0.1.0")), None);
        // A hand-edited record can never smuggle in another page or a garbage version.
        for (version, url) in [
            ("9.0.0", "https://example.com/releases/tag/v9.0.0".to_owned()),
            ("latest", format!("{RELEASES_PREFIX}tag/latest")),
        ] {
            let bad = CheckRecord {
                update: Some(UpdateInfo {
                    version: version.into(),
                    url,
                }),
                ..CheckRecord::default()
            };
            assert_eq!(remembered(&bad, &v("0.1.0")), None, "{version}");
        }
    }

    #[test]
    fn record_file_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(RECORD_FILE);
        assert_eq!(load_json::<CheckRecord>(&p), CheckRecord::default());
        let r = CheckRecord {
            last_check_ms: 42,
            notified_version: Some("0.2.0".into()),
            update: Some(UpdateInfo {
                version: "0.2.0".into(),
                url: format!("{RELEASES_PREFIX}tag/v0.2.0"),
            }),
        };
        save_json(&p, &r).unwrap();
        assert_eq!(load_json::<CheckRecord>(&p), r);
        std::fs::write(&p, b"{garbage").unwrap();
        assert_eq!(load_json::<CheckRecord>(&p), CheckRecord::default());
    }
}
