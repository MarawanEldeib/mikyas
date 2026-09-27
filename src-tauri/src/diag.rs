//! Diagnostics log: persistence, notification and watcher errors the app recovers from.
//!
//! - Debug builds also print to stderr.
//! - Once [`init`] named the data root, every build appends to `<data_root>/app.log`; at
//!   [`MAX_LOG_BYTES`] the file is moved to `app.log.1` (replacing the older one), so the two
//!   together stay small.
//! - Messages name files and operations only, never values read from Claude's files or the
//!   user's settings (same rule as the shim's `_errors.log`).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use mikyas_core::time::now_ms;

pub const MAX_LOG_BYTES: u64 = 256 * 1024;
const FILE_NAME: &str = "app.log";

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
/// Serialises appends and the size-cap rotation.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Starts writing `<data_root>/app.log` (first call wins).
pub fn init(data_root: &Path) {
    let _ = LOG_FILE.set(data_root.join(FILE_NAME));
}

pub fn log(msg: &str) {
    if cfg!(debug_assertions) {
        eprintln!("[mikyas] {msg}");
    }
    if let Some(path) = LOG_FILE.get() {
        let _guard = crate::state::lock(&WRITE_LOCK);
        append_capped(path, &format!("{} {msg}\n", now_ms()), MAX_LOG_BYTES);
    }
}

/// Appends `line`, first rotating `path` to `<path>.1` when it has reached `cap` bytes.
/// Best-effort: a log that cannot be written is skipped.
fn append_capped(path: &Path, line: &str, cap: u64) {
    if fs::metadata(path).is_ok_and(|m| m.len() >= cap) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = fs::rename(path, PathBuf::from(old));
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_is_size_capped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("data").join(FILE_NAME);
        for i in 0..10 {
            append_capped(&path, &format!("line {i}\n"), 20);
        }
        let current = fs::read_to_string(&path).unwrap();
        let rotated = fs::read_to_string(tmp.path().join("data").join("app.log.1")).unwrap();
        assert!(current.len() < 40 && rotated.len() < 40, "{current:?} {rotated:?}");
        assert!(current.ends_with("line 9\n"));
        assert_eq!(fs::read_dir(tmp.path().join("data")).unwrap().count(), 2);
    }
}
