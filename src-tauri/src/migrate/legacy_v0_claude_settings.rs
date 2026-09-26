//! Connect / Disconnect: edits Claude Code's `settings.json` so its statusline runs through the
//! capture shim — changing NOTHING else, byte for byte.
//!
//! Approach: SURGICAL SPLICING, never re-serialisation. Parse with serde_json only to validate and
//! to read values; then locate byte spans with a small JSON scanner and splice:
//! - Connect with an existing `statusLine` object whose `type` is `"command"`: replace only the
//!   span of the `statusLine.command` string literal with the JSON-escaped wrapped command
//!   (`serde_json::to_string`). All other bytes — key order, indentation, CRLF, BOM, trailing
//!   newline, other `statusLine` keys like `padding` / `refreshInterval` — are untouched.
//! - Connect with no `statusLine` key: insert a member `"statusLine": {"type": "command",
//!   "command": "<wrapped>"}` as the LAST member of the top-level object, formatted to match the
//!   file's detected indentation and line endings (2-space + LF for `JSON.stringify(x, null, 2)`
//!   files). An empty object `{` W `}` becomes `{` NL indent MEMBER NL `}`; a non-empty W (`{ }`,
//!   `{\n}`) is kept in [`WrapRecord::empty_object_ws`] instead of the file.
//! - `statusLine` present but not `type: "command"` (or not an object) → `UnsupportedStatusLine`.
//! - Already connected (the command is recognised by `cmdline::unwrap`) with the same shim path
//!   and a mode valid for `shell` → no change (idempotent: returns the input bytes). Different
//!   shim path or shell → re-wrap the recorded original.
//! - Files that don't parse as strict JSON (comments / trailing commas = JSONC) → `NotStrictJson`.
//!   Top level not an object → `NotAnObject`.
//!
//! The [`WrapRecord`] keeps the RAW JSON literal of the original `command` (exact bytes including
//! quotes and escapes), so Disconnect can restore it byte-for-byte.
//!
//! Disconnect:
//! - Current command not recognised as ours → `Ok(None)` (leave untouched).
//! - Recognised, and its original text equals `wrap.original_command` → splice
//!   `wrap.original_command_raw` back.
//! - Recognised, but the user edited the tail since (differs from the record, or no record) →
//!   splice `serde_json::to_string(tail)`.
//! - Recognised Default form (we inserted the statusLine) → remove the whole `"statusLine"` member
//!   including its separating comma and the whitespace/newline before it, so a file that was only
//!   touched by Connect returns to its exact original bytes. If it is the only member and the
//!   record has `empty_object_ws`, the object's inside becomes that whitespace again.
//!
//! `disconnect(connect(x).bytes, record) == x` must hold byte-for-byte for every non-blank input
//! `x` that connect accepts (property/round-trip tests with LF, CRLF, BOM, 2/4-space and tab
//! indents, unicode and escaped characters, `{}`, `{ }`, `{\n}` and one-line files).
//!
//! Details the rules above leave open:
//! - "Already connected" means `wrap(original, shim, shell)` reproduces the current command
//!   exactly; anything else is re-wrapped.
//! - Blank input — empty (missing file), or only JSON whitespace after an optional BOM — holds no
//!   settings: status is `NotConfigured`, Disconnect leaves it alone, and Connect writes the same
//!   file as for `{}\n`, without a BOM (there is no content whose encoding to keep).
//!   Disconnecting that result yields `{}\n` (a valid settings file) rather than the blank input.
//! - Connect on an already-connected file cannot see the original literal (or the whitespace of
//!   an emptied object) any more; its record then carries the canonical encoding
//!   `serde_json::to_string(original)` and no `empty_object_ws`. Callers should keep the record
//!   from the first connect when the original is unchanged; without `empty_object_ws`, Disconnect
//!   leaves `{}` where Connect replaced `{ }` / `{\n}`.
//! - A Default form that replaced a blank `command` (e.g. `""`) is restored from the record's raw
//!   literal. Without such a record, a statusLine holding only `type` + `command` is removed, and
//!   one with other keys keeps them and gets `"command": ""`.
//! - Duplicate `statusLine` keys (or duplicate `type` / `command` inside it) are rejected as
//!   `NotStrictJson`: the edit target would be ambiguous.
//! - Every edit is re-parsed and checked (only `statusLine` changed, command as intended) before
//!   it is returned; a failed check is reported as `NotStrictJson` rather than written.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::cmdline::{self, CmdlineError, ShellKind, WrapMode};
use sovawatch_core::time::Ms;

