//! Claude Desktop's own usage sampler: `<desktop_root>/plan-usage-history.json`.
//!
//! Observed format (undocumented, version 2):
//! ```json
//! {"version":2,"samples":[{"t":1790000000000,"org":"<uuid>","u":{"fh":29,"sd":59}}]}
//! ```
//! - `t`: epoch ms. Samples arrive ~every 15 min while Desktop runs (gaps of hours/days happen).
//! - `u.fh`: 5-hour %, `u.sd`: 7-day %. Integers 0..=100 today (100 does occur); accept floats.
//!   Optional `so`/`sn` (per-model weekly) and any unknown keys map via `WindowKind::from_key`.
//! - No reset times.
//! - `org` identifies the account's organization. PRIVACY: it must never be stored, logged,
//!   serialised or shown. Compare orgs only in memory to keep samples of the org that owns the
//!   newest sample (a different account's history is dropped), then discard the string.

use std::collections::BTreeMap;

use crate::engine::types::{Observation, Sample, WindowKind};
use crate::paths::Paths;
use crate::saferead::SafeReader;
use crate::sources::SourceError;
use crate::time::Ms;

pub const SUPPORTED_VERSION: u32 = 2;
/// The file grows ~100 samples/day; refuse anything absurd.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopUsage {
    pub version: u32,
    /// Per window, samples sorted by `t_ms` ascending, duplicates (same t) removed, pct clamped
    /// to 0..=100, non-numeric values skipped.
    pub series: BTreeMap<WindowKind, Vec<Sample>>,
    pub last_sample_ms: Option<Ms>,
}

/// Parses the file. `version != 2` → `Err(SourceError::SchemaChanged(v))`; missing/invalid
/// structure → `Err(SourceError::Parse(..))`. Truncated JSON (Desktop mid-write) is a Parse
/// error — callers keep their previous good value. Samples with a missing/invalid `t` or `u` are
/// skipped individually. Samples without `org` are kept only if the newest sample also has none.
pub fn parse(bytes: &[u8]) -> Result<DesktopUsage, SourceError> {
    let _ = bytes;
    todo!("desktop_usage::parse")
}

/// The newest sample of each series as a Desktop observation (`resets_at_ms: None`).
pub fn latest_observations(usage: &DesktopUsage) -> Vec<Observation> {
    let _ = usage;
    todo!("desktop_usage::latest_observations")
}

/// Loads every existing `plan-usage-history.json` (see [`Paths::desktop_usage_files`]) and returns
/// the one with the newest `last_sample_ms`. `Ok(None)` if no file exists. If every existing file
/// fails, returns the first error (SchemaChanged takes precedence over Parse).
pub fn load(reader: &SafeReader, paths: &Paths) -> Result<Option<DesktopUsage>, SourceError> {
    let _ = (reader, paths);
    todo!("desktop_usage::load")
}
