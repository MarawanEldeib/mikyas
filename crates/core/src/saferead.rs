//! File-read allowlist. Every read of Claude Code's or Claude Desktop's files goes through
//! [`SafeReader`], so of those the widget can only ever open the handful it documents in
//! PRIVACY.md. Of the widget's own data root only `capture/*.json` is allowed (the capture loader
//! reads through the reader); its other files are read directly (see the crate docs). Credential,
//! cookie and browser-storage files are hard-denied even if they sit inside an allowed tree.
//!
//! - The denylist applies to the part of a path below the allowed root it matches, so a user
//!   profile that happens to live below a folder named e.g. `Network` still works.
//! - An existing file's resolved target (symlinks, junctions) must be allowed as well, checked
//!   against the rules with their roots resolved the same way. Those are computed once; a root that
//!   did not exist then is resolved through its longest existing ancestor, and when a target fails
//!   the cached rules they are resolved again once before the read is refused (a root created
//!   later behind a junction). A path that cannot be resolved for any reason but "not found" is
//!   refused.
//! - [`SafeReader::open`] checks the resolved target again after opening, which narrows (but, as
//!   std has no portable way to get the path of an open handle, cannot close) the window in which a
//!   link could be swapped between the check and the open.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use crate::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("read denied by allowlist: {0}")]
    Denied(PathBuf),
    #[error("file larger than {max} bytes")]
    TooLarge { max: u64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Path components (compared case-insensitively) that may never be opened.
const DENIED_NAMES: &[&str] = &[
    ".credentials.json",
    "credentials.json",
    ".claude.json",
    "config.json",
    "claude_desktop_config.json",
    "cookies",
    "cookies-journal",
    "login data",
    "login data-journal",
    "web data",
    "local state",
    "local storage",
    "session storage",
    "indexeddb",
    "network",
];
const DENIED_SUFFIXES: &[&str] = &["-tokens.json", ".ldb", ".sqlite", ".db"];
const DENIED_PREFIXES: &[&str] = &["ant-"];

#[derive(Debug, Clone)]
enum Rule {
    /// Files below the root with this extension (lowercase, no dot).
    TreeExt(PathBuf, &'static str),
    /// Exactly this file.
    File(PathBuf),
    /// Cowork transcripts: `*.jsonl` below the root that sit inside a `.claude/projects`
    /// directory pair (so a session's `.claude/history.jsonl` prompt history is not readable).
    CoworkTranscripts(PathBuf),
}

#[derive(Debug)]
pub struct SafeReader {
    /// Lexically normalised rules, matched against the requested path.
    rules: Vec<Rule>,
    /// The same rules with symlinks/junctions in their roots resolved, matched against the
    /// resolved target of an existing file. Computed once, and replaced when resolving them again
    /// after a miss gives a different result.
    canonical: RwLock<Vec<Rule>>,
}

impl Clone for SafeReader {
    fn clone(&self) -> Self {
        Self {
            rules: self.rules.clone(),
            canonical: RwLock::new(self.canonical_rules().clone()),
        }
    }
}

impl SafeReader {
    pub fn new(paths: &Paths) -> Self {
        let mut rules = vec![
            Rule::TreeExt(paths.capture_dir(), "json"),
            Rule::File(paths.claude_settings()),
            Rule::TreeExt(paths.projects_dir(), "jsonl"),
        ];
        for root in paths.desktop_roots() {
            rules.push(Rule::File(root.join("plan-usage-history.json")));
            rules.push(Rule::TreeExt(root.join("claude-code-sessions"), "json"));
            rules.push(Rule::CoworkTranscripts(root.join("local-agent-mode-sessions")));
        }
        let rules: Vec<Rule> = rules.iter().map(|r| r.map(normalize)).collect();
        let canonical = RwLock::new(rules.iter().map(canonical_rule).collect());
        Self { rules, canonical }
    }

    /// True if `path` may be read.
    pub fn allows(&self, path: &Path) -> bool {
        let lexical = normalize(path);
        if !self.rules.iter().any(|r| rule_matches(r, &lexical)) {
            return false;
        }
        // If the file exists, the resolved target (symlinks/junctions) must also be allowed.
        match std::fs::canonicalize(path) {
            Ok(real) => self.allows_resolved(&normalize(&real)),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        }
    }

    /// `real` (normalised, fully resolved) matches a resolved rule. The cached rules are tried
    /// first; on a miss they are resolved afresh, for a root that appeared after construction.
    fn allows_resolved(&self, real: &Path) -> bool {
        if self.canonical_rules().iter().any(|r| rule_matches(r, real)) {
            return true;
        }
        let fresh: Vec<Rule> = self.rules.iter().map(canonical_rule).collect();
        let allowed = fresh.iter().any(|r| rule_matches(r, real));
        if allowed {
            // Keep them, so later reads below that root do not resolve every rule again.
            *self.canonical.write().unwrap_or_else(PoisonError::into_inner) = fresh;
        }
        allowed
    }

    fn canonical_rules(&self) -> RwLockReadGuard<'_, Vec<Rule>> {
        self.canonical.read().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn cached_match(&self, real: &Path) -> bool {
        self.canonical_rules().iter().any(|r| rule_matches(r, real))
    }

    /// True if `dir` may be listed (it is, or is inside, an allowed tree root).
    pub fn allows_dir(&self, dir: &Path) -> bool {
        let d = normalize(dir);
        self.rules.iter().any(|r| match r {
            Rule::TreeExt(root, _) | Rule::CoworkTranscripts(root) => below_root(&d, root).is_some_and(|rest| !is_denied(rest)),
            Rule::File(f) => f.parent().is_some_and(|p| p == d),
        })
    }

    /// Opens `path` if it may be read, then checks the resolved target again (see the module docs).
    pub fn open(&self, path: &Path) -> Result<File, ReadError> {
        if !self.allows(path) {
            return Err(ReadError::Denied(path.to_path_buf()));
        }
        let file = File::open(path)?;
        let still_allowed = std::fs::canonicalize(path).is_ok_and(|real| self.allows_resolved(&normalize(&real)));
        if !still_allowed {
            return Err(ReadError::Denied(path.to_path_buf()));
        }
        Ok(file)
    }

    /// Reads the whole file, refusing files larger than `max_bytes`.
    pub fn read(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>, ReadError> {
        let file = self.open(path)?;
        let len = file.metadata()?.len();
        if len > max_bytes {
            return Err(ReadError::TooLarge { max: max_bytes });
        }
        let mut buf = Vec::with_capacity(len as usize);
        file.take(max_bytes + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > max_bytes {
            return Err(ReadError::TooLarge { max: max_bytes });
        }
        Ok(buf)
    }

    pub fn read_dir(&self, dir: &Path) -> Result<std::fs::ReadDir, ReadError> {
        if !self.allows_dir(dir) {
            return Err(ReadError::Denied(dir.to_path_buf()));
        }
        Ok(std::fs::read_dir(dir)?)
    }
}

impl Rule {
    fn map(&self, f: impl Fn(&Path) -> PathBuf) -> Rule {
        match self {
            Rule::TreeExt(r, e) => Rule::TreeExt(f(r), e),
            Rule::File(p) => Rule::File(f(p)),
            Rule::CoworkTranscripts(r) => Rule::CoworkTranscripts(f(r)),
        }
    }
}

/// Resolves symlinks/junctions in a (normalised) rule. A root that does not exist yet is resolved
/// through its longest existing ancestor, with the missing components appended again.
fn canonical_rule(rule: &Rule) -> Rule {
    rule.map(|p| {
        let mut existing = p;
        let mut missing = Vec::new();
        loop {
            if let Ok(real) = std::fs::canonicalize(existing) {
                let mut out = real;
                out.extend(missing.iter().rev());
                return normalize(&out);
            }
            match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    missing.push(name);
                    existing = parent;
                }
                _ => return p.to_path_buf(),
            }
        }
    })
}

/// `rule` and `path` are both normalised. The denylist is applied to the part of `path` below the
/// rule's root only.
fn rule_matches(rule: &Rule, path: &Path) -> bool {
    match rule {
        Rule::TreeExt(root, ext) => {
            below_root(path, root).is_some_and(|rest| !is_denied(rest)) && has_ext(path, ext)
        }
        Rule::File(f) => path == f && f.file_name().is_some_and(|n| !is_denied(Path::new(n))),
        Rule::CoworkTranscripts(root) => {
            below_root(path, root).is_some_and(|rest| !is_denied(rest))
                && has_ext(path, "jsonl")
                && under_claude_projects(root, path)
        }
    }
}

/// The components of `path` below `root`, if `path` is `root` or inside it.
fn below_root<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    path.strip_prefix(root).ok()
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
}

/// True if a `.claude` directory directly followed by a `projects` directory lies between `root`
/// and the file.
fn under_claude_projects(root: &Path, path: &Path) -> bool {
    let Some(dirs) = path.strip_prefix(root).ok().and_then(Path::parent) else {
        return false;
    };
    let names: Vec<String> = dirs
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    names.windows(2).any(|w| w[0] == ".claude" && w[1] == "projects")
}

fn is_denied(path: &Path) -> bool {
    path.components().any(|c| {
        let Component::Normal(name) = c else { return false };
        let name = name.to_string_lossy().to_lowercase();
        DENIED_NAMES.contains(&name.as_str())
            || DENIED_SUFFIXES.iter().any(|s| name.ends_with(s))
            || DENIED_PREFIXES.iter().any(|p| name.starts_with(p))
    })
}


/// Lexical normalisation: absolute, `.`/`..` resolved, `\\?\` stripped, and lowercased on
/// Windows (case-insensitive file system) so comparisons are stable.
fn normalize(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let text = abs.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let mut out = PathBuf::new();
    for c in Path::new(text).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if cfg!(windows) {
        PathBuf::from(out.to_string_lossy().to_lowercase())
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths::with_roots(
            tmp.path().join(".claude"),
            vec![tmp.path().join("Roaming").join("Claude")],
            tmp.path().join("data"),
        );
        (tmp, p)
    }

    #[test]
    fn allows_documented_files() {
        let (_t, p) = setup();
        let r = SafeReader::new(&p);
        assert!(r.allows(&p.projects_dir().join("proj").join("abc.jsonl")));
        assert!(r.allows(&p.claude_settings()));
        assert!(r.allows(&p.capture_dir().join("s1.json")));
        let d = &p.desktop_roots()[0];
        assert!(r.allows(&d.join("plan-usage-history.json")));
        assert!(r.allows(&d.join("claude-code-sessions").join("a").join("b").join("local_1.json")));
        let cowork = d.join("local-agent-mode-sessions").join("a").join("s");
        assert!(r.allows(&cowork.join(".claude").join("projects").join("p").join("t.jsonl")));
    }

    #[test]
    fn cowork_allows_only_transcripts_under_claude_projects() {
        let (_t, p) = setup();
        let r = SafeReader::new(&p);
        let root = p.desktop_roots()[0].join("local-agent-mode-sessions");
        let s = root.join("acct").join("org").join("sess");
        assert!(r.allows(&s.join(".claude").join("projects").join("p").join("t.jsonl")));
        assert!(r.allows(&s.join(".Claude").join("Projects").join("p").join("T.JSONL")) || !cfg!(windows));
        assert!(!r.allows(&s.join(".claude").join("history.jsonl")), "prompt history is private");
        assert!(!r.allows(&root.join("x").join("t.jsonl")));
        assert!(!r.allows(&s.join("projects").join("t.jsonl")), "needs .claude/projects");
        assert!(!r.allows(&s.join(".claude").join("x").join("projects").join("t.jsonl")));
        assert!(!r.allows(&s.join(".claude").join("projects").join("p").join("t.json")));
        assert!(r.allows_dir(&s.join(".claude")), "the walk may still descend");
    }

    #[test]
    fn canonical_rule_resolves_the_longest_existing_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = normalize(&tmp.path().join("a").join("b"));
        let Rule::File(resolved) = canonical_rule(&Rule::File(missing)) else { unreachable!() };
        let base = normalize(&std::fs::canonicalize(tmp.path()).unwrap());
        assert_eq!(resolved, base.join("a").join("b"));
    }

    #[test]
    fn canonical_rules_are_cached_and_symlinked_roots_still_work() {
        let (_t, p) = setup();
        std::fs::create_dir_all(p.capture_dir()).unwrap();
        let f = p.capture_dir().join("s.json");
        std::fs::write(&f, b"{}").unwrap();
        let r = SafeReader::new(&p);
        assert_eq!(r.rules.len(), r.canonical_rules().len());
        assert!(r.allows(&f));
    }

    #[test]
    fn denies_credentials_and_browser_storage() {
        let (_t, p) = setup();
        let r = SafeReader::new(&p);
        assert!(!r.allows(&p.claude_home().join(".credentials.json")));
        assert!(!r.allows(&p.data_root().join(".credentials.json")), "denylist beats allowed tree");
        let d = &p.desktop_roots()[0];
        assert!(!r.allows(&d.join("config.json")));
        assert!(!r.allows(&d.join("Cookies")));
        assert!(!r.allows(&d.join("Local Storage").join("leveldb").join("000003.log")));
        assert!(!r.allows(&d.join("claude_desktop_config.json")));
        assert!(!r.allows(&p.projects_dir().join("p").join("buddy-tokens.json")));
    }

    #[test]
    fn denylist_ignores_directories_above_the_allowed_roots() {
        // A profile below a folder that happens to be named like a denied component.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("Network").join("ant-home");
        let p = Paths::with_roots(
            base.join(".claude"),
            vec![base.join("Roaming").join("Claude")],
            base.join("data"),
        );
        let r = SafeReader::new(&p);
        assert!(r.allows(&p.projects_dir().join("proj").join("abc.jsonl")));
        assert!(r.allows(&p.capture_dir().join("s1.json")));
        assert!(r.allows(&p.desktop_roots()[0].join("plan-usage-history.json")));
        assert!(r.allows_dir(&p.projects_dir()));
        // Below the roots the denylist still applies.
        assert!(!r.allows(&p.projects_dir().join("network").join("abc.jsonl")));
        assert!(!r.allows(&p.projects_dir().join("p").join("ant-x.jsonl")));
        assert!(!r.allows_dir(&p.projects_dir().join("Cookies")));
    }

    /// Makes `link` point at the directory `target` (a junction on Windows, which needs no
    /// privileges). Returns false if the platform refused.
    fn dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[test]
    fn root_created_later_behind_a_link_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let linked = tmp.path().join("linked");
        if !dir_link(&real, &linked) {
            return;
        }
        // The data root does not exist yet when the reader is built.
        let p = Paths::with_roots(tmp.path().join(".claude"), vec![], linked.join("cuw"));
        let r = SafeReader::new(&p);
        std::fs::create_dir_all(p.capture_dir()).unwrap();
        let f = p.capture_dir().join("s.json");
        std::fs::write(&f, b"{}").unwrap();
        assert!(r.allows(&f));
        assert_eq!(r.read(&f, 100).unwrap(), b"{}");
    }

    #[test]
    fn rules_resolved_again_after_a_miss_are_kept() {
        // A missing root component that later appears as a link: the first read resolves the
        // rules again, and later reads use the refreshed rules instead of resolving every time.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let p = Paths::with_roots(tmp.path().join(".claude"), vec![], tmp.path().join("later").join("cuw"));
        let r = SafeReader::new(&p);
        if !dir_link(&real, &tmp.path().join("later")) {
            return;
        }
        std::fs::create_dir_all(p.capture_dir()).unwrap();
        let f = p.capture_dir().join("s.json");
        std::fs::write(&f, b"{}").unwrap();
        let resolved = normalize(&std::fs::canonicalize(&f).unwrap());
        assert!(!r.cached_match(&resolved), "stale before the first read");
        assert!(r.allows(&f));
        assert!(r.cached_match(&resolved), "the refreshed rules are cached");
        assert_eq!(r.clone().read(&f, 100).unwrap(), b"{}");
    }

    #[test]
    fn link_out_of_an_allowed_root_is_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("x.jsonl"), b"{}").unwrap();
        let p = Paths::with_roots(tmp.path().join(".claude"), vec![], tmp.path().join("data"));
        std::fs::create_dir_all(p.projects_dir()).unwrap();
        if !dir_link(&outside, &p.projects_dir().join("p")) {
            return;
        }
        let r = SafeReader::new(&p);
        let f = p.projects_dir().join("p").join("x.jsonl");
        assert!(!r.allows(&f));
        assert!(matches!(r.open(&f), Err(ReadError::Denied(_))));
    }

    #[test]
    fn only_the_capture_json_files_of_the_data_root_are_readable() {
        let (_t, p) = setup();
        let r = SafeReader::new(&p);
        assert!(r.allows(&p.capture_dir().join("s1.json")));
        assert!(r.allows_dir(&p.capture_dir()));
        assert!(!r.allows(&p.state_file()));
        assert!(!r.allows(&p.history_file()));
        assert!(!r.allows(&p.capture_dir().join("_shim.log")));
        assert!(!r.allows(&p.bin_dir().join("cuw-capture.exe")));
    }

    #[test]
    fn denies_wrong_extension_and_traversal() {
        let (_t, p) = setup();
        let r = SafeReader::new(&p);
        assert!(!r.allows(&p.projects_dir().join("p").join("notes.txt")));
        let sneaky = p.projects_dir().join("..").join(".credentials.json");
        assert!(!r.allows(&sneaky));
        let sneaky2 = p.projects_dir().join("..").join("secret.jsonl");
        assert!(!r.allows(&sneaky2), "escapes projects dir lexically");
    }

    #[test]
    fn read_enforces_size_and_denial() {
        let (_t, p) = setup();
        std::fs::create_dir_all(p.capture_dir()).unwrap();
        let f = p.capture_dir().join("s.json");
        std::fs::write(&f, b"0123456789").unwrap();
        let r = SafeReader::new(&p);
        assert_eq!(r.read(&f, 100).unwrap(), b"0123456789");
        assert!(matches!(r.read(&f, 5), Err(ReadError::TooLarge { .. })));
        assert!(matches!(
            r.read(&p.claude_home().join(".credentials.json"), 100),
            Err(ReadError::Denied(_))
        ));
    }

    #[test]
    fn dir_listing_rules() {
        let (_t, p) = setup();
        let r = SafeReader::new(&p);
        assert!(r.allows_dir(&p.projects_dir()));
        assert!(r.allows_dir(&p.projects_dir().join("sub")));
        assert!(r.allows_dir(&p.desktop_roots()[0]), "parent of an allowed file may be listed");
        assert!(!r.allows_dir(&p.desktop_roots()[0].join("Local Storage")));
        assert!(!r.allows_dir(&p.desktop_roots()[0].join("logs")));
    }
}
