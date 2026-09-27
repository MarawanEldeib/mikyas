//! Property / fuzz tests for every parser of external data, through the crate's public API:
//! statusline captures, Claude Code `settings.json` Connect/Disconnect, statusline command
//! wrapping, transcript tails, Claude Desktop's usage file and the widget's own history file.
//!
//! Each property checks "never panics" plus the invariant the rest of the widget relies on
//! (whitelist only, byte-identical Disconnect, percentages in 0..=100, no message text or account
//! id kept). Case counts are kept small enough for the whole file to run in a few seconds; the
//! modules' own unit tests run the larger suites.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use mikyas_core::capture::{self, CAPTURE_VERSION};
use mikyas_core::claude_settings::{self, SettingsError, Status};
use mikyas_core::cmdline::{self, CmdlineError, ShellKind, WrapMode};
use mikyas_core::engine::types::WindowKind;
use mikyas_core::history::{History, MAX_SPARK_BUCKETS};
use mikyas_core::paths::Paths;
use mikyas_core::saferead::SafeReader;
use mikyas_core::sources::desktop_usage;
use mikyas_core::sources::transcript;
use mikyas_core::time::Ms;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const SID: &str = "00000000-0000-4000-8000-000000000001";
const NOW: Ms = 1_790_000_000_000;
const SHIM: &str = "C:/Users/tester/AppData/Local/Mikyas/bin/mikyas-capture.exe";
/// Text that stands for message content, prompts and other private values: it must never come
/// out of a parser. Contains multi-byte characters so a cut can land inside one.
const MARKER: &str = "zqSECRETé😀zq";

fn shells() -> impl Strategy<Value = ShellKind> {
    prop_oneof![Just(ShellKind::Bash), Just(ShellKind::Cmd), Just(ShellKind::Pwsh), Just(ShellKind::LegacyPowerShell)]
}

/// Any JSON value, with keys and strings drawn from all of Unicode.
fn arb_json(depth: u32) -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(|f| serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)),
        "\\PC{0,12}".prop_map(Value::String),
        Just(Value::String(MARKER.to_owned())),
    ];
    leaf.prop_recursive(depth, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(("\\PC{0,8}", inner), 0..5).prop_map(|kv| Value::Object(kv.into_iter().collect())),
        ]
    })
}

// ---- statusline capture ----

/// Top-level keys a capture file may hold, and the keys allowed inside each nested object.
const CAPTURE_KEYS: &[&str] =
    &["v", "session_id", "written_at_ms", "changed_at_ms", "fingerprint", "model", "context", "rate_limits", "api_ms"];
const MODEL_KEYS: &[&str] = &["id", "display_name"];
const CONTEXT_KEYS: &[&str] = &["used_percentage", "context_window_size", "exceeds_200k"];
const WINDOW_KEYS: &[&str] = &["used_percentage", "resets_at"];

fn keys(v: &Value) -> BTreeSet<&str> {
    v.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default()
}

fn subset(v: &Value, allowed: &[&str]) -> bool {
    keys(v).iter().all(|k| allowed.contains(k))
}