/// What Connect changed, persisted in `wrap.json` so Disconnect can undo it exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WrapRecord {
    /// Decoded original `statusLine.command`; `None` if there was no statusLine.
    pub original_command: Option<String>,
    /// Exact JSON literal of the original command as it appeared in the file (with quotes).
    pub original_command_raw: Option<String>,
    /// The whitespace inside an empty top-level object (`{ }`, `{\n}`) that Connect replaced
    /// with the new member's lines. Missing from `wrap.json` files of earlier versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_object_ws: Option<String>,
    pub shim_path: String,
    pub mode: WrapMode,
    pub shell: ShellKind,
    pub at_ms: Ms,
}

/// How the statusline in `settings.json` is configured right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotConfigured,
    /// A statusLine the widget did not create (`command` is `None` for non-command types).
    Foreign {
        command: Option<String>,
    },
    Connected {
        mode: WrapMode,
        shim_path: String,
        original: Option<String>,
    },
}

/// Why `settings.json` could not be read or edited.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("settings.json is not strict JSON (comments or trailing commas?) — edit it manually")]
    NotStrictJson,
    #[error("settings.json top level is not an object")]
    NotAnObject,
    #[error("statusLine is not a command-type statusline")]
    UnsupportedStatusLine,
    #[error(transparent)]
    Cmdline(#[from] CmdlineError),
}

/// Blank input (a missing file, or only whitespace after an optional BOM) → `NotConfigured`.
pub fn status(bytes: &[u8]) -> Result<Status, SettingsError> {
    if is_blank(bytes) {
        return Ok(Status::NotConfigured);
    }
    let doc = Doc::parse(bytes)?;
    Ok(match doc.status_line()? {
        StatusLine::Absent => Status::NotConfigured,
        StatusLine::Other => Status::Foreign { command: None },
        StatusLine::Command { command, .. } => match cmdline::unwrap(&command) {
            Some(outer) => Status::Connected {
                mode: outer.mode,
                shim_path: outer.shim_path,
                original: innermost_original(&command),
            },
            None => Status::Foreign { command: Some(command) },
        },
    })
}

/// Returns the new file bytes and the record to persist in `wrap.json`. Blank input (see
/// [`status`]) is treated as `{}` and produces a minimal file `{\n  "statusLine": …\n}\n`,
/// without a BOM.
pub fn connect(
    bytes: &[u8],
    shim_path: &str,
    shell: ShellKind,
    now_ms: Ms,
) -> Result<(Vec<u8>, WrapRecord), SettingsError> {
    cmdline::validate_shim_path(shim_path)?;
    if is_blank(bytes) {
        return connect(b"{}\n", shim_path, shell, now_ms);
    }
    let doc = Doc::parse(bytes)?;
    let record = |original_command: Option<String>, original_command_raw: Option<String>, mode: WrapMode| WrapRecord {
        original_command,
        original_command_raw,
        empty_object_ws: None,
        shim_path: shim_path.to_owned(),
        mode,
        shell,
        at_ms: now_ms,
    };
    match doc.status_line()? {
        StatusLine::Other => Err(SettingsError::UnsupportedStatusLine),
        StatusLine::Absent => {
            let wrapped = cmdline::wrap(None, shim_path, shell)?;
            let out = doc.insert_status_line(&json_string(&wrapped.command)?)?;
            verify(&doc, &out, Some(&wrapped.command))?;
            let inside = doc.text_at(doc.top.open + 1..doc.top.close);
            let empty_object_ws = (doc.top.members.is_empty() && !inside.is_empty()).then(|| inside.to_owned());
            Ok((out, WrapRecord { empty_object_ws, ..record(None, None, wrapped.mode) }))
        }
        StatusLine::Command { literal, command, .. } => {
            let (original, raw) = if cmdline::unwrap(&command).is_some() {
                let original = innermost_original(&command);
                let raw = original.as_deref().map(json_string).transpose()?;
                (original, raw)
            } else {
                (Some(command.clone()), Some(doc.text_at(literal.clone()).to_owned()))
            };
            let wrapped = cmdline::wrap(original.as_deref(), shim_path, shell)?;
            let rec = record(original, raw, wrapped.mode);
            if wrapped.command == command {
                return Ok((bytes.to_vec(), rec));
            }
            let out = doc.splice(literal, &json_string(&wrapped.command)?)?;
            verify(&doc, &out, Some(&wrapped.command))?;
            Ok((out, rec))
        }
    }
}

