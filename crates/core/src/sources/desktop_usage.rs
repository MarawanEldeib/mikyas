//! Claude Desktop's own usage sampler: `<desktop_root>/plan-usage-history.json`.
//!
//! Observed format (undocumented, version 2; a newer `version` is read best-effort, see
//! [`parse_until`]):
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
//!
//! The file only grows, so it is not parsed into a JSON tree: typed visitors keep just `version`,
//! each sample's `t`, `org` (borrowed from the input) and the numeric `u` values.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::engine::types::{DesktopHealth, Observation, Sample, Source, WindowKind};
use crate::paths::Paths;
use crate::saferead::SafeReader;
use crate::sources::SourceError;
use crate::time::{MAX_PLAUSIBLE_MS, Ms, json_time_to_ms, system_time_ms};

pub const SUPPORTED_VERSION: u32 = 2;
/// The file grows ~100 samples/day; refuse anything absurd.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Longest `u` key accepted as a window name. Keys must also be `[A-Za-z0-9_]+`, so arbitrary
/// strings from the file never become window names shown in the UI.
const MAX_KEY_LEN: usize = 32;

/// Parsed usage history of the account that owns the newest sample. Holds no account identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopUsage {
    /// The file's `version`: [`SUPPORTED_VERSION`], or a newer one that was read best-effort.
    pub version: u32,
    /// Per window, samples sorted by `t_ms` ascending, duplicates (same t) removed, pct clamped
    /// to 0..=100, non-numeric values skipped.
    pub series: BTreeMap<WindowKind, Vec<Sample>>,
    /// `t` of the newest kept sample; `None` if the file holds no usable sample.
    pub last_sample_ms: Option<Ms>,
}

impl DesktopUsage {
    /// The file's version when it is newer than [`SUPPORTED_VERSION`] (read best-effort).
    pub fn newer_version(&self) -> Option<u32> {
        (self.version > SUPPORTED_VERSION).then_some(self.version)
    }

    /// Source health of a successfully loaded file.
    pub fn health(&self) -> DesktopHealth {
        DesktopHealth::Ok { last_sample_ms: self.last_sample_ms, newer_version: self.newer_version() }
    }
}

/// A structurally valid sample. Deliberately not `Debug`: it carries the org string, which must
/// never be printed.
struct RawSample<'a> {
    t_ms: Ms,
    org: Option<Cow<'a, str>>,
    /// `None` unless `u` is valid: the usable `(key, pct)` pairs in file order.
    usage: Option<Vec<(Cow<'a, str>, f32)>>,
}

/// [`parse_until`] without a time limit (sample times are still range-checked by
/// [`json_time_to_ms`]).
pub fn parse(bytes: &[u8]) -> Result<DesktopUsage, SourceError> {
    parse_until(bytes, MAX_PLAUSIBLE_MS)
}

/// Parses the file. An older `version` than 2 → `Err(SourceError::SchemaChanged(v))`. A newer one
/// is read best-effort with the same rules (unknown fields and keys are ignored); it is only
/// `SchemaChanged` when it has no `samples` array, or a non-empty one in which no sample has both a
/// valid `t` and a valid `u`. Missing/invalid structure of a version-2 file →
/// `Err(SourceError::Parse(..))`. Truncated JSON (Desktop mid-write) is a Parse
/// error — callers keep their previous good value. Samples with a missing/invalid `t`, or one
/// after `max_t_ms` (written while the clock ran ahead), are skipped individually.
///
/// The owning account is the org of the newest sample that names an org or has a valid `u`, so
/// an account switch counts from the new account's first sample even before it reports values.
/// Only the owner's samples with a valid `u` are kept; samples without `org` only if the owner
/// has none either.
///
/// A `u` counts as valid when it holds at least one numeric value under a plausible key. Of
/// several samples with the same `t`, the one written last wins. Error messages never quote the
/// document, so they cannot leak the org.
pub fn parse_until(bytes: &[u8], max_t_ms: Ms) -> Result<DesktopUsage, SourceError> {
    // A UTF-8 BOM (added by some Windows editors) is not JSON whitespace to serde_json.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let doc: Doc<'_> = serde_json::from_slice(bytes).map_err(json_error)?;
    let Doc::Object { version, samples } = doc else {
        return Err(parse_error("top level is not an object"));
    };
    let version = version.as_ref().and_then(as_version).ok_or_else(|| parse_error("missing or invalid version"))?;
    let newer = version > SUPPORTED_VERSION;
    if version < SUPPORTED_VERSION {
        return Err(SourceError::SchemaChanged(version));
    }
    let entries = match samples {
        Some(entries) => entries,
        None if newer => return Err(SourceError::SchemaChanged(version)),
        None => return Err(parse_error("missing samples array")),
    };

    let any_entries = !entries.is_empty();
    let mut raw: Vec<RawSample<'_>> = entries.into_iter().filter_map(raw_sample).collect();
    // A newer format whose samples no longer carry a recognisable `t` and `u` is not guessed at.
    if newer && any_entries && !raw.iter().any(|s| s.usage.is_some()) {
        return Err(SourceError::SchemaChanged(version));
    }
    raw.retain(|s| s.t_ms <= max_t_ms);
    // `max_by_key` returns the last of equal maxima, i.e. the later entry of an append-only file.
    let owner =
        raw.iter().filter(|s| s.org.is_some() || s.usage.is_some()).max_by_key(|s| s.t_ms).map(|s| s.org.as_deref());

    let mut series: BTreeMap<WindowKind, Vec<Sample>> = BTreeMap::new();
    let mut last_sample_ms = None;
    for sample in raw.iter().filter(|s| Some(s.org.as_deref()) == owner) {
        let Some(usage) = &sample.usage else { continue };
        last_sample_ms = last_sample_ms.max(Some(sample.t_ms));
        for (key, pct) in usage {
            series.entry(WindowKind::from_key(key)).or_default().push(Sample { t_ms: sample.t_ms, pct: *pct });
        }
    }
    for samples in series.values_mut() {
        sort_dedup(samples);
    }
    Ok(DesktopUsage { version, series, last_sample_ms })
}

/// The newest sample of each series as a Desktop observation (`resets_at_ms: None`).
pub fn latest_observations(usage: &DesktopUsage) -> Vec<Observation> {
    usage
        .series
        .iter()
        .filter_map(|(kind, samples)| {
            let last = samples.last()?;
            Some(Observation {
                kind: kind.clone(),
                pct: last.pct,
                resets_at_ms: None,
                observed_at_ms: last.t_ms,
                source: Source::Desktop,
            })
        })
        .collect()
}

