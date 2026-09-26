//! The crate's central privacy rule, enforced on its own source: every read of Claude's files goes
//! through `saferead::SafeReader`. A raw file read (`File::open`, `fs::read`, `fs::read_to_string`,
//! `fs::read_dir`, `OpenOptions` with `.read(true)`) outside test code must be on the allowlist
//! below, which mirrors the exceptions listed in the crate docs (`src/lib.rs`). Adding a raw read
//! means adding it here and to those docs, after checking that it never opens a Claude file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Raw-read patterns counted in non-test code.
const PATTERNS: &[&str] =
    &["File::open(", "fs::read(", "fs::read_to_string(", "fs::read_dir(", "fs::copy(", ".read(true)"];

/// `std::fs` functions that read contents (or list a directory). Importing one by name would let a
/// bare call (`read_dir(x)`) slip past [`PATTERNS`], so non-test code may not import them.
const READ_FNS: &[&str] = &["read", "read_to_string", "read_dir", "copy"];

/// `(file below src/, pattern, count)`: every raw read the crate is allowed to make.
const ALLOWED: &[(&str, &str, usize)] = &[
    // The reader itself.
    ("saferead.rs", "File::open(", 1),
    ("saferead.rs", "fs::read_dir(", 1),
    // `read_capture_file`: the shim's own capture file before it replaces it, and pruning.
    ("capture.rs", "File::open(", 1),
    // The widget's own history: loading, and the last-byte check before an append.
    ("history.rs", "fs::read(", 1),
    ("history.rs", ".read(true)", 1),
    // Entry names of `%LOCALAPPDATA%\Packages` (MSIX copies of Claude Desktop); nothing is opened.
    ("paths.rs", "fs::read_dir(", 1),
    // Pruning lists the widget's own capture dir.
    ("sources/statusline.rs", "fs::read_dir(", 1),
];

#[test]
fn raw_file_reads_are_only_the_documented_exceptions() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: BTreeMap<(String, &str), usize> = BTreeMap::new();
    for file in rust_files(&src) {
        let rel = file.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        let code = without_test_code(&std::fs::read_to_string(&file).unwrap());
        for pattern in PATTERNS {
            let n = code.matches(pattern).count();
            if n > 0 {
                found.insert((rel.clone(), pattern), n);
            }
        }
    }
    let allowed: BTreeMap<(String, &str), usize> =
        ALLOWED.iter().map(|&(file, pattern, n)| ((file.to_owned(), pattern), n)).collect();
    assert_eq!(found, allowed, "raw file reads outside SafeReader changed; see the module docs");
}

#[test]
fn std_fs_read_functions_are_not_imported_by_name() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for file in rust_files(&src) {
        let code = without_test_code(&std::fs::read_to_string(&file).unwrap());
        if imports_read_fn(&code) {
            offenders.push(file.display().to_string());
        }
    }
    assert!(offenders.is_empty(), "import std::fs::File/fs instead: {offenders:?}");
}

#[test]
fn read_fn_imports_are_detected() {
    assert!(imports_read_fn("use std::fs::read_dir;"));
    assert!(imports_read_fn("use std::fs::{self, read};"));
    assert!(imports_read_fn("use std::{fs::{File, read_to_string}, io};"));
    assert!(imports_read_fn("use std::fs::copy as cp;"));
    assert!(!imports_read_fn("use std::fs::{self, File, OpenOptions, DirEntry};"));
    assert!(!imports_read_fn("use std::io::Read;"));
}

/// True if a `use` declaration of `code` names a [`READ_FNS`] function of `std::fs`.
fn imports_read_fn(code: &str) -> bool {
    let mut rest = code;
    while let Some(at) = rest.find("use ") {
        let is_keyword = rest[..at].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let stmt = &rest[at..];
        let stmt = &stmt[..stmt.find(';').unwrap_or(stmt.len())];
        if is_keyword {
            if let Some(fs) = stmt.find("fs::") {
                let names = stmt[fs + 4..].split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
                if names.into_iter().any(|name| READ_FNS.contains(&name)) {
                    return true;
                }
            }
        }
        rest = &rest[at + 4..];
    }
    false
}

#[test]
fn test_code_is_stripped_but_nothing_else() {
    let src = r##"
fn keep() { let a = b'{'; let s = "}"; File::open(x); }
#[cfg(test)]
mod tests {
    // an apostrophe in a comment: don't
    fn t<'a>(x: &'a str) { let r = r#"{"k":"}}"}"#; File::open(y); }
}
#[cfg(test)]
use something::File;
fn after() { fs::read(z); }
"##;
    let code = without_test_code(src);
    assert_eq!(code.matches("File::open(").count(), 1, "{code}");
    assert!(code.contains("fn after()"));
    assert!(!code.contains("something"));
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// `code` without the items that follow a `#[cfg(test)]` attribute (a braced block, or an item
/// ending in `;`). Strings, raw strings, char literals and comments are skipped when matching
/// braces.
fn without_test_code(code: &str) -> String {
    const MARK: &str = "#[cfg(test)]";
    let bytes = code.as_bytes();
    let mut out = String::new();
    let mut rest = 0;
    while let Some(found) = code[rest..].find(MARK) {
        let start = rest + found;
        out.push_str(&code[rest..start]);
        rest = item_end(bytes, start + MARK.len());
    }
    out.push_str(&code[rest..]);
    out
}

/// The index just after the item starting at `i`.
fn item_end(b: &[u8], mut i: usize) -> usize {
    let mut depth = 0usize;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            b'r' if matches!(b.get(i + 1), Some(b'"' | b'#')) && !ident_byte(b, i) => {
                i = raw_string_end(b, i + 1);
                continue;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'\'' => {
                // A char literal ('x', '\n', '\''); otherwise a lifetime.
                if b.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    while i < b.len() && b[i] != b'\'' {
                        i += 1;
                    }
                } else if let Some(len) =
                    std::str::from_utf8(&b[i + 1..]).ok().and_then(|s| s.chars().next()).map(char::len_utf8)
                {
                    if b.get(i + 1 + len) == Some(&b'\'') {
                        i += 1 + len;
                    }
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// `b[i]` is preceded by an identifier byte (so an `r` is part of a name, not a raw string).
fn ident_byte(b: &[u8], i: usize) -> bool {
    i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') && b[i - 1] != b'b'
}

/// `b[i..]` starts with the `#`s and quote of a raw string; returns the index after it.
fn raw_string_end(b: &[u8], mut i: usize) -> usize {
    let mut hashes = 0;
    while b.get(i) == Some(&b'#') {
        hashes += 1;
        i += 1;
    }
    if b.get(i) != Some(&b'"') {
        return i;
    }
    i += 1;
    while i < b.len() {
        if b[i] == b'"' && b[i + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes {
            return i + 1 + hashes;
        }
        i += 1;
    }
    b.len()
}