/// `Ok(None)` when there is nothing of ours to undo (including blank input).
pub fn disconnect(bytes: &[u8], wrap: Option<&WrapRecord>) -> Result<Option<Vec<u8>>, SettingsError> {
    if is_blank(bytes) {
        return Ok(None);
    }
    let doc = Doc::parse(bytes)?;
    let StatusLine::Command { index, literal, command, object } = doc.status_line()? else {
        return Ok(None);
    };
    if cmdline::unwrap(&command).is_none() {
        return Ok(None);
    }
    let out = match innermost_original(&command) {
        Some(original) => {
            let raw = match recorded_raw(wrap, |o| o == original) {
                Some(raw) => raw.to_owned(),
                None => json_string(&original)?,
            };
            let out = doc.splice(literal, &raw)?;
            verify(&doc, &out, Some(&original))?;
            out
        }
        None => {
            if let Some(raw) = recorded_raw(wrap, |o| o.trim().is_empty()) {
                let out = doc.splice(literal, raw)?;
                verify(&doc, &out, Some(&decode_literal(raw)?))?;
                out
            } else if object.members.iter().all(|m| m.key == "type" || m.key == "command") {
                let out = doc.remove_member(index, recorded_empty_ws(wrap))?;
                verify(&doc, &out, None)?;
                out
            } else {
                let out = doc.splice(literal, "\"\"")?;
                verify(&doc, &out, Some(""))?;
                out
            }
        }
    };
    Ok(Some(out))
}

const BOM: &[u8] = b"\xEF\xBB\xBF";
const STATUS_LINE: &str = "statusLine";

/// Only JSON whitespace: space, tab, CR, LF.
fn is_json_ws(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

/// Only JSON whitespace, optionally after a BOM: a file with no settings.
fn is_blank(bytes: &[u8]) -> bool {
    is_json_ws(bytes.strip_prefix(BOM).unwrap_or(bytes))
}

/// A strictly valid settings document with the byte spans of its top-level members.
struct Doc<'a> {
    bytes: &'a [u8],
    /// Length of the UTF-8 BOM (0 or 3). Spans are relative to `text`, which follows it.
    bom: usize,
    text: &'a str,
    map: Map<String, Value>,
    top: json::Object,
}

enum StatusLine {
    Absent,
    /// Present, but not an object with `"type": "command"` and a string `command`.
    Other,
    Command {
        /// Index of the `statusLine` member in the top-level object.
        index: usize,
        /// Span of the `command` string literal (with quotes).
        literal: Range<usize>,
        command: String,
        object: json::Object,
    },
}

