//! The only module that knows where things live on disk. Every other module receives
//! paths from a [`Paths`] value so tests can point everything at a temp directory.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};

/// Directory name of the widget's own data under the OS local-data dir.
pub const APP_DIR_NAME: &str = "ClaudeUsageWidget";
/// Directory under the data root where the shim writes statusline captures.
const CAPTURE_DIR_NAME: &str = "capture";
/// Env var that overrides the widget data root (tests, dev builds, Connect's self-test). Only an
/// absolute local path without `..` is honoured (see [`data_root_from`]).
pub const DATA_DIR_ENV: &str = "CUW_DATA_DIR";
/// Claude Code's own override for `~/.claude`.
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    claude_home: PathBuf,
    desktop_roots: Vec<PathBuf>,
    data_root: PathBuf,
    /// The Desktop roots came from [`detect_desktop_roots`] (and may be refreshed).
    detected_desktop: bool,
}

impl Paths {
    /// Resolves real locations for the current user.
    pub fn detect() -> Self {
        let claude_home = std::env::var_os(CLAUDE_CONFIG_DIR_ENV)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| home_dir().join(".claude"));
        Self {
            claude_home,
            desktop_roots: detect_desktop_roots(),
            data_root: detect_data_root(),
            detected_desktop: true,
        }
    }

    /// Detects the Desktop roots again (Claude Desktop's MSIX package can be installed, moved or
    /// removed while the widget runs; this only lists entry names). Returns whether they changed,
    /// in which case a [`crate::saferead::SafeReader`] built from the old value must be rebuilt.
    /// Explicit roots ([`Self::with_roots`]) are kept as they are.
    pub fn refresh_desktop_roots(&mut self) -> bool {
        if !self.detected_desktop {
            return false;
        }
        let roots = detect_desktop_roots();
        let changed = roots != self.desktop_roots;
        self.desktop_roots = roots;
        changed
    }

    /// Only [`Self::capture_dir`], for the capture shim: it runs on every statusline update and
    /// needs nothing else, while [`Self::detect`] also lists `%LOCALAPPDATA%\Packages`.
    pub fn detect_capture_dir() -> PathBuf {
        detect_data_root().join(CAPTURE_DIR_NAME)
    }

    /// Explicit locations, for tests.
    pub fn with_roots(claude_home: PathBuf, desktop_roots: Vec<PathBuf>, data_root: PathBuf) -> Self {
        Self {
            claude_home,
            desktop_roots,
            data_root,
            detected_desktop: false,
        }
    }

    // ---- Claude Code ----
    pub fn claude_home(&self) -> &Path {
        &self.claude_home
    }
    pub fn claude_settings(&self) -> PathBuf {
        self.claude_home.join("settings.json")
    }
    pub fn projects_dir(&self) -> PathBuf {
        self.claude_home.join("projects")
    }

    // ---- Claude Desktop ----
    /// Candidate Desktop data roots (may not exist).
    pub fn desktop_roots(&self) -> &[PathBuf] {
        &self.desktop_roots
    }
    /// Existing `plan-usage-history.json` files, most specific (non-MSIX) root first.
    pub fn desktop_usage_files(&self) -> Vec<PathBuf> {
        self.existing_under_roots("plan-usage-history.json")
    }
    /// Existing `claude-code-sessions` dirs (Desktop Code-tab session metadata).
    pub fn desktop_sessions_dirs(&self) -> Vec<PathBuf> {
        self.existing_under_roots("claude-code-sessions")
    }
    /// Existing `local-agent-mode-sessions` dirs (Cowork transcripts live below these).
    pub fn cowork_dirs(&self) -> Vec<PathBuf> {
        self.existing_under_roots("local-agent-mode-sessions")
    }

    fn existing_under_roots(&self, name: &str) -> Vec<PathBuf> {
        self.desktop_roots
            .iter()
            .map(|r| r.join(name))
            .filter(|p| p.exists())
            .collect()
    }

    // ---- widget data ----
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }
    pub fn capture_dir(&self) -> PathBuf {
        self.data_root.join(CAPTURE_DIR_NAME)
    }
    pub fn bin_dir(&self) -> PathBuf {
        self.data_root.join("bin")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.data_root.join("backups")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_root.join("logs")
    }
    pub fn history_file(&self) -> PathBuf {
        self.data_root.join("history.jsonl")
    }
    pub fn state_file(&self) -> PathBuf {
        self.data_root.join("state.json")
    }
    pub fn alerts_file(&self) -> PathBuf {
        self.data_root.join("alerts.json")
    }
    pub fn wrap_file(&self) -> PathBuf {
        self.data_root.join("wrap.json")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.data_root.join("settings.json")
    }
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn detect_data_root() -> PathBuf {
    data_root_from(std::env::var_os(DATA_DIR_ENV), || {
        dirs::data_local_dir().unwrap_or_else(|| home_dir().join(".local").join("share"))
    })
}

/// The [`DATA_DIR_ENV`] value if it is a usable override ([`valid_override`]), else
/// `<local data dir>/ClaudeUsageWidget`.
fn data_root_from(env_value: Option<OsString>, local_dir: impl FnOnce() -> PathBuf) -> PathBuf {
    env_value
        .map(PathBuf::from)
        .filter(|p| valid_override(p))
        .unwrap_or_else(|| local_dir().join(APP_DIR_NAME))
}

