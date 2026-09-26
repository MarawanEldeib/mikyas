//! File helpers shared by the sources: extension checks, the reader-only directory walk and
//! lenient field deserialisation.

use std::ffi::OsStr;
use std::fs::DirEntry;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::saferead::SafeReader;

/// True if `path`'s extension is `ext` (ASCII case-insensitive, `ext` without the dot).
pub(crate) fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Calls `on_file` for every regular file below `root`, descending at most `max_depth` levels
/// (entries directly in `root` are level 1). Directories are listed only through the reader;
/// unreadable ones are skipped. Symlinks and junctions are never followed.
pub(crate) fn walk_files(
    reader: &SafeReader,
    root: &Path,
    max_depth: usize,
    skip_dir: &dyn Fn(&OsStr) -> bool,
    on_file: &mut dyn FnMut(&DirEntry),
) {
    walk_level(reader, root, 1, max_depth, skip_dir, on_file);
}

fn walk_level(
    reader: &SafeReader,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    skip_dir: &dyn Fn(&OsStr) -> bool,
    on_file: &mut dyn FnMut(&DirEntry),
) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = reader.read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_file() {
            on_file(&entry);
        } else if kind.is_dir() && !skip_dir(&entry.file_name()) {
            walk_level(reader, &entry.path(), depth + 1, max_depth, skip_dir, on_file);
        }
    }
}

/// Deserialises a field leniently: a value of the wrong type becomes `None` instead of failing
/// the whole record.
pub(crate) fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_check_ignores_case() {
        assert!(has_extension(Path::new("a/b.JSONL"), "jsonl"));
        assert!(!has_extension(Path::new("a/b.jsonl.tmp"), "jsonl"));
        assert!(!has_extension(Path::new("a/jsonl"), "jsonl"));
    }
}