/// Loads every existing `plan-usage-history.json` (see [`Paths::desktop_usage_files`]) with
/// [`parse_until`] and returns the one with the newest `last_sample_ms`. `Ok(None)` if no file
/// exists. If every existing file fails, returns the first error (SchemaChanged takes precedence
/// over Parse).
///
/// Ties on `last_sample_ms` go to the earlier (more specific) root. A file that disappears
/// between listing and reading counts as absent. A SchemaChanged file modified after the chosen
/// file's newest sample (after its mtime if it has none) is the one the running Desktop writes,
/// the chosen one a stale copy in another root: its error is returned instead.
pub fn load(reader: &SafeReader, paths: &Paths, max_t_ms: Ms) -> Result<Option<DesktopUsage>, SourceError> {
    let mut best: Option<(DesktopUsage, PathBuf)> = None;
    let mut error: Option<SourceError> = None;
    // Newest mtime among the SchemaChanged files, with that file's version.
    let mut newer_schema: Option<(Ms, u32)> = None;
    for path in paths.desktop_usage_files() {
        match read_file(reader, &path, max_t_ms) {
            Ok(usage) => {
                if best.as_ref().is_none_or(|(b, _)| usage.last_sample_ms > b.last_sample_ms) {
                    best = Some((usage, path));
                }
            }
            Err(SourceError::NotFound) => {}
            Err(e) => {
                if let SourceError::SchemaChanged(version) = e {
                    let mtime = modified_ms(&path);
                    if let Some(m) = mtime.filter(|&m| newer_schema.is_none_or(|(n, _)| m > n)) {
                        newer_schema = Some((m, version));
                    }
                }
                let replace = match (&error, &e) {
                    (None, _) => true,
                    (Some(SourceError::SchemaChanged(_)), _) => false,
                    (Some(_), SourceError::SchemaChanged(_)) => true,
                    (Some(_), _) => false,
                };
                if replace {
                    error = Some(e);
                }
            }
        }
    }
    match (best, error) {
        (Some((usage, path)), _) => {
            let written = usage.last_sample_ms.or_else(|| modified_ms(&path));
            match newer_schema {
                Some((mtime, version)) if written.is_some_and(|w| mtime > w) => {
                    Err(SourceError::SchemaChanged(version))
                }
                _ => Ok(Some(usage)),
            }
        }
        (None, Some(e)) => Err(e),
        (None, None) => Ok(None),
    }
}

fn read_file(reader: &SafeReader, path: &Path, max_t_ms: Ms) -> Result<DesktopUsage, SourceError> {
    let bytes = reader.read(path, MAX_FILE_BYTES)?;
    parse_until(&bytes, max_t_ms)
}

/// The file's mtime in epoch ms, if the file system reports one.
fn modified_ms(path: &Path) -> Option<Ms> {
    system_time_ms(std::fs::metadata(path).ok()?.modified().ok()?)
}

fn parse_error(msg: &str) -> SourceError {
    SourceError::Parse(msg.to_owned())
}

/// Category and position only: serde's own message could in principle quote input.
fn json_error(e: serde_json::Error) -> SourceError {
    SourceError::Parse(format!("invalid JSON ({:?}) at line {} column {}", e.classify(), e.line(), e.column()))
}

/// Accepts `2` and `2.0`; anything else that is not a non-negative integer is invalid.
fn as_version(v: &Value) -> Option<u32> {
    let n = v.as_u64().or_else(|| {
        v.as_f64().filter(|f| f.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(f)).map(|f| f as u64)
    })?;
    u32::try_from(n).ok()
}

fn raw_sample(entry: Entry<'_>) -> Option<RawSample<'_>> {
    let Entry::Object { t, org, u } = entry else { return None };
    let t_ms = t.as_ref().and_then(json_time_to_ms)?;
    let usage = u.map(|pairs| {
        pairs
            .into_iter()
            .filter_map(|(key, value)| {
                let pct = usable_pct(&key, value?)?;
                Some((key, pct))
            })
            .collect::<Vec<_>>()
    });
    let usage = usage.filter(|pairs| !pairs.is_empty());
    let org = org.filter(|o| !o.is_empty());
    Some(RawSample { t_ms, org, usage })
}

/// The clamped percentage of one `u` entry, if the key is plausible and the value finite.
fn usable_pct(key: &str, value: f64) -> Option<f32> {
    if !is_window_key(key) || !value.is_finite() {
        return None;
    }
    Some(value.clamp(0.0, 100.0) as f32)
}

fn is_window_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_LEN && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

// ---- typed document visitors ----
//
// Each accepts any JSON shape where the document might hold something unexpected, so a single odd
// sample or field is skipped instead of failing the whole file (as the tree-based parser did).

/// The top level: an object with (optionally) `version` and `samples`, or anything else.
enum Doc<'a> {
    Object {
        version: Option<Value>,
        /// `None` unless `samples` is an array.
        samples: Option<Vec<Entry<'a>>>,
    },
    Other,
}

/// One element of `samples`: an object's `t`, `org` (if a string) and `u` (if an object: its
/// numeric values by key, `None` for a non-numeric value), or anything else.
enum Entry<'a> {
    Object { t: Option<Value>, org: Option<Cow<'a, str>>, u: Option<Vec<(Cow<'a, str>, Option<f64>)>> },
    Other,
}

/// Visitor methods that accept any JSON scalar, so an unexpected shape yields `$value`.
macro_rules! scalars_as {
    ($ty:ty, $value:expr) => {
        fn visit_bool<E>(self, _: bool) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_i64<E>(self, _: i64) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_u64<E>(self, _: u64) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_f64<E>(self, _: f64) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_str<E>(self, _: &str) -> Result<$ty, E> {
            Ok($value)
        }
        fn visit_unit<E>(self) -> Result<$ty, E> {
            Ok($value)
        }
    };
}

struct DocVisitor;

impl<'de> Visitor<'de> for DocVisitor {
    type Value = Doc<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a usage history document")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Doc<'de>, A::Error> {
        let (mut version, mut samples) = (None, None);
        while let Some(StrField(key)) = map.next_key::<StrField<'de>>()? {
            match key.as_deref().unwrap_or_default() {
                "version" => version = Some(map.next_value::<Value>()?),
                "samples" => samples = map.next_value::<SamplesField<'de>>()?.0,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Doc::Object { version, samples })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Doc<'de>, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| Doc::Other)
    }

    scalars_as!(Doc<'de>, Doc::Other);
}

impl<'de> Deserialize<'de> for Doc<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(DocVisitor)
    }
}

/// `samples`: the entries when it is an array.
struct SamplesField<'a>(Option<Vec<Entry<'a>>>);

struct SamplesVisitor;

