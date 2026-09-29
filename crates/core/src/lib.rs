//! Token-free Claude usage data sources and engine.
//!
//! Hard rule for everything in this crate: never read credentials, cookies or browser storage,
//! and never make network calls. Every read of Claude Code's or Claude Desktop's files goes
//! through [`saferead::SafeReader`] (whose only data-root rule is `capture/*.json`). The widget's
//! own files under the data root ([`paths::Paths::data_root`]) may be read directly:
//! - `history.jsonl`, by [`history::History`] (loading, and the last-byte check before an append);
//! - `capture/<session_id>.json`, by the shim before it replaces a capture and by
//!   [`sources::statusline::prune`] ([`sources::statusline::load_captures`] and
//!   [`sources::statusline::CaptureCache`] use the reader);
//! - in the app and the shim: `settings.json`, `state.json`, `alerts.json`, `update-check.json`,
//!   `wrap.json`, `bin/mikyas-capture.exe` (to skip identical copies), `backups/` (listed for
//!   rotation) and the shim's `capture/_*.log` files.
//!
//! Outside the data root, Connect reads the shim sidecar next to the app executable (the copy it
//! installs into `bin/`), and on Unix the shim's `--diag` reads `/proc/<pid>/comm` and `stat` for
//! process names. [`paths::Paths::detect`] (and [`paths::Paths::refresh_desktop_roots`]) list the
//! entry names in `%LOCALAPPDATA%\Packages` to find MSIX copies of Claude Desktop.
//!
//! Metadata-only calls (size, modification time, existence, link resolution) that do not go
//! through the reader and never open a file's contents:
//! - core: [`paths::Paths`]' `desktop_usage_files` / `desktop_sessions_dirs` / `cowork_dirs`
//!   (existence of the candidates), the transcript scan's `current_stamp` (a listed file's own
//!   size and mtime), the Desktop usage loader's `modified_ms`, the caches' directory-entry stamps
//!   ([`sources::statusline::CaptureCache`], [`sources::desktop_sessions::DesktopSessionCache`]),
//!   capture pruning's stamps, and the reader's own `canonicalize` of the paths it checks;
//! - the app: `std::fs::metadata` of watched Claude files (size, mtime).
//!
//! `tests/privacy_invariants.rs` fails when a raw file read appears outside this list.

pub mod alerts;
pub mod capture;
pub mod claude_settings;
pub mod cmdline;
pub mod ctx_alerts;
pub mod engine;
pub mod fingerprint;
pub mod history;
pub mod level;
pub mod model_names;
pub mod pace_alerts;
pub mod paths;
pub mod recap;
pub mod saferead;
pub mod sources;
pub mod time;
pub mod turns;