/// A statusline payload: the real top-level shape with arbitrary values, plus arbitrary extra
/// members carrying [`MARKER`] anywhere.
fn arb_statusline() -> impl Strategy<Value = Value> {
    let window = (arb_json(1), arb_json(1), 1_000_000_000i64..7_000_000_000)
        .prop_map(|(extra, pct, resets)| json!({ "used_percentage": pct, "resets_at": resets, "label": extra }));
    let windows = prop::collection::vec(("[a-z_]{1,12}|\\PC{1,6}", window), 0..6)
        .prop_map(|kv| Value::Object(kv.into_iter().collect()));
    (
        windows,
        // `display_name` is whitelisted, so it gets ordinary text rather than the marker.
        prop_oneof!["[A-Za-z0-9 .()]{0,20}".prop_map(Value::from), any::<i64>().prop_map(Value::from)],
        arb_json(2),
        prop::collection::vec(("\\PC{1,10}", arb_json(2)), 0..4),
        0.0f64..150.0,
        any::<bool>(),
    )
        .prop_map(|(rate_limits, model, extra, members, pct, exceeds)| {
            let mut obj = json!({
                "session_id": SID,
                "transcript_path": format!(r"C:\Users\tester\.claude\projects\p\{SID}.jsonl"),
                "cwd": format!(r"C:\Users\tester\{MARKER}"),
                "version": "9.9.9-zq",
                "model": { "id": "claude-opus-5-5[1m]", "display_name": model, "secret": MARKER },
                "workspace": { "current_dir": MARKER, "extra": extra },
                "cost": { "total_api_duration_ms": 5, "total_cost_usd": 1.5, "note": MARKER },
                "context_window": { "used_percentage": pct, "context_window_size": 1_000_000, "label": MARKER },
                "exceeds_200k_tokens": exceeds,
                "rate_limits": rate_limits,
            });
            let map = obj.as_object_mut().unwrap();
            for (k, v) in members {
                // Never overwrite a whitelisted member: those are what is under test.
                map.entry(k).or_insert(v);
            }
            obj
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn capture_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = capture::record_from_bytes(&bytes, NOW);
        let _ = capture::parse_capture(&bytes);
    }

    #[test]
    fn capture_keeps_only_the_whitelist(json in arb_statusline()) {
        let rec = capture::record_from_bytes(&serde_json::to_vec(&json).unwrap(), NOW).unwrap();
        let out = serde_json::to_value(&rec).unwrap();
        let text = out.to_string();
        prop_assert!(subset(&out, CAPTURE_KEYS), "{}", text);
        prop_assert!(subset(&out["model"], MODEL_KEYS), "{}", text);
        prop_assert!(subset(&out["context"], CONTEXT_KEYS), "{}", text);
        for window in out["rate_limits"].as_object().into_iter().flat_map(|o| o.values()) {
            prop_assert!(subset(window, WINDOW_KEYS), "{}", text);
            let pct = window["used_percentage"].as_f64().unwrap();
            prop_assert!((0.0..=100.0).contains(&pct), "{}", text);
        }
        for leaked in ["zqSECRET", "tester", "9.9.9-zq", "total_cost_usd", "workspace", "label"] {
            prop_assert!(!text.contains(leaked), "{} leaked into {}", leaked, text);
        }
        prop_assert_eq!(rec.v, CAPTURE_VERSION);
        // What is written is what is read back.
        prop_assert_eq!(capture::parse_capture(text.as_bytes()), Some(rec));
    }
}

// ---- settings.json Connect / Disconnect ----

/// Arbitrary settings objects, sometimes with a `statusLine` of any shape.
fn arb_settings_object() -> impl Strategy<Value = Map<String, Value>> {
    let status_line = prop_oneof![
        3 => Just(None),
        2 => "\\PC{0,30}".prop_map(|cmd| Some(json!({ "type": "command", "command": cmd }))),
        1 => ("\\PC{0,20}", arb_json(1)).prop_map(|(cmd, extra)| {
            Some(json!({ "padding": extra, "type": "command", "command": cmd, "refreshInterval": 5 }))
        }),
        1 => arb_json(2).prop_map(Some),
    ];
    (prop::collection::vec(("\\PC{0,10}", arb_json(3)), 0..6), status_line, 0usize..8).prop_map(
        |(members, status_line, at)| {
            let mut members: Vec<(String, Value)> =
                members.into_iter().filter(|(k, _)| k != "statusLine").collect::<Map<_, _>>().into_iter().collect();
            if let Some(sl) = status_line {
                members.insert(at.min(members.len()), ("statusLine".to_owned(), sl));
            }
            members.into_iter().collect()
        },
    )
}

fn render(obj: &Map<String, Value>, style: u8) -> Vec<u8> {
    let text = match style % 4 {
        0 => serde_json::to_string_pretty(obj).unwrap(),
        1 => serde_json::to_string(obj).unwrap(),
        2 => serde_json::to_string_pretty(obj).unwrap().replace('\n', "\r\n") + "\r\n",
        _ => format!("\u{FEFF}{}\n", serde_json::to_string_pretty(obj).unwrap()),
    };
    text.into_bytes()
}

fn without_status_line(bytes: &[u8]) -> Map<String, Value> {
    let bytes = bytes.strip_prefix("\u{FEFF}".as_bytes()).unwrap_or(bytes);
    let mut map: Map<String, Value> = serde_json::from_slice(bytes).unwrap();
    map.remove("statusLine");
    map
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn connect_changes_only_the_status_line_and_disconnect_restores_every_byte(
        obj in arb_settings_object(),
        style in any::<u8>(),
        shell in shells(),
    ) {
        let input = render(&obj, style);
        match claude_settings::connect(&input, SHIM, shell, NOW) {
            Ok((out, record)) => {
                let connected = matches!(claude_settings::status(&out), Ok(Status::Connected { .. }));
                prop_assert!(connected);
                prop_assert_eq!(without_status_line(&out), without_status_line(&input));
                prop_assert_eq!(claude_settings::disconnect(&out, Some(&record)).unwrap(), Some(input));
            }
            Err(e) => {
                let expected = matches!(
                    e,
                    SettingsError::UnsupportedStatusLine
                        | SettingsError::Cmdline(CmdlineError::NeedsReview | CmdlineError::Unsupported)
                );
                prop_assert!(expected, "{:?}", e);
            }
        }
    }

    #[test]
    fn settings_parsers_never_panic_on_near_json(obj in arb_settings_object(), cut in any::<prop::sample::Index>()) {
        // Truncated and otherwise damaged files: every entry point returns, none panics.
        let full = render(&obj, 0);
        let cut = &full[..cut.index(full.len() + 1)];
        let _ = claude_settings::status(cut);
        let blank = cut.trim_ascii().is_empty(); // treated as `{}` by design (see `connect`)
        if let Some((out, record)) = claude_settings::connect(cut, SHIM, ShellKind::Bash, NOW).ok().filter(|_| !blank) {
            prop_assert_eq!(claude_settings::disconnect(&out, Some(&record)).unwrap(), Some(cut.to_vec()));
        }
        let _ = claude_settings::disconnect(cut, None);
    }
}

// ---- statusline command wrapping ----

fn arb_shim_path() -> impl Strategy<Value = String> {
    prop::collection::vec("[A-Za-z0-9 ._-]{1,10}|[éøü漢字]{1,3}", 1..4)
        .prop_map(|dirs| format!("C:/Users/tester/{}/mikyas-capture.exe", dirs.join("/")))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn wrap_then_unwrap_recovers_the_command_for_every_shell(
        original in "\\PC{0,40}",
        shim in arb_shim_path(),
        shell in shells(),
    ) {
        prop_assert!(cmdline::validate_shim_path(&shim).is_ok(), "{}", shim);
        match cmdline::wrap(Some(&original), &shim, shell) {
            Ok(wrapped) => {
                let un = cmdline::unwrap(&wrapped.command);
                prop_assert!(un.is_some(), "not recognised: {:?}", wrapped.command);
                let un = un.unwrap();
                prop_assert_eq!(&un.shim_path, &shim);
                prop_assert_eq!(un.mode, wrapped.mode);
                if original.trim().is_empty() {
                    prop_assert_eq!(un.mode, WrapMode::Default);
                } else if cmdline::unwrap(&original).is_none() {
                    prop_assert_eq!(un.original.as_deref(), Some(original.as_str()));
                }
            }
            Err(e) => {
                let expected = matches!(e, CmdlineError::NeedsReview | CmdlineError::Unsupported);
                prop_assert!(expected, "{:?}", e);
            }
        }
    }
}

// ---- transcript tails ----

#[derive(Debug, Clone)]
enum Line {
    /// A complete assistant line.
    Assistant {
        model: &'static str,
        sidechain: bool,
        usage: bool,
        tokens: [u32; 3],
        t: Ms,
        end_turn: bool,
    },
    Prompt {
        t: Ms,
    },
    ToolResult {
        t: Ms,
    },
    Identity {
        model_id: &'static str,
    },
    /// Any bytes without a newline (including invalid UTF-8).
    Garbage(Vec<u8>),
    /// A line cut short (as when Claude Code is mid-write): never parses.
    Cut(Box<Line>, prop::sample::Index),
}

const MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5", "claude-haiku-4-5-20251001", "<synthetic>", ""];
const IDENTITIES: &[&str] = &["claude-opus-5-5[1m]", "claude-sonnet-5", "claude-sonnet-5[1m]"];

fn iso(t: Ms) -> String {
    chrono::DateTime::from_timestamp_millis(t).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Line {
    fn bytes(&self) -> Vec<u8> {
        let v = match self {
            Line::Assistant { model, sidechain, usage, tokens, t, end_turn } => {
                let mut message = json!({
                    "model": model,
                    "role": "assistant",
                    "stop_reason": if *end_turn { "end_turn" } else { "tool_use" },
                    "content": [{ "type": "text", "text": MARKER }],
                });
                if *usage {
                    message["usage"] = json!({
                        "input_tokens": tokens[0],
                        "cache_creation_input_tokens": tokens[1],
                        "cache_read_input_tokens": tokens[2],
                        "output_tokens": 7,
                    });
                }
                json!({
                    "type": "assistant", "isSidechain": sidechain, "sessionId": SID, "entrypoint": "cli",
                    "cwd": r"C:\Users\tester\proj", "timestamp": iso(*t), "message": message,
                })
            }
            Line::Prompt { t } => json!({
                "type": "user", "isSidechain": false, "timestamp": iso(*t),
                "message": { "role": "user", "content": MARKER },
            }),
            Line::ToolResult { t } => json!({
                "type": "user", "isSidechain": false, "timestamp": iso(*t),
                "message": { "role": "user", "content": [{ "type": "tool_result", "content": MARKER }] },
            }),
            Line::Identity { model_id } => json!({
                "type": "attachment", "sessionId": SID,
                "attachment": { "type": "model", "identity": { "modelId": model_id, "displayName": MARKER } },
            }),
            Line::Garbage(bytes) => return bytes.clone(),
            Line::Cut(line, at) => {
                let full = line.bytes();
                // Strictly shorter than the line, so the closing brace is always missing.
                return full[..at.index(full.len())].to_vec();
            }
        };
        serde_json::to_vec(&v).unwrap()
    }

    /// Context tokens and model if this line is a complete qualifying assistant line.
    fn qualifying(&self) -> Option<(&'static str, u64)> {
        match self {
            Line::Assistant { model, sidechain: false, usage: true, tokens, .. }
                if !model.is_empty() && *model != "<synthetic>" =>
            {
                Some((model, tokens.iter().map(|&n| u64::from(n)).sum()))
            }
            _ => None,
        }
    }
}

fn arb_line() -> impl Strategy<Value = Line> {
    let t = NOW - 86_400_000..NOW;
    let complete = prop_oneof![
        6 => (prop::sample::select(MODELS), prop::bool::weighted(0.15), prop::bool::weighted(0.9),
              any::<[u32; 3]>(), t.clone(), any::<bool>())
            .prop_map(|(model, sidechain, usage, tokens, t, end_turn)| {
                Line::Assistant { model, sidechain, usage, tokens, t, end_turn }
            }),
        2 => t.clone().prop_map(|t| Line::Prompt { t }),
        2 => t.prop_map(|t| Line::ToolResult { t }),
        1 => prop::sample::select(IDENTITIES).prop_map(|model_id| Line::Identity { model_id }),
    ];
    prop_oneof![
        8 => complete.clone(),
        1 => prop::collection::vec(any::<u8>().prop_filter("no newline", |b| *b != b'\n'), 0..40).prop_map(Line::Garbage),
        1 => (complete, any::<prop::sample::Index>()).prop_map(|(l, at)| Line::Cut(Box::new(l), at)),
    ]
}

struct TranscriptDir {
    _tmp: tempfile::TempDir,
    reader: SafeReader,
    file: std::path::PathBuf,
}

fn transcript_dir() -> TranscriptDir {
    let tmp = tempfile::tempdir().unwrap();
    let claude = tmp.path().join("claude");
    let project = claude.join("projects").join("C--Users-tester-proj");
    fs::create_dir_all(&project).unwrap();
    let paths = Paths::with_roots(claude, vec![], tmp.path().join("data"));
    let reader = SafeReader::new(&paths);
    TranscriptDir { file: project.join(format!("{SID}.jsonl")), reader, _tmp: tmp }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn transcript_tail_matches_the_last_qualifying_line(
        lines in prop::collection::vec(arb_line(), 0..24),
        crlf in any::<bool>(),
        trailing_newline in any::<bool>(),
    ) {
        let dir = transcript_dir();
        let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
        let mut bytes = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                bytes.extend_from_slice(eol);
            }
            bytes.extend(line.bytes());
        }
        if trailing_newline {
            bytes.extend_from_slice(eol);
        }
        fs::write(&dir.file, &bytes).unwrap();

        let mut identity = None;
        let tail = transcript::scan_tail(&dir.reader, &dir.file, &mut identity).unwrap();
        let expected = lines.iter().rev().find_map(Line::qualifying);
        match (&tail, expected) {
            (None, None) => {}
            (Some(tail), Some((model, tokens))) => {
                prop_assert_eq!(tail.model_id.as_deref(), Some(model));
                prop_assert_eq!(tail.ctx_tokens, tokens);
                prop_assert!(tail.max_ctx_tokens_seen >= tail.ctx_tokens);
                prop_assert_eq!(&tail.session_id, SID);
                prop_assert_eq!(tail.project.as_deref(), Some("proj"));
                // The head scan is tolerant by design: a cut attachment line still counts while
                // its id is complete.
                let has_1m_identity = lines.iter().any(|l| l.bytes().windows(5).any(|w| w == b"[1m]\""));
                if tail.identity_1m == Some(true) {
                    prop_assert!(has_1m_identity);
                }
                if let (Some(end), Some(start), true) = (tail.turn.ended_ms, tail.turn.started_ms, tail.turn.start_is_lower_bound) {
                    prop_assert!(start <= end);
                }
                let debug = format!("{tail:?}");
                prop_assert!(!debug.contains("zqSECRET"), "message text kept: {}", debug);
            }
            (tail, expected) => prop_assert!(false, "tail {:?}, expected {:?}", tail, expected),
        }
    }

    #[test]
    fn transcript_tail_never_panics_on_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let dir = transcript_dir();
        fs::write(&dir.file, &bytes).unwrap();
        let mut identity = None;
        prop_assert!(transcript::scan_tail(&dir.reader, &dir.file, &mut identity).is_ok());
    }
}