impl<'a> Doc<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, SettingsError> {
        let bom = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
        let text = bytes.get(bom..).and_then(|b| std::str::from_utf8(b).ok()).ok_or(SettingsError::NotStrictJson)?;
        let value: Value = serde_json::from_str(text).map_err(|_| SettingsError::NotStrictJson)?;
        let Value::Object(map) = value else {
            return Err(SettingsError::NotAnObject);
        };
        let top = json::document(text.as_bytes()).map_err(|_| SettingsError::NotStrictJson)?;
        Ok(Self { bytes, bom, text, map, top })
    }

    fn text_at(&self, span: Range<usize>) -> &'a str {
        self.text.get(span).unwrap_or("")
    }

    fn status_line(&self) -> Result<StatusLine, SettingsError> {
        let mut found = self.top.members.iter().enumerate().filter(|(_, m)| m.key == STATUS_LINE);
        let first = found.next();
        if found.next().is_some() || first.is_some() != self.map.contains_key(STATUS_LINE) {
            return Err(SettingsError::NotStrictJson);
        }
        let Some((index, member)) = first else {
            return Ok(StatusLine::Absent);
        };
        let Some(Value::Object(sl)) = self.map.get(STATUS_LINE) else {
            return Ok(StatusLine::Other);
        };
        let object =
            json::object_at(self.text.as_bytes(), member.value.start).map_err(|_| SettingsError::NotStrictJson)?;
        let count = |key: &str| object.members.iter().filter(|m| m.key == key).count();
        if count("type") > 1 || count("command") > 1 {
            return Err(SettingsError::NotStrictJson);
        }
        let (Some("command"), Some(Value::String(command))) =
            (sl.get("type").and_then(Value::as_str), sl.get("command"))
        else {
            return Ok(StatusLine::Other);
        };
        let literal = object
            .members
            .iter()
            .find(|m| m.key == "command")
            .map(|m| m.value.clone())
            .ok_or(SettingsError::NotStrictJson)?;
        // The scanner's view of the literal must agree with serde's.
        if decode_literal(self.text_at(literal.clone()))? != *command {
            return Err(SettingsError::NotStrictJson);
        }
        Ok(StatusLine::Command { index, literal, command: command.clone(), object })
    }

    /// The file bytes with `span` (relative to `text`) replaced by `with`.
    fn splice(&self, span: Range<usize>, with: &str) -> Result<Vec<u8>, SettingsError> {
        let (start, end) = (self.bom + span.start, self.bom + span.end);
        let (Some(head), Some(tail)) = (self.bytes.get(..start), self.bytes.get(end..)) else {
            return Err(SettingsError::NotStrictJson);
        };
        let mut out = Vec::with_capacity(self.bytes.len() + with.len());
        out.extend_from_slice(head);
        out.extend_from_slice(with.as_bytes());
        out.extend_from_slice(tail);
        Ok(out)
    }

    /// Appends `"statusLine": {"type": "command", "command": <literal>}` as the last top-level
    /// member, in the file's own style.
    fn insert_status_line(&self, literal: &str) -> Result<Vec<u8>, SettingsError> {
        let nl = line_ending(self.text);
        let members = &self.top.members;
        let Some(last) = members.last() else {
            // `{` W `}` → `{` NL unit MEMBER NL `}`; W goes into the record, so Disconnect can
            // put it back.
            let unit = indent_unit(self.text, None);
            let member = pretty_member(literal, &unit, nl);
            return self.splice(self.top.open + 1..self.top.close, &format!("{nl}{unit}{member}{nl}"));
        };
        let before_last = members.len().checked_sub(2).map_or(self.top.open + 1, |i| members[i].value.end);
        let at = last.value.end;
        if self.text_at(before_last..last.key_span.start).contains('\n') {
            let unit = indent_unit(self.text, Some(last));
            let member = pretty_member(literal, &unit, nl);
            self.splice(at..at, &format!(",{nl}{unit}{member}"))
        } else {
            // One-line object: add no line breaks; mirror the spacing after the first `:`.
            let first = &members[0];
            let sp = if self.text_at(first.key_span.end..first.value.start).len() > 1 { " " } else { "" };
            self.splice(
                at..at,
                &format!(",{sp}\"statusLine\":{sp}{{\"type\":{sp}\"command\",{sp}\"command\":{sp}{literal}}}"),
            )
        }
    }

    /// Removes top-level member `index` together with the separator that joins it to its
    /// neighbours (the inverse of [`Doc::insert_status_line`]). An only member with `empty_ws`
    /// leaves `{` `empty_ws` `}`.
    fn remove_member(&self, index: usize, empty_ws: Option<&str>) -> Result<Vec<u8>, SettingsError> {
        let members = &self.top.members;
        let member = members.get(index).ok_or(SettingsError::NotStrictJson)?;
        if let (1, Some(ws)) = (members.len(), empty_ws) {
            return self.splice(self.top.open + 1..self.top.close, ws);
        }
        let span = if members.len() == 1 {
            let tail = self.text_at(member.value.end..self.text.len());
            let eol = if tail.starts_with("\r\n") { 2 } else { usize::from(tail.starts_with('\n')) };
            self.top.open + 1..member.value.end + eol
        } else if index > 0 {
            members[index - 1].value.end..member.value.end
        } else {
            member.key_span.start..members[1].key_span.start
        };
        self.splice(span, "")
    }
}

