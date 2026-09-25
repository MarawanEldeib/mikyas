//! File-system watches on the capture dir, the Claude Code projects dir and the Cowork session
//! dirs. Events become pipeline [`Msg`]s; coalescing happens in the pipeline. Directories that do
//! not exist yet are retried by [`Watcher::ensure`] (the pipeline calls it every 30 s).
//! The Desktop data dir is deliberately NOT watched (Electron churns it); it is polled.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cuw_core::paths::Paths;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::pipeline::{Msg, log};
use crate::state::Shared;

pub struct Watcher {
    inner: Option<RecommendedWatcher>,
    paths: Paths,
    watched: HashSet<PathBuf>,
}

impl Watcher {
    pub fn new(shared: Arc<Shared>, paths: Paths) -> Self {
        let capture_dir = paths.capture_dir();
        let _ = std::fs::create_dir_all(&capture_dir);
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                if let Some(msg) = classify(&capture_dir, &path) {
                    shared.send(msg);
                }
            }
        };
        let inner = match notify::recommended_watcher(handler) {
            Ok(w) => Some(w),
            Err(e) => {
                log(&format!("file watcher unavailable ({e}); relying on polling"));
                None
            }
        };
        let mut w = Self {
            inner,
            paths,
            watched: HashSet::new(),
        };
        w.ensure();
        w
    }

    /// Adds watches for directories that exist now and are not watched yet.
    pub fn ensure(&mut self) {
        let Some(inner) = self.inner.as_mut() else { return };
        let mut wanted = vec![
            (self.paths.capture_dir(), RecursiveMode::NonRecursive),
            (self.paths.projects_dir(), RecursiveMode::Recursive),
        ];
        wanted.extend(
            self.paths
                .cowork_dirs()
                .into_iter()
                .map(|d| (d, RecursiveMode::Recursive)),
        );
        for (dir, mode) in wanted {
            if self.watched.contains(&dir) || !dir.is_dir() {
                continue;
            }
            match inner.watch(&dir, mode) {
                Ok(()) => {
                    self.watched.insert(dir);
                }
                Err(e) => log(&format!("watch failed for a source dir: {e}")),
            }
        }
    }
}

/// Maps a changed path to a pipeline message.
fn classify(capture_dir: &Path, path: &Path) -> Option<Msg> {
    if path.starts_with(capture_dir) {
        let json = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
        return json.then_some(Msg::Captures);
    }
    let jsonl = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl"));
    jsonl.then(|| Msg::Transcript(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_paths() {
        let cap = Path::new("C:/d/capture");
        assert_eq!(classify(cap, Path::new("C:/d/capture/s.json")), Some(Msg::Captures));
        assert_eq!(classify(cap, Path::new("C:/d/capture/s.json.tmp")), None);
        assert_eq!(
            classify(cap, Path::new("C:/h/projects/p/a.jsonl")),
            Some(Msg::Transcript("C:/h/projects/p/a.jsonl".into()))
        );
        assert_eq!(classify(cap, Path::new("C:/h/projects/p/a.txt")), None);
    }
}