impl<'de> Visitor<'de> for SamplesVisitor {
    type Value = SamplesField<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a samples array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SamplesField<'de>, A::Error> {
        let mut entries = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(entry) = seq.next_element::<Entry<'de>>()? {
            entries.push(entry);
        }
        Ok(SamplesField(Some(entries)))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<SamplesField<'de>, A::Error> {
        IgnoredAny.visit_map(map).map(|_| SamplesField(None))
    }

    scalars_as!(SamplesField<'de>, SamplesField(None));
}

impl<'de> Deserialize<'de> for SamplesField<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(SamplesVisitor)
    }
}

struct EntryVisitor;

impl<'de> Visitor<'de> for EntryVisitor {
    type Value = Entry<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a sample")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entry<'de>, A::Error> {
        let (mut t, mut org, mut u) = (None, None, None);
        while let Some(StrField(key)) = map.next_key::<StrField<'de>>()? {
            match key.as_deref().unwrap_or_default() {
                "t" => t = Some(map.next_value::<Value>()?),
                "org" => org = map.next_value::<StrField<'de>>()?.0,
                "u" => u = map.next_value::<UsageField<'de>>()?.0,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Entry::Object { t, org, u })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Entry<'de>, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| Entry::Other)
    }

    scalars_as!(Entry<'de>, Entry::Other);
}

impl<'de> Deserialize<'de> for Entry<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(EntryVisitor)
    }
}

/// A string (borrowed when it has no escapes), or `None` for any other value.
struct StrField<'a>(Option<Cow<'a, str>>);

struct StrVisitor;

impl<'de> Visitor<'de> for StrVisitor {
    type Value = StrField<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<StrField<'de>, E> {
        Ok(StrField(Some(Cow::Borrowed(v))))
    }

    fn visit_str<E>(self, v: &str) -> Result<StrField<'de>, E> {
        Ok(StrField(Some(Cow::Owned(v.to_owned()))))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<StrField<'de>, A::Error> {
        IgnoredAny.visit_map(map).map(|_| StrField(None))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<StrField<'de>, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| StrField(None))
    }

    fn visit_bool<E>(self, _: bool) -> Result<StrField<'de>, E> {
        Ok(StrField(None))
    }
    fn visit_i64<E>(self, _: i64) -> Result<StrField<'de>, E> {
        Ok(StrField(None))
    }
    fn visit_u64<E>(self, _: u64) -> Result<StrField<'de>, E> {
        Ok(StrField(None))
    }
    fn visit_f64<E>(self, _: f64) -> Result<StrField<'de>, E> {
        Ok(StrField(None))
    }
    fn visit_unit<E>(self) -> Result<StrField<'de>, E> {
        Ok(StrField(None))
    }
}

impl<'de> Deserialize<'de> for StrField<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrVisitor)
    }
}

/// `u`: its values by key when it is an object (`None` for a non-numeric value).
struct UsageField<'a>(Option<Vec<(Cow<'a, str>, Option<f64>)>>);

struct UsageVisitor;

impl<'de> Visitor<'de> for UsageVisitor {
    type Value = UsageField<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a usage object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<UsageField<'de>, A::Error> {
        let mut pairs = Vec::new();
        while let Some(StrField(key)) = map.next_key::<StrField<'de>>()? {
            pairs.push((key.unwrap_or_default(), map.next_value::<NumField>()?.0));
        }
        Ok(UsageField(Some(pairs)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<UsageField<'de>, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| UsageField(None))
    }

    scalars_as!(UsageField<'de>, UsageField(None));
}

impl<'de> Deserialize<'de> for UsageField<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(UsageVisitor)
    }
}

/// A number as `f64`, or `None` for any other value.
struct NumField(Option<f64>);

struct NumVisitor;