/// `record.original_command_raw` if the record's original satisfies `matches` and the raw text
/// really is one JSON string literal of it (the record comes from disk and may be stale).
fn recorded_raw(record: Option<&WrapRecord>, matches: impl Fn(&str) -> bool) -> Option<&str> {
    let record = record?;
    let original = record.original_command.as_deref().filter(|o| matches(o))?;
    let raw = record.original_command_raw.as_deref()?;
    (decode_literal(raw).ok()? == original).then_some(raw)
}

/// `record.empty_object_ws` if it is non-empty and only JSON whitespace (it comes from disk too).
fn recorded_empty_ws(record: Option<&WrapRecord>) -> Option<&str> {
    record?.empty_object_ws.as_deref().filter(|ws| !ws.is_empty() && is_json_ws(ws.as_bytes()))
}

/// Decodes a complete JSON string literal (quotes included, nothing around it).
fn decode_literal(raw: &str) -> Result<String, SettingsError> {
    match json::string(raw.as_bytes(), 0) {
        Ok((end, value)) if end == raw.len() => Ok(value),
        _ => Err(SettingsError::NotStrictJson),
    }
}

fn json_string(s: &str) -> Result<String, SettingsError> {
    serde_json::to_string(s).map_err(|_| SettingsError::NotStrictJson)
}

/// The user's command inside (possibly repeated) wrapping; `None` for the Default form.
fn innermost_original(command: &str) -> Option<String> {
    let mut current = command.to_owned();
    // Each unwrap strictly shortens the text, so this terminates.
    while let Some(inner) = cmdline::unwrap(&current) {
        current = inner.original?;
    }
    Some(current)
}

/// Re-parses an edit and checks that it changed nothing but `statusLine`, and that the
/// statusLine now runs `command` (or, for `None`, is gone).
fn verify(before: &Doc<'_>, out: &[u8], command: Option<&str>) -> Result<(), SettingsError> {
    let fail = SettingsError::NotStrictJson;
    let after = Doc::parse(out).map_err(|_| fail.clone())?;
    let others_equal = |a: &Map<String, Value>, b: &Map<String, Value>| {
        a.iter().filter(|(k, _)| *k != STATUS_LINE).all(|(k, v)| b.get(k) == Some(v))
    };
    if after.bom != before.bom || !others_equal(&before.map, &after.map) || !others_equal(&after.map, &before.map) {
        return Err(fail);
    }
    match command {
        None => {
            if after.map.contains_key(STATUS_LINE) {
                return Err(fail);
            }
        }
        Some(expected) => {
            let StatusLine::Command { command, .. } = after.status_line()? else {
                return Err(fail);
            };
            if command != expected {
                return Err(fail);
            }
            // Other statusLine keys (padding, refreshInterval, …) are untouched.
            if let (Some(Value::Object(old)), Some(Value::Object(new))) =
                (before.map.get(STATUS_LINE), after.map.get(STATUS_LINE))
            {
                let rest = |m: &Map<String, Value>| {
                    m.iter()
                        .filter(|(k, _)| *k != "command")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Map<_, _>>()
                };
                if rest(old) != rest(new) {
                    return Err(fail);
                }
            }
        }
    }
    Ok(())
}

