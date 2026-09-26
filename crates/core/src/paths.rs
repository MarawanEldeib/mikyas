//! The only module that knows where things live on disk. Every other module receives
//! paths from a [`Paths`] value so tests can point everything at a temp directory.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Directory name of the widget's own data under the OS local-data dir.
pub const APP_DIR_NAME: &str = "ClaudeUsageWidget";
/// Directory under the data root where the shim writes statusline captures.
const CAPTURE_DIR_NAME: &str = "capture";
/// Env var that overrides the widget data root (tests, dev builds).
pub const DATA_DIR_ENV: &str = "CUW_DATA_DIR";
/// Claude Code's own override for `~/.claude`.
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    claude_home: PathBuf,
    desktop_roots: Vec<PathBuf>,
    data_root: PathBuf,
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
        }
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

/// The [`DATA_DIR_ENV`] value unless unset or empty, else `<local data dir>/ClaudeUsageWidget`.
fn data_root_from(env_value: Option<OsString>, local_dir: impl FnOnce() -> PathBuf) -> PathBuf {
    env_value
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| local_dir().join(APP_DIR_NAME))
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

    #[test]
    fn data_root_honours_the_env_override_unless_empty() {
        let local = || PathBuf::from("/local");
        let default = PathBuf::from("/local/ClaudeUsageWidget");
        assert_eq!(
            data_root_from(Some("/override".into()), || unreachable!("override wins")),
            PathBuf::from("/override")
        );
        assert_eq!(data_root_from(Some(OsString::new()), local), default);
        assert_eq!(data_root_from(None, local), default);
    }

    #[test]
    fn fast_capture_dir_matches_the_full_detection() {
        assert_eq!(Paths::detect_capture_dir(), Paths::detect().capture_dir());
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