/// An override must be an absolute local path without `..`: the data root decides where Connect
/// installs the capture helper that Claude Code runs, so a relative path (resolved against
/// whatever the working directory is), a network share (`\\server\share`, `\\?\UNC\…`, device
/// paths) or a path climbing out of its stated directory is ignored.
fn valid_override(path: &Path) -> bool {
    let local = match path.components().next() {
        Some(Component::Prefix(prefix)) => matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
        Some(Component::RootDir) => !cfg!(windows),
        _ => false,
    };
    // A verbatim (`\\?\`) path is not normalised: its `.` stays a plain component, so compare the
    // text as well.
    let dots = |c: Component<'_>| {
        matches!(c, Component::ParentDir | Component::CurDir) || c.as_os_str() == ".." || c.as_os_str() == "."
    };
    local && path.is_absolute() && !path.components().any(dots)
}

/// `<config_dir>/Claude` on every OS (`%APPDATA%\Claude`, `~/Library/Application Support/Claude`,
/// `~/.config/Claude`), plus MSIX-virtualised copies on Windows.
fn detect_desktop_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(cfg) = dirs::config_dir() {
        roots.push(cfg.join("Claude"));
    }
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        if let Ok(entries) = std::fs::read_dir(local.join("Packages")) {
            let mut msix: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
                .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude"))
                .collect();
            msix.sort();
            roots.extend(msix);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_derived_from_roots() {
        let p = Paths::with_roots("/h/.claude".into(), vec!["/d/Claude".into()], "/data/cuw".into());
        assert_eq!(p.claude_settings(), PathBuf::from("/h/.claude/settings.json"));
        assert_eq!(p.projects_dir(), PathBuf::from("/h/.claude/projects"));
        assert_eq!(p.capture_dir(), PathBuf::from("/data/cuw/capture"));
        assert_eq!(p.history_file(), PathBuf::from("/data/cuw/history.jsonl"));
        assert!(p.desktop_usage_files().is_empty(), "non-existent roots are filtered");
    }

    /// An absolute local path on this platform.
    fn absolute(rest: &str) -> PathBuf {
        if cfg!(windows) { PathBuf::from(format!(r"C:\{rest}")) } else { PathBuf::from(format!("/{rest}")) }
    }

    #[test]
    fn data_root_honours_the_env_override_unless_empty() {
        let local = || PathBuf::from("/local");
        let default = PathBuf::from("/local/ClaudeUsageWidget");
        let override_dir = absolute("override");
        assert_eq!(
            data_root_from(Some(override_dir.clone().into()), || unreachable!("override wins")),
            override_dir
        );
        assert_eq!(data_root_from(Some(OsString::new()), local), default);
        assert_eq!(data_root_from(None, local), default);
    }

    #[test]
    fn data_root_override_must_be_an_absolute_local_path() {
        let local = || PathBuf::from("/local");
        let default = PathBuf::from("/local/ClaudeUsageWidget");
        for bad in [
            "relative",
            r".\here",
            r"\\server\share\cuw",
            r"\\?\UNC\server\share\cuw",
            r"\\.\pipe\cuw",
        ] {
            assert_eq!(data_root_from(Some(bad.into()), local), default, "{bad}");
        }
        let climbing = absolute("x").join("..").join("y");
        assert_eq!(data_root_from(Some(climbing.into()), local), default);
        if cfg!(windows) {
            assert_eq!(data_root_from(Some(r"\rooted-no-drive".into()), local), default);
            assert_eq!(data_root_from(Some(r"C:relative".into()), local), default);
            // Verbatim paths are not normalised: `.` and `..` there are refused all the same.
            assert_eq!(data_root_from(Some(r"\\?\C:\x\..\y".into()), local), default);
            assert_eq!(data_root_from(Some(r"\\?\C:\x\.\y".into()), local), default);
            let verbatim = PathBuf::from(r"\\?\C:\cuw");
            assert_eq!(data_root_from(Some(verbatim.clone().into()), local), verbatim);
        }
    }

    #[test]
    fn capture_dir_is_below_the_data_root() {
        let root = data_root_from(Some(absolute("data").into()), || unreachable!());
        let p = Paths::with_roots(absolute("h"), vec![], root.clone());
        assert_eq!(p.capture_dir(), root.join(CAPTURE_DIR_NAME));
    }

    #[test]
    fn explicit_desktop_roots_are_not_refreshed() {
        let mut p = Paths::with_roots("/h/.claude".into(), vec!["/d/Claude".into()], "/data".into());
        assert!(!p.refresh_desktop_roots());
        assert_eq!(p.desktop_roots(), [PathBuf::from("/d/Claude")]);
    }

    #[test]
    fn existing_desktop_files_are_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Claude");
        std::fs::create_dir_all(root.join("claude-code-sessions")).unwrap();
        std::fs::write(root.join("plan-usage-history.json"), "{}").unwrap();
        let p = Paths::with_roots(tmp.path().join(".claude"), vec![root.clone()], tmp.path().join("data"));
        assert_eq!(p.desktop_usage_files(), vec![root.join("plan-usage-history.json")]);
        assert_eq!(p.desktop_sessions_dirs(), vec![root.join("claude-code-sessions")]);
        assert!(p.cowork_dirs().is_empty());
    }
}
