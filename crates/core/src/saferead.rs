//! File-read allowlist. Every source parser reads through [`SafeReader`], so the widget can
//! only ever open the handful of files it documents in PRIVACY.md. Credential, cookie and
//! browser-storage files are hard-denied even if they sit inside an allowed tree.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

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
    /// Anything below the root.
    Tree(PathBuf),
    /// Files below the root with this extension (lowercase, no dot).
    TreeExt(PathBuf, &'static str),
    /// Exactly this file.
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct SafeReader {
    rules: Vec<Rule>,
}

impl SafeReader {
    pub fn new(paths: &Paths) -> Self {
        let mut rules = vec![
            Rule::Tree(paths.data_root().to_path_buf()),
            Rule::File(paths.claude_settings()),
            Rule::TreeExt(paths.projects_dir(), "jsonl"),
        ];
        for root in paths.desktop_roots() {
            rules.push(Rule::File(root.join("plan-usage-history.json")));
            rules.push(Rule::TreeExt(root.join("claude-code-sessions"), "json"));
            rules.push(Rule::TreeExt(root.join("local-agent-mode-sessions"), "jsonl"));
        }
        Self { rules }
    }

    /// True if `path` may be read.
    pub fn allows(&self, path: &Path) -> bool {
        let lexical = normalize(path);
        if is_denied(&lexical) || !self.rules.iter().any(|r| rule_matches(r, &lexical)) {
            return false;
        }
        // If the file exists, the resolved target (symlinks/junctions) must also be allowed.
        match std::fs::canonicalize(path) {
            Ok(real) => {
                let real = normalize(&real);
                !is_denied(&real)
                    && self
                        .rules
                        .iter()
                        .any(|r| rule_matches(&canonical_rule(r), &real))
            }
            Err(_) => true,
        }
    }

    /// True if `dir` may be listed (it is, or is inside, an allowed tree root).
    pub fn allows_dir(&self, dir: &Path) -> bool {
        let d = normalize(dir);
        !is_denied(&d)
            && self.rules.iter().any(|r| match r {
                Rule::Tree(root) | Rule::TreeExt(root, _) => starts_with(&d, &normalize(root)),
                Rule::File(f) => normalize(f).parent().is_some_and(|p| p == d),
            })
    }

    pub fn open(&self, path: &Path) -> Result<File, ReadError> {
        if !self.allows(path) {
            return Err(ReadError::Denied(path.to_path_buf()));
        }
        Ok(File::open(path)?)
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

fn canonical_rule(rule: &Rule) -> Rule {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    match rule {
        Rule::Tree(r) => Rule::Tree(canon(r)),
        Rule::TreeExt(r, e) => Rule::TreeExt(canon(r), e),
        Rule::File(f) => Rule::File(canon(f)),
    }
}

fn rule_matches(rule: &Rule, path: &Path) -> bool {
    match rule {
        Rule::Tree(root) => starts_with(path, &normalize(root)),
        Rule::TreeExt(root, ext) => {
            starts_with(path, &normalize(root))
                && path
                    .extension()
                    .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
        }
        Rule::File(f) => path == normalize(f),
    }
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

fn starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
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
        assert!(r.allows(&d.join("local-agent-mode-sessions").join("x").join("t.jsonl")));
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