// ---- Claude Desktop plan-usage-history.json ----

const ORGS: &[&str] = &["11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222"];

fn arb_usage_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        4 => (0u8..=100).prop_map(Value::from),
        2 => (-50.0f64..200.0).prop_map(Value::from),
        1 => any::<f64>().prop_map(|f| serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)),
        1 => arb_json(1),
    ]
}

fn arb_sample() -> impl Strategy<Value = Value> {
    let t = prop_oneof![
        6 => (NOW - 10 * 86_400_000..NOW).prop_map(Value::from),
        1 => any::<i64>().prop_map(Value::from),
        1 => Just(Value::from("2026-09-24T00:00:00Z")),
        1 => arb_json(1),
    ];
    let org = prop_oneof![4 => prop::sample::select(ORGS).prop_map(Value::from), 1 => arb_json(0)];
    let key = prop_oneof![
        4 => prop::sample::select(&["fh", "sd", "so", "sn"][..]).prop_map(str::to_owned),
        1 => "[a-z_]{1,40}",
        1 => "\\PC{0,6}",
    ];
    let u = prop_oneof![
        6 => prop::collection::vec((key, arb_usage_value()), 0..5)
            .prop_map(|kv| Value::Object(kv.into_iter().collect())),
        1 => arb_json(1),
    ];
    (t, org, u, any::<(bool, bool)>(), arb_json(1)).prop_map(|(t, org, u, (drop_t, drop_org), extra)| {
        let mut s = json!({ "t": t, "org": org, "u": u, "x": extra });
        if drop_t {
            s.as_object_mut().unwrap().remove("t");
        }
        if drop_org {
            s.as_object_mut().unwrap().remove("org");
        }
        s
    })
}