/// `"\r\n"` if the file's first line break is CRLF, else `"\n"`.
fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes().get(i - 1) == Some(&b'\r') => "\r\n",
        _ => "\n",
    }
}

/// One indentation level: the leading whitespace of the last member's line when it starts its
/// own line, else of the first indented line in the file, else two spaces.
fn indent_unit(text: &str, last: Option<&json::Member>) -> String {
    let is_indent = |ws: &str| !ws.is_empty() && ws.bytes().all(|b| b == b' ' || b == b'\t');
    if let Some(member) = last {
        let before = text.get(..member.key_span.start).unwrap_or("");
        if let Some(nl) = before.rfind('\n') {
            let ws = &before[nl + 1..];
            if is_indent(ws) {
                return ws.to_owned();
            }
        }
    }
    for line in text.split('\n').skip(1) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let body = line.trim_start_matches([' ', '\t']);
        if !body.is_empty() && body.len() < line.len() {
            return line[..line.len() - body.len()].to_owned();
        }
    }
    "  ".to_owned()
}

/// `"statusLine": {…}` laid out like `JSON.stringify(x, null, unit)` one level deep.
fn pretty_member(literal: &str, unit: &str, nl: &str) -> String {
    format!(
        "\"statusLine\": {{{nl}{unit}{unit}\"type\": \"command\",{nl}{unit}{unit}\"command\": {literal}{nl}{unit}}}"
    )
}

/// A minimal strict-JSON scanner that reports byte spans. Input is expected to be valid JSON
/// already (serde_json validates first), but every malformed input is still reported as
/// [`json::Invalid`] rather than panicking.
mod json {
    use std::ops::Range;