impl<'de> Visitor<'de> for NumVisitor {
    type Value = NumField;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a number")
    }

    fn visit_i64<E>(self, v: i64) -> Result<NumField, E> {
        Ok(NumField(Some(v as f64)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<NumField, E> {
        Ok(NumField(Some(v as f64)))
    }

    fn visit_f64<E>(self, v: f64) -> Result<NumField, E> {
        Ok(NumField(Some(v)))
    }

    fn visit_bool<E>(self, _: bool) -> Result<NumField, E> {
        Ok(NumField(None))
    }

    fn visit_str<E>(self, _: &str) -> Result<NumField, E> {
        Ok(NumField(None))
    }

    fn visit_unit<E>(self) -> Result<NumField, E> {
        Ok(NumField(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<NumField, A::Error> {
        IgnoredAny.visit_map(map).map(|_| NumField(None))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<NumField, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| NumField(None))
    }
}

impl<'de> Deserialize<'de> for NumField {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumVisitor)
    }
}

/// Sorts by time; of several samples with the same `t`, the one written last wins.
fn sort_dedup(samples: &mut Vec<Sample>) {
    // Stable sort keeps file order among equal `t`.
    samples.sort_by_key(|s| s.t_ms);
    // `dedup_by` passes (later, earlier-kept); copy the later value into the kept slot.
    samples.dedup_by(|later, kept| {
        let same = later.t_ms == kept.t_ms;
        if same {
            *kept = *later;
        }
        same
    });
}

/// Deterministic generator of realistic `plan-usage-history.json` documents, shared with the
/// engine tests. It simulates real 5-hour windows (a window starts at the first message after
/// the previous one expired and lasts 5 h) and a fixed weekly reset, then samples them the way
/// Desktop does: every 15 min while the machine is awake, a few off-cycle samples, night gaps,
/// an afternoon sleep gap, a 100 % plateau and an older second account.
#[cfg(test)]
pub(crate) mod synth {
    use serde_json::{Value, json};

    use crate::time::{DAY_MS, FIVE_HOURS_MS, HOUR_MS, MINUTE_MS, Ms, SEVEN_DAYS_MS};

    pub(crate) const ORG_A: &str = "00000000-0000-4000-8000-000000000001";
    pub(crate) const ORG_B: &str = "11111111-1111-4111-8111-111111111111";
    /// 2026-09-20T00:00:00Z.
    pub(crate) const START_MS: Ms = 1_789_862_400_000;
    pub(crate) const DAYS: i64 = 4;
    pub(crate) const SAMPLE_EVERY_MS: Ms = 15 * MINUTE_MS;
    /// The simulated weekly reset: day 2, 03:00, inside a night gap.
    pub(crate) const WEEKLY_RESET_MS: Ms = START_MS + 2 * DAY_MS + 3 * HOUR_MS;

    /// One sample the simulated Desktop wrote for the current account, plus the ground truth.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) struct TrueSample {
        pub t_ms: Ms,
        pub fh: f32,
        pub sd: f32,
        /// When the 5-hour window running at `t_ms` really ends (`None`: no window running).
        pub fh_reset_ms: Option<Ms>,
        /// When the weekly window running at `t_ms` really ends.
        pub sd_reset_ms: Ms,
    }

    pub(crate) struct Synth {
        pub json: String,
        /// ORG_A samples in time order, exactly what `parse` must return.
        pub samples: Vec<TrueSample>,
    }

    impl Synth {
        pub(crate) fn fh(&self) -> Vec<crate::engine::types::Sample> {
            self.series(|s| s.fh)
        }
        pub(crate) fn sd(&self) -> Vec<crate::engine::types::Sample> {
            self.series(|s| s.sd)
        }
        fn series(&self, f: impl Fn(&TrueSample) -> f32) -> Vec<crate::engine::types::Sample> {
            self.samples.iter().map(|s| crate::engine::types::Sample { t_ms: s.t_ms, pct: f(s) }).collect()
        }
    }

    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        /// Uniform-ish integer in `lo..=hi`.
        fn range(&mut self, lo: i64, hi: i64) -> i64 {
            lo + (self.next() % (hi - lo + 1) as u64) as i64
        }
    }

    /// Desktop is running (and sampling) from 08:00 to midnight, except two sleep gaps.
    fn awake(t: Ms) -> bool {
        let day = (t - START_MS) / DAY_MS;
        let minute = (t - START_MS) % DAY_MS / MINUTE_MS;
        let asleep = match day {
            1 => (13 * 60..15 * 60 + 10).contains(&minute),
            3 => (17 * 60..19 * 60 + 30).contains(&minute),
            _ => false,
        };
        minute >= 8 * 60 && !asleep
    }

    /// Bursts of 0.5–1.2 %/min.
    pub(crate) fn realistic() -> Synth {
        generate(10.0)
    }

    /// The same days with a tenth of the usage (bursts of 0.05–0.12 %/min): Desktop's integers
    /// stay at 0 for up to ten minutes after a window starts.
    pub(crate) fn light() -> Synth {
        generate(100.0)
    }

    /// Burst rates are `5..=12 / rate_divisor` %/min (the scripted bursts scale alike).
    fn generate(rate_divisor: f32) -> Synth {
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        let end = START_MS + DAYS * DAY_MS;
        let heavy_burst = START_MS + DAY_MS + 9 * HOUR_MS;
        let phone_burst = START_MS + 2 * DAY_MS + 5 * HOUR_MS;

        let (mut fh, mut fh_end) = (0.0_f32, None::<Ms>);
        let (mut sd, mut sd_end) = (58.0_f32, WEEKLY_RESET_MS);
        let (mut burst_until, mut next_burst, mut rate) = (0, START_MS + 8 * HOUR_MS + 20 * MINUTE_MS, 0.0_f32);
        let mut samples = Vec::new();

        let mut t = START_MS;
        while t < end {
            if fh_end.is_some_and(|e| t >= e) {
                fh = 0.0;
                fh_end = None;
            }
            if t >= sd_end {
                sd = 0.0;
                sd_end += SEVEN_DAYS_MS;
            }

            if t == heavy_burst {
                // Guarantees a 100 % plateau (at the realistic rates).
                (burst_until, rate, next_burst) = (t + 150 * MINUTE_MS, 10.0 / rate_divisor, t + 240 * MINUTE_MS);
            } else if t == phone_burst {
                // Usage on another device during the night gap: starts a window Desktop never saw.
                (burst_until, rate, next_burst) =
                    (t + 30 * MINUTE_MS, 6.0 / rate_divisor, t + 3 * HOUR_MS + 20 * MINUTE_MS);
            } else if t >= next_burst && t >= burst_until && awake(t) {
                burst_until = t + rng.range(20, 110) * MINUTE_MS;
                rate = rng.range(5, 12) as f32 / rate_divisor;
                next_burst = burst_until + rng.range(20, 200) * MINUTE_MS;
            }
            let phone = (phone_burst..phone_burst + 30 * MINUTE_MS).contains(&t);
            if t < burst_until && (awake(t) || phone) && fh < 100.0 && sd < 100.0 {
                if fh_end.is_none() {
                    fh_end = Some(t + FIVE_HOURS_MS);
                }
                fh = (fh + rate).min(100.0);
                sd = (sd + rate * 0.08).min(100.0);
            }

            let offset = t - START_MS;
            let on_cycle = offset % SAMPLE_EVERY_MS == 0;
            let off_cycle = offset % (6 * HOUR_MS) == 7 * MINUTE_MS;
            if awake(t) && (on_cycle || off_cycle) {
                samples.push(TrueSample {
                    t_ms: t,
                    fh: fh.round(),
                    sd: sd.round(),
                    fh_reset_ms: fh_end,
                    sd_reset_ms: sd_end,
                });
            }
            t += MINUTE_MS;
        }

        let entry = |t: Ms, org: Option<&str>, u: Value| match org {
            Some(org) => json!({"t": t, "org": org, "u": u}),
            None => json!({"t": t, "u": u}),
        };
        // A previous account, older than everything else: must be dropped.
        let mut out: Vec<Value> = (0..12)
            .map(|i| {
                let t = START_MS - 3 * HOUR_MS + i * SAMPLE_EVERY_MS;
                entry(t, Some(ORG_B), json!({"fh": 90 + i % 3, "sd": 97}))
            })
            .collect();
        out.extend(samples.iter().map(|s| entry(s.t_ms, Some(ORG_A), json!({"fh": s.fh as i64, "sd": s.sd as i64}))));
        // File noise Desktop could produce: an exact duplicate, a swapped pair, a sample with no `u`.
        let dup = out[40].clone();
        out.insert(41, dup);
        out.swap(60, 61);
        out.push(json!({"t": end, "org": ORG_A}));

        Synth { json: json!({"version": 2, "samples": out}).to_string(), samples }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use std::time::{Duration, UNIX_EPOCH};

    use super::synth::{ORG_A, ORG_B, START_MS};
    use super::*;
    use crate::time::{DAY_MS, FIVE_HOURS_MS, MINUTE_MS};

    const EDGE: &str = include_str!("../../tests/fixtures/desktop_usage/edge_cases.json");

    fn s(t_ms: Ms, pct: f32) -> Sample {
        Sample { t_ms, pct }
    }

    fn series<'a>(u: &'a DesktopUsage, key: &str) -> &'a [Sample] {
        u.series.get(&WindowKind::from_key(key)).map(Vec::as_slice).unwrap_or_default()
    }

    fn assert_no_org(text: &str) {
        for org in [ORG_A, ORG_B] {
            assert!(!text.contains(org), "org leaked: {text}");
            // Distinctive fragments, in case something reformats the string.
            assert!(!text.contains(&org[8..23]), "org fragment leaked: {text}");
        }
    }

    #[test]
    fn parses_edge_case_fixture() {
        let u = parse(EDGE.as_bytes()).unwrap();
        assert_eq!(u.version, 2);
        assert_eq!(u.last_sample_ms, Some(1_790_211_600_000));
        assert_eq!(
            series(&u, "fh"),
            [
                s(1_790_208_000_000, 29.0),
                s(1_790_208_900_000, 30.0),
                s(1_790_209_800_000, 32.0),  // duplicate t: the later entry wins
                s(1_790_211_600_000, 100.0)  // 140 clamped
            ]
        );
        assert_eq!(
            series(&u, "sd"),
            [
                s(1_790_208_000_000, 59.0),
                s(1_790_208_900_000, 60.0),
                s(1_790_209_800_000, 61.0),
                s(1_790_211_600_000, 0.0) // -3 clamped
            ]
        );
        // `so`/`sn` map to the per-model weekly windows; "12" (string) and null are skipped.
        assert_eq!(series(&u, "so"), [s(1_790_208_000_000, 12.0)]);
        assert_eq!(series(&u, "sn"), [s(1_790_208_000_000, 7.5)]);
        assert!(u.series.contains_key(&WindowKind::Other("seven_day_opus".into())));
        assert!(u.series.contains_key(&WindowKind::Other("seven_day_sonnet".into())));
        // Unknown but plausible key kept; implausible key dropped.
        assert_eq!(u.series.get(&WindowKind::Other("xh".into())), Some(&vec![s(1_790_209_800_000, 3.0)]));
        assert_eq!(u.series.len(), 5, "{:?}", u.series.keys().collect::<Vec<_>>());
        // Other org's and org-less samples were dropped.
        assert!(u.series.values().flatten().all(|s| s.t_ms >= 1_790_208_000_000));
    }

    #[test]
    fn floats_are_accepted_and_clamped() {
        let u =
            parse(br#"{"version":2,"samples":[{"t":1790208000000,"u":{"fh":12.75,"sd":100.4,"so":-0.5}}]}"#).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 12.75)]);
        assert_eq!(series(&u, "sd"), [s(1_790_208_000_000, 100.0)]);
        assert_eq!(series(&u, "so"), [s(1_790_208_000_000, 0.0)]);
    }

    #[test]
    fn version_handling() {
        // A new schema is reported as such when its structure is unrecognisable.
        assert!(matches!(parse(br#"{"version":3,"data":{"x":1}}"#), Err(SourceError::SchemaChanged(3))));
        assert!(matches!(parse(br#"{"version":3,"samples":{}}"#), Err(SourceError::SchemaChanged(3))));
        assert!(matches!(parse(br#"{"version":1,"samples":[]}"#), Err(SourceError::SchemaChanged(1))));
        assert!(matches!(parse(br#"{"version":0,"samples":[]}"#), Err(SourceError::SchemaChanged(0))));
        let u = parse(br#"{"version":2.0,"samples":[]}"#).unwrap();
        assert_eq!(
            (u.newer_version(), u.health()),
            (None, DesktopHealth::Ok { last_sample_ms: None, newer_version: None })
        );
        for bad in [
            &br#"{"samples":[]}"#[..],
            br#"{"version":"2","samples":[]}"#,
            br#"{"version":-2,"samples":[]}"#,
            br#"{"version":2.5,"samples":[]}"#,
            br#"{"version":99999999999,"samples":[]}"#,
        ] {
            assert!(matches!(parse(bad), Err(SourceError::Parse(_))), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn newer_version_is_read_best_effort() {
        // Unknown top-level fields, sample fields and `u` keys are ignored; `t` and `u` still parse.
        let u = parse(
            br#"{"version":3,"meta":{"k":1},"samples":[
                {"t":1790208000000,"org":"x","u":{"fh":29,"sd":59,"zz_new":4},"src":"app","extra":[1]},
                {"t":1790208900000,"org":"x","u":{"fh":31,"sd":60}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(u.version, 3);
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 29.0), s(1_790_208_900_000, 31.0)]);
        assert_eq!(series(&u, "zz_new"), [s(1_790_208_000_000, 4.0)]);
        assert_eq!(u.newer_version(), Some(3));
        assert_eq!(u.health(), DesktopHealth::Ok { last_sample_ms: Some(1_790_208_900_000), newer_version: Some(3) });
        // An empty samples array has nothing to disagree with.
        assert_eq!(parse(br#"{"version":4,"samples":[]}"#).unwrap().newer_version(), Some(4));
        // Some odd samples next to a recognisable one: the recognisable one is kept.
        let u = parse(br#"{"version":3,"samples":[{"time":1},{"t":1790208000000,"u":{"fh":5}}]}"#).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 5.0)]);
    }

    #[test]
    fn newer_version_without_recognisable_samples_is_a_schema_change() {
        for doc in [
            &br#"{"version":3,"records":[]}"#[..],
            br#"{"version":3,"samples":[{"time":1790208000000,"usage":{"fh":1}}]}"#,
            br#"{"version":3,"samples":[{"t":"2026-09-20","u":{"fh":1}}]}"#,
            br#"{"version":3,"samples":[{"t":1790208000000,"u":[29,59]}]}"#,
            br#"{"version":3,"samples":[{"t":1790208000000,"u":{"fh":"29%"}}]}"#,
            br#"{"version":3,"samples":[[1790208000000,29,59]]}"#,
        ] {
            assert!(matches!(parse(doc), Err(SourceError::SchemaChanged(3))), "{}", String::from_utf8_lossy(doc));
        }
        // Samples that are only too new (clock ahead) do not make the format unrecognised.
        let u = parse_until(br#"{"version":3,"samples":[{"t":1790208000000,"u":{"fh":5}}]}"#, 1).unwrap();
        assert_eq!((u.series.len(), u.newer_version()), (0, Some(3)));
    }

    #[test]
    fn invalid_structure_is_a_parse_error() {
        for bad in [
            &b""[..],
            b"[]",
            b"null",
            br#"{"version":2}"#,
            br#"{"version":2,"samples":{}}"#,
            br#"{"version":2,"samples":[{"t":1790208000000,"u":{"fh":1}}"#,
            b"\xff\xfe{",
        ] {
            assert!(matches!(parse(bad), Err(SourceError::Parse(_))), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn truncated_file_is_a_parse_error() {
        let synth = synth::realistic();
        for cut in [1, synth.json.len() / 3, synth.json.len() / 2, synth.json.len() - 1] {
            assert!(matches!(parse(&synth.json.as_bytes()[..cut]), Err(SourceError::Parse(_))), "cut at {cut}");
        }
    }

    #[test]
    fn utf8_bom_is_tolerated() {
        // Notepad and some Windows tools prepend a BOM when re-saving a UTF-8 file.
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(b"{\"version\":2,\r\n\"samples\":[{\"t\":1790208000000,\"u\":{\"fh\":7}}]}\r\n");
        let u = parse(&bytes).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 7.0)]);
    }

    #[test]
    fn empty_and_unusable_samples() {
        let u = parse(br#"{"version":2,"samples":[]}"#).unwrap();
        assert_eq!(u.series.len(), 0);
        assert_eq!(u.last_sample_ms, None);
        // Every sample lacks a usable `t` or `u`.
        let u = parse(
            br#"{"version":2,"samples":[{"u":{"fh":1}},{"t":1790208000000},{"t":1790208000000,"u":{"fh":"x"}},7]}"#,
        )
        .unwrap();
        assert_eq!(u.series.len(), 0);
        assert_eq!(u.last_sample_ms, None);
    }

    #[test]
    fn unsorted_input_is_sorted_and_duplicates_removed() {
        let u = parse(
            br#"{"version":2,"samples":[
                {"t":1790208900000,"u":{"fh":3}},
                {"t":1790208000000,"u":{"fh":1}},
                {"t":1790209800000,"u":{"fh":5}},
                {"t":1790208000000,"u":{"fh":2}},
                {"t":1790208450000,"u":{"fh":4}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            series(&u, "fh"),
            [
                s(1_790_208_000_000, 2.0),
                s(1_790_208_450_000, 4.0),
                s(1_790_208_900_000, 3.0),
                s(1_790_209_800_000, 5.0)
            ]
        );
        assert_eq!(u.last_sample_ms, Some(1_790_209_800_000));
    }

    #[test]
    fn keeps_only_the_newest_samples_org() {
        let doc = format!(
            r#"{{"version":2,"samples":[
                {{"t":1790208000000,"org":"{ORG_B}","u":{{"fh":70}}}},
                {{"t":1790208900000,"org":"{ORG_A}","u":{{"fh":10}}}},
                {{"t":1790209800000,"u":{{"fh":50}}}},
                {{"t":1790210700000,"org":"{ORG_A}","u":{{"fh":20}}}}
            ]}}"#
        );
        let u = parse(doc.as_bytes()).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_900_000, 10.0), s(1_790_210_700_000, 20.0)]);

        // Newest sample has no org: only org-less samples are kept.
        let doc = format!(
            r#"{{"version":2,"samples":[
                {{"t":1790208000000,"u":{{"fh":5}}}},
                {{"t":1790208900000,"org":"{ORG_A}","u":{{"fh":10}}}},
                {{"t":1790209800000,"org":"","u":{{"fh":7}}}}
            ]}}"#
        );
        let u = parse(doc.as_bytes()).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 5.0), s(1_790_209_800_000, 7.0)]);

        // Equal newest `t` from two orgs: the later entry decides.
        let doc = format!(
            r#"{{"version":2,"samples":[
                {{"t":1790208000000,"org":"{ORG_B}","u":{{"fh":1}}}},
                {{"t":1790208900000,"org":"{ORG_B}","u":{{"fh":2}}}},
                {{"t":1790208900000,"org":"{ORG_A}","u":{{"fh":3}}}}
            ]}}"#
        );
        let u = parse(doc.as_bytes()).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_900_000, 3.0)]);
    }

    #[test]
    fn owner_is_decided_before_unusable_values_are_dropped() {
        // Account switch: the new account's first samples carry no usable value yet. Showing the
        // previous account's usage as current would be wrong.
        let switched = |extra: &str| {
            let doc = format!(
                r#"{{"version":2,"samples":[
                    {{"t":1790208000000,"org":"{ORG_A}","u":{{"fh":70}}}},
                    {{"t":1790208900000,"org":"{ORG_B}","u":{{"fh":"x"}}}},
                    {{"t":1790209800000,"org":"{ORG_B}","u":{{}}}}{extra}
                ]}}"#
            );
            parse(doc.as_bytes()).unwrap()
        };
        let u = switched("");
        assert!(u.series.is_empty(), "{u:?}");
        assert_eq!(u.last_sample_ms, None);
        // A sample without any `u` names its account too.
        let u = switched(&format!(r#",{{"t":1790210700000,"org":"{ORG_B}"}}"#));
        assert!(u.series.is_empty(), "{u:?}");
        // Once the new account reports values, only those are kept.
        let u = switched(&format!(r#",{{"t":1790210700000,"org":"{ORG_B}","u":{{"fh":10}}}}"#));
        assert_eq!(series(&u, "fh"), [s(1_790_210_700_000, 10.0)]);
        assert_eq!(u.last_sample_ms, Some(1_790_210_700_000));
        // A newer valueless sample of the same account keeps its older values.
        let doc = format!(
            r#"{{"version":2,"samples":[
                {{"t":1790208000000,"org":"{ORG_A}","u":{{"fh":70}}}},
                {{"t":1790208900000,"org":"{ORG_A}","u":{{}}}}
            ]}}"#
        );
        let u = parse(doc.as_bytes()).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 70.0)]);
        assert_eq!(u.last_sample_ms, Some(1_790_208_000_000));
        // A sample with neither an org nor values decides nothing.
        let doc = format!(
            r#"{{"version":2,"samples":[
                {{"t":1790208000000,"org":"{ORG_A}","u":{{"fh":70}}}},
                {{"t":1790208900000}},
                {{"t":1790209800000,"org":7,"u":{{"fh":"x"}}}}
            ]}}"#
        );
        let u = parse(doc.as_bytes()).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 70.0)]);
    }

    #[test]
    fn samples_after_max_t_are_ignored() {
        // A clock jump wrote a sample three days ahead, here for another account: it must neither
        // pick the account nor become the newest sample.
        let doc = format!(
            r#"{{"version":2,"samples":[
                {{"t":1790208000000,"org":"{ORG_A}","u":{{"fh":10}}}},
                {{"t":1790208900000,"org":"{ORG_A}","u":{{"fh":12}}}},
                {{"t":1790467200000,"org":"{ORG_B}","u":{{"fh":90}}}}
            ]}}"#
        );
        let u = parse_until(doc.as_bytes(), 1_790_209_200_000).unwrap();
        assert_eq!(series(&u, "fh"), [s(1_790_208_000_000, 10.0), s(1_790_208_900_000, 12.0)]);
        assert_eq!(u.last_sample_ms, Some(1_790_208_900_000));
        assert_eq!(latest_observations(&u)[0].observed_at_ms, 1_790_208_900_000);
        // The limit is inclusive.
        let u = parse_until(doc.as_bytes(), 1_790_208_900_000).unwrap();
        assert_eq!(u.last_sample_ms, Some(1_790_208_900_000));
        // Without a limit the future sample decides.
        assert_eq!(series(&parse(doc.as_bytes()).unwrap(), "fh"), [s(1_790_467_200_000, 90.0)]);
    }

    #[test]
    fn org_never_leaves_parse() {
        let synth = synth::realistic();
        let docs = [EDGE.to_owned(), synth.json.clone()];
        for doc in &docs {
            assert!(doc.contains(ORG_A));
            let u = parse(doc.as_bytes()).unwrap();
            assert_no_org(&format!("{u:?}"));
            assert_no_org(&format!("{u:#?}"));
            for kind in u.series.keys() {
                assert_no_org(kind.key());
            }
            let obs = latest_observations(&u);
            assert_no_org(&format!("{obs:?}"));
            assert_no_org(&serde_json::to_string(&obs).unwrap());
        }
        // Errors never quote the document either.
        let cut = EDGE.find(ORG_A).unwrap() + ORG_A.len() + 1;
        let err = parse(&EDGE.as_bytes()[..cut]).unwrap_err();
        assert_no_org(&format!("{err:?} {err}"));
        let bad = format!(r#"{{"version":2,"samples":[{{"t":1,"org":"{ORG_A}","u":{{"fh":1}}}}],}}"#);
        let err = parse(bad.as_bytes()).unwrap_err();
        assert_no_org(&format!("{err:?} {err}"));
        // A `u` key made from the org is not a plausible window name.
        let sneaky = format!(r#"{{"version":2,"samples":[{{"t":1790208000000,"u":{{"fh":1,"{ORG_A}":2}}}}]}}"#);
        assert_no_org(&format!("{:?}", parse(sneaky.as_bytes()).unwrap()));
    }

    #[test]
    fn realistic_fixture_round_trips() {
        let synth = synth::realistic();
        let u = parse(synth.json.as_bytes()).unwrap();
        let fh = synth.fh();
        let sd = synth.sd();
        assert_eq!(series(&u, "fh"), fh.as_slice());
        assert_eq!(series(&u, "sd"), sd.as_slice());
        assert_eq!(u.series.len(), 2);
        assert_eq!(u.last_sample_ms, fh.last().map(|s| s.t_ms));
        assert!(fh.iter().all(|s| s.t_ms >= START_MS), "older account dropped");
    }

    /// Guards the generator itself: it must keep exercising the shapes seen in real files.
    #[test]
    fn realistic_fixture_has_real_world_shapes() {
        let synth = synth::realistic();
        let fh = synth.fh();
        let pairs = || fh.windows(2).map(|w| (w[0], w[1]));
        assert!(fh.len() > 200, "a few days of samples: {}", fh.len());
        assert!(fh.iter().all(|s| (0.0..=100.0).contains(&s.pct) && s.pct.fract() == 0.0));
        assert!(pairs().any(|(a, b)| a.pct == 100.0 && b.pct == 100.0), "100 % plateau");
        assert!(pairs().any(|(a, b)| b.pct > 1.0 && b.pct < a.pct - 1.0), "reset drop that never touches 0");
        assert!(pairs().any(|(a, b)| b.t_ms - a.t_ms > FIVE_HOURS_MS), "gap longer than the window");
        assert!(
            pairs().any(|(a, b)| (2 * 60 * MINUTE_MS..FIVE_HOURS_MS).contains(&(b.t_ms - a.t_ms))),
            "sleep gap shorter than the window"
        );
        assert!(fh.iter().any(|s| s.t_ms % (15 * MINUTE_MS) != 0), "off-cycle samples");
        let sd = synth.sd();
        assert!(sd.windows(2).any(|w| w[1].pct < w[0].pct - 30.0), "weekly reset drop");
    }

    #[test]
    fn latest_observations_per_window() {
        let u = parse(EDGE.as_bytes()).unwrap();
        let obs = latest_observations(&u);
        assert_eq!(obs.len(), 5);
        let fh = obs.iter().find(|o| o.kind == WindowKind::FiveHour).unwrap();
        assert_eq!(
            *fh,
            Observation {
                kind: WindowKind::FiveHour,
                pct: 100.0,
                resets_at_ms: None,
                observed_at_ms: 1_790_211_600_000,
                source: Source::Desktop,
            }
        );
        let opus = obs.iter().find(|o| o.kind == WindowKind::Other("seven_day_opus".into())).unwrap();
        assert_eq!((opus.pct, opus.observed_at_ms), (12.0, 1_790_208_000_000));
        assert!(obs.iter().all(|o| o.source == Source::Desktop && o.resets_at_ms.is_none()));
        assert!(latest_observations(&parse(br#"{"version":2,"samples":[]}"#).unwrap()).is_empty());
    }

    struct Env {
        _tmp: tempfile::TempDir,
        paths: Paths,
    }

    impl Env {
        /// Two Desktop roots (regular and MSIX-style), neither holding a usage file yet.
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let roots = vec![
                tmp.path().join("Roaming").join("Claude"),
                tmp.path().join("Packages").join("Claude_test").join("LocalCache").join("Roaming").join("Claude"),
            ];
            for r in &roots {
                std::fs::create_dir_all(r).unwrap();
            }
            let paths = Paths::with_roots(tmp.path().join(".claude"), roots, tmp.path().join("data"));
            Self { _tmp: tmp, paths }
        }
        fn write(&self, root: usize, content: &str) {
            let path = self.paths.desktop_roots()[root].join("plan-usage-history.json");
            std::fs::write(path, content).unwrap();
        }
        /// Writes like [`Env::write`] and sets the file's mtime.
        fn write_at(&self, root: usize, content: &str, mtime_ms: Ms) {
            self.write(root, content);
            let path = self.paths.desktop_roots()[root].join("plan-usage-history.json");
            let file = std::fs::File::options().write(true).open(path).unwrap();
            let mtime = UNIX_EPOCH + Duration::from_millis(u64::try_from(mtime_ms).unwrap());
            file.set_modified(mtime).unwrap();
        }
        fn load(&self) -> Result<Option<DesktopUsage>, SourceError> {
            self.load_until(MAX_PLAUSIBLE_MS)
        }
        fn load_until(&self, max_t_ms: Ms) -> Result<Option<DesktopUsage>, SourceError> {
            load(&SafeReader::new(&self.paths), &self.paths, max_t_ms)
        }
    }

    fn doc(last_t: Ms, fh: u32) -> String {
        format!(
            r#"{{"version":2,"samples":[{{"t":{},"org":"{ORG_A}","u":{{"fh":1}}}},{{"t":{last_t},"org":"{ORG_A}","u":{{"fh":{fh}}}}}]}}"#,
            last_t - 15 * MINUTE_MS
        )
    }

    const T: Ms = 1_790_208_000_000;
    const TRUNCATED: &str = r#"{"version":2,"samples":[{"t":1790208000000,"u":{"fh""#;
    /// A newer format that is not readable best-effort.
    const V3: &str = r#"{"version":3,"records":[]}"#;

    #[test]
    fn load_without_files_is_none() {
        let env = Env::new();
        assert!(env.load().unwrap().is_none());
    }

    #[test]
    fn load_prefers_newest_last_sample() {
        let env = Env::new();
        env.write(0, &doc(T, 10));
        assert_eq!(env.load().unwrap().unwrap().last_sample_ms, Some(T));
        env.write(1, &doc(T + MINUTE_MS, 20));
        let u = env.load().unwrap().unwrap();
        assert_eq!(u.last_sample_ms, Some(T + MINUTE_MS));
        assert_eq!(series(&u, "fh").last().map(|s| s.pct), Some(20.0));
        env.write(0, &doc(T + 2 * MINUTE_MS, 30));
        let u = env.load().unwrap().unwrap();
        assert_eq!(series(&u, "fh").last().map(|s| s.pct), Some(30.0));
        // Tie: the first (non-MSIX) root wins.
        env.write(1, &doc(T + 2 * MINUTE_MS, 40));
        let u = env.load().unwrap().unwrap();
        assert_eq!(series(&u, "fh").last().map(|s| s.pct), Some(30.0));
    }

    #[test]
    fn load_skips_broken_files_when_one_is_good() {
        let env = Env::new();
        env.write(0, TRUNCATED);
        env.write(1, &doc(T, 10));
        assert_eq!(env.load().unwrap().unwrap().last_sample_ms, Some(T));
        env.write(0, &doc(T, 10));
        // A v3 file last written before the v2 file's newest sample is a leftover.
        env.write_at(1, V3, T - DAY_MS);
        assert_eq!(env.load().unwrap().unwrap().last_sample_ms, Some(T));
    }

    #[test]
    fn newer_schema_change_beats_a_stale_copy_in_another_root() {
        let env = Env::new();
        // The v2 copy was last sampled two days ago; the running Desktop writes v3 in root 1.
        env.write_at(0, &doc(T - 2 * DAY_MS, 10), T - 2 * DAY_MS);
        env.write_at(1, V3, T);
        assert!(matches!(env.load(), Err(SourceError::SchemaChanged(3))));
        // Order of the roots does not matter.
        env.write_at(0, V3, T);
        env.write_at(1, &doc(T - 2 * DAY_MS, 10), T - 2 * DAY_MS);
        assert!(matches!(env.load(), Err(SourceError::SchemaChanged(3))));
        // The newest sample is the reference, not the v2 file's own (e.g. restored) mtime.
        env.write_at(1, &doc(T - 2 * DAY_MS, 10), T + DAY_MS);
        assert!(matches!(env.load(), Err(SourceError::SchemaChanged(3))));
        // Without samples the v2 file's mtime is the reference.
        env.write_at(1, r#"{"version":2,"samples":[]}"#, T - MINUTE_MS);
        assert!(matches!(env.load(), Err(SourceError::SchemaChanged(3))));
        env.write_at(1, r#"{"version":2,"samples":[]}"#, T + MINUTE_MS);
        assert_eq!(env.load().unwrap().unwrap().last_sample_ms, None);
    }

    #[test]
    fn load_ignores_samples_after_max_t() {
        let env = Env::new();
        // Root 0 holds a sample from a clock three days ahead; root 1 is really the newer one.
        env.write(
            0,
            &format!(
                r#"{{"version":2,"samples":[{{"t":{T},"org":"{ORG_A}","u":{{"fh":10}}}},{{"t":{},"org":"{ORG_A}","u":{{"fh":50}}}}]}}"#,
                T + 3 * DAY_MS
            ),
        );
        env.write(1, &doc(T + 10 * MINUTE_MS, 20));
        let u = env.load_until(T + 20 * MINUTE_MS).unwrap().unwrap();
        assert_eq!(u.last_sample_ms, Some(T + 10 * MINUTE_MS));
        assert_eq!(series(&u, "fh").last().map(|s| s.pct), Some(20.0));
        assert_eq!(env.load().unwrap().unwrap().last_sample_ms, Some(T + 3 * DAY_MS));
    }

    #[test]
    fn load_reads_a_newer_version_best_effort() {
        let env = Env::new();
        env.write(0, &doc(T, 10).replace(r#""version":2"#, r#""version":3"#));
        let u = env.load().unwrap().unwrap();
        assert_eq!((u.last_sample_ms, u.newer_version()), (Some(T), Some(3)));
        // Competes with other roots like any readable file.
        env.write(1, &doc(T + MINUTE_MS, 20));
        let u = env.load().unwrap().unwrap();
        assert_eq!((u.last_sample_ms, u.newer_version()), (Some(T + MINUTE_MS), None));
    }

    #[test]
    fn load_error_precedence() {
        let env = Env::new();
        env.write(0, TRUNCATED);
        assert!(matches!(env.load(), Err(SourceError::Parse(_))));
        env.write(1, V3);
        assert!(matches!(env.load(), Err(SourceError::SchemaChanged(3))), "SchemaChanged beats Parse");
        env.write(0, V3);
        env.write(1, TRUNCATED);
        assert!(matches!(env.load(), Err(SourceError::SchemaChanged(3))));
    }

    #[test]
    fn parse_never_panics_on_garbage() {
        let synth = synth::realistic();
        let bytes = synth.json.as_bytes();
        // Flip bytes across the document; every result must be Ok or a clean error.
        for i in (0..bytes.len()).step_by(97) {
            let mut b = bytes.to_vec();
            b[i] ^= 0x5a;
            let _ = parse(&b);
        }
        for doc in [
            r#"{"version":2,"samples":[{"t":1e300,"u":{"fh":1}}]}"#,
            r#"{"version":2,"samples":[{"t":1790208000000,"u":{"fh":1e308}}]}"#,
            r#"{"version":2,"samples":[{"t":18446744073709551615,"u":{"fh":1}}]}"#,
        ] {
            let _ = parse(doc.as_bytes());
        }
    }
}