fn arb_desktop_doc() -> impl Strategy<Value = Value> {
    let samples = prop_oneof![
        8 => prop::collection::vec(prop_oneof![9 => arb_sample(), 1 => arb_json(1)], 0..30).prop_map(Value::Array),
        1 => arb_json(2),
    ];
    let version = prop_oneof![8 => Just(json!(2)), 1 => Just(json!(2.0)), 1 => arb_json(0)];
    (version, samples, arb_json(2))
        .prop_map(|(version, samples, extra)| json!({ "version": version, "extra": extra, "samples": samples }))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn desktop_usage_parse_keeps_its_invariants(doc in arb_desktop_doc(), cut in any::<prop::sample::Index>()) {
        let bytes = serde_json::to_vec(&doc).unwrap();
        if let Ok(usage) = desktop_usage::parse(&bytes) {
            prop_assert_eq!(usage.version, desktop_usage::SUPPORTED_VERSION);
            let mut newest = None;
            for (kind, samples) in &usage.series {
                let key = kind.key();
                let plausible = !key.is_empty()
                    && key.len() <= 32
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                prop_assert!(plausible || matches!(kind, WindowKind::Other(k) if k.starts_with("seven_day_")), "{:?}", kind);
                prop_assert!(!samples.is_empty());
                prop_assert!(samples.windows(2).all(|w| w[0].t_ms < w[1].t_ms), "not strictly ascending");
                for s in samples {
                    prop_assert!(s.pct.is_finite() && (0.0..=100.0).contains(&s.pct), "{}", s.pct);
                }
                newest = newest.max(samples.last().map(|s| s.t_ms));
            }
            prop_assert_eq!(usage.last_sample_ms, newest);
            let debug = format!("{usage:?}");
            for org in ORGS {
                prop_assert!(!debug.contains(org), "org kept: {}", debug);
            }
        }
        // A truncated file (Desktop mid-write) is an error, never a panic.
        let _ = desktop_usage::parse(&bytes[..cut.index(bytes.len())]);
    }

    #[test]
    fn desktop_usage_parse_never_panics_on_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = desktop_usage::parse(&bytes);
    }
}