    /// Deeper than serde_json's own recursion limit (128), so every document it accepts scans.
    const MAX_DEPTH: usize = 256;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Invalid;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Member {
        /// Decoded key.
        pub key: String,
        /// The key's string literal, quotes included.
        pub key_span: Range<usize>,
        pub value: Range<usize>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Object {
        /// Index of `{`.
        pub open: usize,
        /// Index of `}`.
        pub close: usize,
        pub members: Vec<Member>,
    }

    /// Scans a whole document whose top-level value is an object.
    pub(super) fn document(s: &[u8]) -> Result<Object, Invalid> {
        let obj = object(s, skip_ws(s, 0), 0)?;
        if skip_ws(s, obj.close + 1) != s.len() {
            return Err(Invalid);
        }
        Ok(obj)
    }

    /// Scans the object starting at `s[at] == b'{'` (a nested one; depth is not tracked here
    /// beyond the limit).
    pub(super) fn object_at(s: &[u8], at: usize) -> Result<Object, Invalid> {
        object(s, at, 1)
    }

    pub(super) fn skip_ws(s: &[u8], mut i: usize) -> usize {
        while matches!(s.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            i += 1;
        }
        i
    }

    fn object(s: &[u8], at: usize, depth: usize) -> Result<Object, Invalid> {
        if depth > MAX_DEPTH || s.get(at) != Some(&b'{') {
            return Err(Invalid);
        }
        let mut members = Vec::new();
        let mut i = skip_ws(s, at + 1);
        if s.get(i) == Some(&b'}') {
            return Ok(Object { open: at, close: i, members });
        }
        loop {
            let (key_end, key) = string(s, i)?;
            let key_span = i..key_end;
            i = skip_ws(s, key_end);
            if s.get(i) != Some(&b':') {
                return Err(Invalid);
            }
            let value_start = skip_ws(s, i + 1);
            let value_end = value(s, value_start, depth + 1)?;
            members.push(Member { key, key_span, value: value_start..value_end });
            i = skip_ws(s, value_end);
            match s.get(i) {
                Some(b',') => i = skip_ws(s, i + 1),
                Some(b'}') => return Ok(Object { open: at, close: i, members }),
                _ => return Err(Invalid),
            }
        }
    }

    fn array(s: &[u8], at: usize, depth: usize) -> Result<usize, Invalid> {
        if depth > MAX_DEPTH || s.get(at) != Some(&b'[') {
            return Err(Invalid);
        }
        let mut i = skip_ws(s, at + 1);
        if s.get(i) == Some(&b']') {
            return Ok(i + 1);
        }
        loop {
            i = skip_ws(s, value(s, i, depth + 1)?);
            match s.get(i) {
                Some(b',') => i = skip_ws(s, i + 1),
                Some(b']') => return Ok(i + 1),
                _ => return Err(Invalid),
            }
        }
    }

    /// Scans the value at `s[at]`; returns the index just past it.
    fn value(s: &[u8], at: usize, depth: usize) -> Result<usize, Invalid> {
        match s.get(at) {
            Some(b'{') => object(s, at, depth).map(|o| o.close + 1),
            Some(b'[') => array(s, at, depth),
            Some(b'"') => string(s, at).map(|(end, _)| end),
            Some(b't') => literal(s, at, b"true"),
            Some(b'f') => literal(s, at, b"false"),
            Some(b'n') => literal(s, at, b"null"),
            Some(b'-' | b'0'..=b'9') => number(s, at),
            _ => Err(Invalid),
        }
    }

    fn literal(s: &[u8], at: usize, word: &[u8]) -> Result<usize, Invalid> {
        let end = at + word.len();
        if s.get(at..end) == Some(word) { Ok(end) } else { Err(Invalid) }
    }

    fn digits(s: &[u8], mut i: usize) -> usize {
        while matches!(s.get(i), Some(b'0'..=b'9')) {
            i += 1;
        }
        i
    }

    /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`
    fn number(s: &[u8], at: usize) -> Result<usize, Invalid> {
        let mut i = at + usize::from(s.get(at) == Some(&b'-'));
        match s.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => i = digits(s, i + 1),
            _ => return Err(Invalid),
        }
        if s.get(i) == Some(&b'.') {
            let end = digits(s, i + 1);
            if end == i + 1 {
                return Err(Invalid);
            }
            i = end;
        }
        if matches!(s.get(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(s.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            let end = digits(s, i);
            if end == i {
                return Err(Invalid);
            }
            i = end;
        }
        Ok(i)
    }

    fn hex4(s: &[u8], at: usize) -> Result<u32, Invalid> {
        let quad = s.get(at..at + 4).ok_or(Invalid)?;
        quad.iter().try_fold(0u32, |acc, &b| Some(acc * 16 + char::from(b).to_digit(16)?)).ok_or(Invalid)
    }

    /// Scans and decodes the string literal at `s[at] == b'"'`; returns (index past the closing
    /// quote, decoded text). Handles every escape including `\uXXXX` surrogate pairs; rejects
    /// lone surrogates and raw control characters, as serde_json does.
    pub(super) fn string(s: &[u8], at: usize) -> Result<(usize, String), Invalid> {
        if s.get(at) != Some(&b'"') {
            return Err(Invalid);
        }
        let mut out = Vec::new();
        let mut i = at + 1;
        loop {
            match *s.get(i).ok_or(Invalid)? {
                b'"' => return String::from_utf8(out).map(|text| (i + 1, text)).map_err(|_| Invalid),
                b'\\' => {
                    let escape = *s.get(i + 1).ok_or(Invalid)?;
                    i += 2;
                    let byte = match escape {
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let unit = hex4(s, i)?;
                            i += 4;
                            let code = match unit {
                                0xD800..=0xDBFF => {
                                    if s.get(i..i + 2) != Some(b"\\u") {
                                        return Err(Invalid);
                                    }
                                    let low = hex4(s, i + 2)?;
                                    if !(0xDC00..=0xDFFF).contains(&low) {
                                        return Err(Invalid);
                                    }
                                    i += 6;
                                    0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
                                }
                                0xDC00..=0xDFFF => return Err(Invalid),
                                _ => unit,
                            };
                            let ch = char::from_u32(code).ok_or(Invalid)?;
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                            continue;
                        }
                        _ => return Err(Invalid),
                    };
                    out.push(byte);
                }
                b if b < 0x20 => return Err(Invalid),
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
    }
}