// ---- history.jsonl ----

fn arb_history_line() -> impl Strategy<Value = (Vec<u8>, bool)> {
    let row =
        (NOW - 86_400_000..NOW, prop::sample::select(&["5h", "7d", "so", "x"][..]), -10.0f32..120.0, any::<bool>())
            .prop_map(|(t, w, p, desktop)| {
                let s = if desktop { "desktop" } else { "cli" };
                (serde_json::to_vec(&json!({ "t": t, "w": w, "p": p, "r": null, "s": s, "e": false })).unwrap(), true)
            });
    prop_oneof![
        6 => row,
        1 => prop::collection::vec(any::<u8>().prop_filter("no newline", |b| *b != b'\n'), 0..60).prop_map(|b| (b, false)),
        1 => arb_json(2).prop_map(|v| (serde_json::to_vec(&v).unwrap(), false)),
    ]
}

fn open_history(dir: &Path, bytes: &[u8]) -> History {
    let path = dir.join("history.jsonl");
    fs::write(&path, bytes).unwrap();
    History::open(path).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn history_loads_every_valid_row_and_skips_the_rest(lines in prop::collection::vec(arb_history_line(), 0..30)) {
        let tmp = tempfile::tempdir().unwrap();
        let mut bytes = Vec::new();
        for (line, _) in &lines {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
        let history = open_history(tmp.path(), &bytes);
        let rows = history.rows();
        // Random bytes can happen to be a valid row too, so this is a lower bound.
        let valid = lines.iter().filter(|(_, valid)| *valid).count();
        prop_assert!(rows.len() >= valid, "{} rows < {} valid lines", rows.len(), valid);
        prop_assert!(rows.windows(2).all(|w| w[0].t <= w[1].t), "rows not sorted");
        for row in rows {
            prop_assert!(row.p.is_finite() && (0.0..=100.0).contains(&row.p), "{}", row.p);
        }
    }

    #[test]
    fn history_open_never_panics_on_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let tmp = tempfile::tempdir().unwrap();
        let history = open_history(tmp.path(), &bytes);
        prop_assert!(history.rows().windows(2).all(|w| w[0].t <= w[1].t));
    }

    #[test]
    fn history_spark_handles_any_range(
        from in any::<i64>(),
        to in any::<i64>(),
        buckets in prop_oneof![0usize..200, Just(MAX_SPARK_BUCKETS + 1), Just(usize::MAX)],
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let row = |t: Ms, p: f32| serde_json::to_vec(&json!({ "t": t, "w": "5h", "p": p, "s": "cli" })).unwrap();
        let mut bytes = Vec::new();
        for (t, p) in [(i64::MIN, 1.0), (NOW, 20.0), (NOW + 60_000, 30.0), (i64::MAX, 99.0)] {
            bytes.extend(row(t, p));
            bytes.push(b'\n');
        }
        let history = open_history(tmp.path(), &bytes);
        let spark = history.spark(&WindowKind::FiveHour, from, to, buckets);
        if to <= from || buckets == 0 {
            prop_assert!(spark.is_empty());
        } else {
            prop_assert_eq!(spark.len(), buckets.min(MAX_SPARK_BUCKETS));
            prop_assert!(spark.iter().all(|p| p.t_ms >= from && p.t_ms <= to));
            prop_assert!(spark.windows(2).all(|w| w[0].t_ms <= w[1].t_ms));
        }
    }
}

#[test]
fn history_append_after_a_torn_line_keeps_every_row() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("history.jsonl");
    fs::write(
        &path,
        b"{\"t\":1790000000000,\"w\":\"5h\",\"p\":10.0,\"r\":null,\"s\":\"cli\",\"e\":false}\n{\"t\":17900",
    )
    .unwrap();
    let mut history = History::open(path.clone()).unwrap();
    assert_eq!(history.rows().len(), 1);
    let state = mikyas_core::engine::types::WindowState {
        kind: WindowKind::FiveHour,
        pct: 12.0,
        reset: mikyas_core::engine::types::ResetInfo::Unknown,
        source: mikyas_core::engine::types::Source::Cli,
        observed_at_ms: NOW + 60_000,
        stale: false,
        limit_reached: false,
        phase: mikyas_core::engine::types::Phase::Active,
    };
    assert!(history.record(&state).unwrap());
    let reopened = History::open(path).unwrap();
    assert_eq!(reopened.rows(), history.rows());
    assert_eq!(reopened.rows().len(), 2);
}
