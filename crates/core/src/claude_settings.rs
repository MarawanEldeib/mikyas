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
//!   files; handle an empty object `{}`).
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
//!   touched by Connect returns to its exact original bytes.
//!
//! `disconnect(connect(x).bytes) == x` must hold byte-for-byte for every input `x` that connect
//! accepts (property/round-trip tests with LF, CRLF, BOM, 2/4-space and tab indents, unicode and
//! escaped characters, `{}` and one-line files).
//!
//! Details the rules above leave open:
//! - "Already connected" means `wrap(original, shim, shell)` reproduces the current command
//!   exactly; anything else is re-wrapped.
//! - Empty input (missing file) connects like `{}\n`, so disconnecting that result yields `{}\n`
//!   (a valid settings file) rather than an empty file.
//! - Connect on an already-connected file cannot see the original literal any more; its record
//!   then carries the canonical encoding `serde_json::to_string(original)`. Callers should keep
//!   the record from the first connect when the original is unchanged.
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

use crate::cmdline::{self, CmdlineError, ShellKind, WrapMode};
use crate::time::Ms;

/// What Connect changed, persisted in `wrap.json` so Disconnect can undo it exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WrapRecord {
    /// Decoded original `statusLine.command`; `None` if there was no statusLine.
    pub original_command: Option<String>,
    /// Exact JSON literal of the original command as it appeared in the file (with quotes).
    pub original_command_raw: Option<String>,
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

/// Empty input (missing file) → `NotConfigured`.
pub fn status(bytes: &[u8]) -> Result<Status, SettingsError> {
    if bytes.is_empty() {
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

/// Returns the new file bytes and the record to persist in `wrap.json`. Empty input (missing
/// file) is treated as `{}` and produces a minimal file `{\n  "statusLine": …\n}\n`.
pub fn connect(
    bytes: &[u8],
    shim_path: &str,
    shell: ShellKind,
    now_ms: Ms,
) -> Result<(Vec<u8>, WrapRecord), SettingsError> {
    cmdline::validate_shim_path(shim_path)?;
    if bytes.is_empty() {
        return connect(b"{}\n", shim_path, shell, now_ms);
    }
    let doc = Doc::parse(bytes)?;
    let record = |original_command: Option<String>, original_command_raw: Option<String>, mode: WrapMode| WrapRecord {
        original_command,
        original_command_raw,
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
            Ok((out, record(None, None, wrapped.mode)))
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

/// `Ok(None)` when there is nothing of ours to undo.
pub fn disconnect(bytes: &[u8], wrap: Option<&WrapRecord>) -> Result<Option<Vec<u8>>, SettingsError> {
    if bytes.is_empty() {
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
                let out = doc.remove_member(index)?;
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
            // `{` W `}` → `{` NL unit MEMBER NL W `}`; removing the member and one line ending
            // restores W exactly.
            let unit = indent_unit(self.text, None);
            let at = self.top.open + 1;
            let member = pretty_member(literal, &unit, nl);
            return self.splice(at..at, &format!("{nl}{unit}{member}{nl}"));
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
    /// neighbours (the inverse of [`Doc::insert_status_line`]).
    fn remove_member(&self, index: usize) -> Result<Vec<u8>, SettingsError> {
        let members = &self.top.members;
        let member = members.get(index).ok_or(SettingsError::NotStrictJson)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    const SHIM: &str = "C:/Users/tester/AppData/Local/ClaudeUsageWidget/bin/cuw-capture.exe";
    const SHIM2: &str = "D:/Program Files/Claude Usage Widget/bin/cuw-capture.exe";
    const NOW: Ms = 1_790_208_000_000;
    const REALISTIC: &str = include_str!("../tests/fixtures/claude_settings/realistic.json");
    const USER_CMD: &str = "pwsh -NoProfile -ExecutionPolicy Bypass -File \"C:/Users/tester/.claude/statusline.ps1\"";
    const USER_LIT: &str =
        r#""pwsh -NoProfile -ExecutionPolicy Bypass -File \"C:/Users/tester/.claude/statusline.ps1\"""#;

    fn s(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    fn crlf(text: &str) -> String {
        text.replace('\n', "\r\n")
    }

    fn with_bom(text: &str) -> Vec<u8> {
        [BOM, text.as_bytes()].concat()
    }

    /// connect → checks → disconnect must give back `input` exactly.
    fn round_trip(input: &[u8], shell: ShellKind) -> (Vec<u8>, WrapRecord) {
        let (out, rec) = connect(input, SHIM, shell, NOW).unwrap();
        assert!(matches!(status(&out), Ok(Status::Connected { .. })), "{}", s(&out));
        assert_eq!(connect(&out, SHIM, shell, NOW).unwrap().0, out, "connect is idempotent");
        assert_eq!(disconnect(&out, Some(&rec)).unwrap().as_deref().map(s), Some(s(input)));
        (out, rec)
    }

    fn wrapped_lit(original: Option<&str>, shell: ShellKind) -> String {
        json_string(&cmdline::wrap(original, SHIM, shell).unwrap().command).unwrap()
    }

    #[test]
    fn status_of_missing_and_unconfigured_files() {
        assert_eq!(status(b""), Ok(Status::NotConfigured));
        assert_eq!(status(b"{}"), Ok(Status::NotConfigured));
        assert_eq!(status(REALISTIC.replace("statusLine", "statusLineOld").as_bytes()), Ok(Status::NotConfigured));
    }

    #[test]
    fn status_of_foreign_statuslines() {
        assert_eq!(status(REALISTIC.as_bytes()), Ok(Status::Foreign { command: Some(USER_CMD.into()) }));
        assert_eq!(
            status(br#"{"statusLine": {"type": "static", "text": "hi"}}"#),
            Ok(Status::Foreign { command: None })
        );
        assert_eq!(status(br#"{"statusLine": "echo hi"}"#), Ok(Status::Foreign { command: None }));
        assert_eq!(status(br#"{"statusLine": {"type": "command"}}"#), Ok(Status::Foreign { command: None }));
        assert_eq!(
            status(br#"{"statusLine": {"type": "command", "command": 5}}"#),
            Ok(Status::Foreign { command: None })
        );
    }

    #[test]
    fn status_of_each_connected_mode() {
        for (original, shell, mode) in [
            (Some("node sl.js"), ShellKind::Bash, WrapMode::Pipe),
            (Some("a && b"), ShellKind::Bash, WrapMode::PipeGrouped),
            (Some("node sl.js"), ShellKind::Pwsh, WrapMode::Pipe),
            (Some("node sl.js"), ShellKind::LegacyPowerShell, WrapMode::Argv),
            (None, ShellKind::Cmd, WrapMode::Default),
        ] {
            let file =
                format!("{{\"statusLine\": {{\"type\": \"command\", \"command\": {}}}}}", wrapped_lit(original, shell));
            assert_eq!(
                status(file.as_bytes()),
                Ok(Status::Connected { mode, shim_path: SHIM.into(), original: original.map(Into::into) }),
                "{file}"
            );
        }
    }

    #[test]
    fn status_errors() {
        assert_eq!(status(b"[1, 2]"), Err(SettingsError::NotAnObject));
        assert_eq!(status(b"\"text\""), Err(SettingsError::NotAnObject));
        assert_eq!(status(b"{ // c\n}"), Err(SettingsError::NotStrictJson));
        assert_eq!(status(b"{\"a\": 1,}"), Err(SettingsError::NotStrictJson));
        assert_eq!(status(b"   "), Err(SettingsError::NotStrictJson));
        assert_eq!(status(b"{\"a\": \"\xff\"}"), Err(SettingsError::NotStrictJson));
    }

    #[test]
    fn connect_realistic_changes_only_the_command_literal() {
        let (out, rec) = round_trip(REALISTIC.as_bytes(), ShellKind::Bash);
        let start = REALISTIC.find(USER_LIT).unwrap();
        let end = start + USER_LIT.len();
        let new_lit = wrapped_lit(Some(USER_CMD), ShellKind::Bash);
        assert_eq!(&out[..start], &REALISTIC.as_bytes()[..start], "prefix bytes");
        assert_eq!(&out[start + new_lit.len()..], &REALISTIC.as_bytes()[end..], "suffix bytes");
        assert_eq!(s(&out[start..start + new_lit.len()]), new_lit);
        assert_eq!(
            s(&out[start..start + new_lit.len()]),
            format!(
                r#""\"{SHIM}\" --tee | pwsh -NoProfile -ExecutionPolicy Bypass -File \"C:/Users/tester/.claude/statusline.ps1\"""#
            )
        );
        assert_eq!(
            rec,
            WrapRecord {
                original_command: Some(USER_CMD.into()),
                original_command_raw: Some(USER_LIT.into()),
                shim_path: SHIM.into(),
                mode: WrapMode::Pipe,
                shell: ShellKind::Bash,
                at_ms: NOW,
            }
        );
        // The hook's own "command" literal is untouched.
        assert!(s(&out).contains(r#""command": "pwsh -NoProfile -File \"C:/Users/tester/.claude/hooks/notify.ps1\"""#));
        // Without a record the canonical literal is identical here, so it also restores exactly.
        assert_eq!(disconnect(&out, None).unwrap().as_deref().map(s), Some(REALISTIC));
    }

    #[test]
    fn connect_realistic_in_every_shell_and_layout() {
        let variants: Vec<Vec<u8>> = vec![
            REALISTIC.as_bytes().to_vec(),
            REALISTIC.trim_end().as_bytes().to_vec(),
            crlf(REALISTIC).into_bytes(),
            with_bom(REALISTIC),
            with_bom(&crlf(REALISTIC.trim_end())),
        ];
        for input in &variants {
            for shell in [ShellKind::Bash, ShellKind::Cmd, ShellKind::Pwsh, ShellKind::LegacyPowerShell] {
                round_trip(input, shell);
            }
        }
    }

    #[test]
    fn connect_is_idempotent() {
        let (once, _) = connect(REALISTIC.as_bytes(), SHIM, ShellKind::Bash, NOW).unwrap();
        let (twice, rec) = connect(&once, SHIM, ShellKind::Bash, NOW + 1).unwrap();
        assert_eq!(twice, once);
        assert_eq!(rec.original_command.as_deref(), Some(USER_CMD));
        assert_eq!(rec.original_command_raw.as_deref(), Some(USER_LIT));
        let (fresh, _) = connect(b"{}", SHIM, ShellKind::Pwsh, NOW).unwrap();
        assert_eq!(connect(&fresh, SHIM, ShellKind::Pwsh, NOW).unwrap().0, fresh);
    }

    #[test]
    fn connect_with_new_shim_or_shell_rewraps() {
        let (first, rec1) = connect(REALISTIC.as_bytes(), SHIM, ShellKind::Bash, NOW).unwrap();
        let (moved, rec2) = connect(&first, SHIM2, ShellKind::Bash, NOW).unwrap();
        assert_ne!(moved, first);
        assert_eq!(
            status(&moved),
            Ok(Status::Connected { mode: WrapMode::Pipe, shim_path: SHIM2.into(), original: Some(USER_CMD.into()) })
        );
        assert_eq!(rec2.shim_path, SHIM2);
        assert_eq!(rec2.original_command.as_deref(), Some(USER_CMD));
        // Disconnect with the FIRST record restores the original bytes.
        assert_eq!(disconnect(&moved, Some(&rec1)).unwrap().as_deref().map(s), Some(REALISTIC));

        let (legacy, _) = connect(&first, SHIM, ShellKind::LegacyPowerShell, NOW).unwrap();
        assert_eq!(
            status(&legacy),
            Ok(Status::Connected { mode: WrapMode::Argv, shim_path: SHIM.into(), original: Some(USER_CMD.into()) })
        );
        assert_eq!(disconnect(&legacy, Some(&rec1)).unwrap().as_deref().map(s), Some(REALISTIC));
    }

    #[test]
    fn connect_inserts_into_files_without_statusline() {
        let lit = wrapped_lit(None, ShellKind::Bash);
        let pretty = |unit: &str, nl: &str| {
            format!(
                "\"statusLine\": {{{nl}{unit}{unit}\"type\": \"command\",{nl}{unit}{unit}\"command\": {lit}{nl}{unit}}}"
            )
        };
        let cases: Vec<(Vec<u8>, String)> = vec![
            (b"{}".to_vec(), format!("{{\n  {}\n}}", pretty("  ", "\n"))),
            (b"{}\n".to_vec(), format!("{{\n  {}\n}}\n", pretty("  ", "\n"))),
            (b"{ }".to_vec(), format!("{{\n  {}\n }}", pretty("  ", "\n"))),
            (b"{}\r\n".to_vec(), format!("{{\r\n  {}\r\n}}\r\n", pretty("  ", "\r\n"))),
            (b"{\"a\":1}".to_vec(), format!("{{\"a\":1,\"statusLine\":{{\"type\":\"command\",\"command\":{lit}}}}}")),
            (
                b"{\"a\": 1, \"b\": [1, 2]}\n".to_vec(),
                format!("{{\"a\": 1, \"b\": [1, 2], \"statusLine\": {{\"type\": \"command\", \"command\": {lit}}}}}\n"),
            ),
            (
                b"{\n  \"a\": 1,\n  \"b\": {\n    \"c\": true\n  }\n}\n".to_vec(),
                format!("{{\n  \"a\": 1,\n  \"b\": {{\n    \"c\": true\n  }},\n  {}\n}}\n", pretty("  ", "\n")),
            ),
            (b"{\n    \"a\": 1\n}".to_vec(), format!("{{\n    \"a\": 1,\n    {}\n}}", pretty("    ", "\n"))),
            (b"{\n\t\"a\": 1\n}\n".to_vec(), format!("{{\n\t\"a\": 1,\n\t{}\n}}\n", pretty("\t", "\n"))),
            (
                b"{\r\n  \"a\": 1\r\n}\r\n".to_vec(),
                format!("{{\r\n  \"a\": 1,\r\n  {}\r\n}}\r\n", pretty("  ", "\r\n")),
            ),
            (
                with_bom("{\n  \"a\": \"\u{e9}\"\n}\n"),
                format!("\u{feff}{{\n  \"a\": \"\u{e9}\",\n  {}\n}}\n", pretty("  ", "\n")),
            ),
        ];
        for (input, expected) in cases {
            let (out, rec) = round_trip(&input, ShellKind::Bash);
            assert_eq!(s(&out), expected, "input {:?}", s(&input));
            assert_eq!((rec.original_command, rec.original_command_raw, rec.mode), (None, None, WrapMode::Default));
        }
    }

    #[test]
    fn connect_missing_file_creates_minimal_settings() {
        let (out, rec) = connect(b"", SHIM, ShellKind::Bash, NOW).unwrap();
        let lit = wrapped_lit(None, ShellKind::Bash);
        assert_eq!(
            s(&out),
            format!("{{\n  \"statusLine\": {{\n    \"type\": \"command\",\n    \"command\": {lit}\n  }}\n}}\n")
        );
        // Disconnecting leaves a valid, empty settings file rather than a zero-byte one.
        assert_eq!(disconnect(&out, Some(&rec)).unwrap().as_deref().map(s), Some("{}\n"));
        assert_eq!(disconnect(b"", Some(&rec)), Ok(None));
    }

    #[test]
    fn statusline_extra_keys_are_preserved() {
        let input = format!(
            "{{\n  \"statusLine\": {{\n    \"padding\": 0,\n    \"type\": \"command\",\n    \"command\": {USER_LIT},\n    \"refreshInterval\": 5\n  }}\n}}\n"
        );
        let (out, _) = round_trip(input.as_bytes(), ShellKind::Bash);
        let lit = wrapped_lit(Some(USER_CMD), ShellKind::Bash);
        assert_eq!(s(&out), input.replace(USER_LIT, &lit));
    }

    #[test]
    fn blank_command_becomes_default_and_restores() {
        for raw in ["\"\"", "\"  \"", "\"\\t\""] {
            let input = format!("{{\"statusLine\": {{\"type\": \"command\", \"command\": {raw}, \"padding\": 1}}}}");
            let (out, rec) = round_trip(input.as_bytes(), ShellKind::Bash);
            assert_eq!(rec.mode, WrapMode::Default);
            // Without the record the member is not removed, because it has keys we never write.
            let restored = disconnect(&out, None).unwrap().unwrap();
            assert_eq!(s(&restored), input.replace(raw, "\"\""));
        }
    }

    #[test]
    fn unsupported_statuslines() {
        for input in [
            r#"{"statusLine": {"type": "static", "command": "x"}}"#,
            r#"{"statusLine": {"command": "x"}}"#,
            r#"{"statusLine": {"type": "command"}}"#,
            r#"{"statusLine": {"type": "command", "command": null}}"#,
            r#"{"statusLine": "node sl.js"}"#,
            r#"{"statusLine": null}"#,
        ] {
            assert_eq!(
                connect(input.as_bytes(), SHIM, ShellKind::Bash, NOW),
                Err(SettingsError::UnsupportedStatusLine),
                "{input}"
            );
            assert_eq!(disconnect(input.as_bytes(), None), Ok(None));
        }
    }

    #[test]
    fn invalid_documents_are_rejected() {
        let connect_err = |b: &[u8]| connect(b, SHIM, ShellKind::Bash, NOW).unwrap_err();
        assert_eq!(connect_err(b"[]"), SettingsError::NotAnObject);
        assert_eq!(connect_err(b"[{\"statusLine\": 1}]\n"), SettingsError::NotAnObject);
        assert_eq!(connect_err(b"null"), SettingsError::NotAnObject);
        for jsonc in [
            "{\n  // comment\n  \"a\": 1\n}",
            "{\n  \"a\": 1,\n}",
            "{\"a\": [1, 2,]}",
            "/* c */ {}",
            "{'a': 1}",
            "{\"a\": 1} trailing",
            "{\"a\": NaN}",
            " ",
            "{\"a\": \"\\ud800\"}",
        ] {
            assert_eq!(connect_err(jsonc.as_bytes()), SettingsError::NotStrictJson, "{jsonc:?}");
            assert_eq!(disconnect(jsonc.as_bytes(), None), Err(SettingsError::NotStrictJson), "{jsonc:?}");
        }
        // Duplicate statusLine keys make the edit target ambiguous.
        let dup =
            r#"{"statusLine": {"type": "command", "command": "a"}, "statusLine": {"type": "command", "command": "b"}}"#;
        assert_eq!(connect_err(dup.as_bytes()), SettingsError::NotStrictJson);
        let dup_cmd = r#"{"statusLine": {"type": "command", "command": "a", "command": "b"}}"#;
        assert_eq!(connect_err(dup_cmd.as_bytes()), SettingsError::NotStrictJson);
    }

    #[test]
    fn cmdline_errors_leave_the_file_alone() {
        let input = r#"{"statusLine": {"type": "command", "command": "a; b"}}"#;
        assert_eq!(
            connect(input.as_bytes(), SHIM, ShellKind::LegacyPowerShell, NOW),
            Err(SettingsError::Cmdline(CmdlineError::Unsupported))
        );
        assert!(matches!(
            connect(input.as_bytes(), "C:/x&y/cuw-capture.exe", ShellKind::Bash, NOW),
            Err(SettingsError::Cmdline(CmdlineError::UnsafeShimPath(_)))
        ));
        let review = r#"{"statusLine": {"type": "command", "command": "a & (b)"}}"#;
        assert_eq!(
            connect(review.as_bytes(), SHIM, ShellKind::Cmd, NOW),
            Err(SettingsError::Cmdline(CmdlineError::NeedsReview))
        );
    }

    #[test]
    fn escaped_keys_are_recognised() {
        let input = r#"{"statusLin\u0065": {"typ\u0065": "command", "comm\u0061nd": "node sl.js"}}"#;
        assert_eq!(status(input.as_bytes()), Ok(Status::Foreign { command: Some("node sl.js".into()) }));
        let (out, _) = round_trip(input.as_bytes(), ShellKind::Bash);
        assert!(s(&out).starts_with(r#"{"statusLin\u0065": {"typ\u0065": "command", "comm\u0061nd": ""#));
    }

    #[test]
    fn unicode_and_escaped_originals_restore_exactly() {
        for lit in [
            r#""echo \u00e9t\u00e9 \ud83d\ude00 \/ \t done""#,
            "\"echo \u{e9}t\u{e9} \u{1F600} caf\u{e9}\"",
            r#""node \"C:\\Users\\tester\\sl.js\"""#,
            r#""printf '\u0041\u00df'""#,
        ] {
            let input =
                format!("{{\n  \"statusLine\": {{\n    \"type\": \"command\",\n    \"command\": {lit}\n  }}\n}}\n");
            let (out, rec) = round_trip(input.as_bytes(), ShellKind::Bash);
            assert_eq!(rec.original_command_raw.as_deref(), Some(lit));
            let decoded: String = serde_json::from_str(lit).unwrap();
            assert_eq!(rec.original_command.as_deref(), Some(decoded.as_str()));
            // Without a record the decoded text is restored in canonical form.
            let canonical = disconnect(&out, None).unwrap().unwrap();
            assert_eq!(s(&canonical), input.replace(lit, &json_string(&decoded).unwrap()));
        }
    }

    #[test]
    fn disconnect_keeps_a_user_edited_tail() {
        let (out, rec) = connect(REALISTIC.as_bytes(), SHIM, ShellKind::Bash, NOW).unwrap();
        let edited_cmd = format!("\"{SHIM}\" --tee | node \"C:/Users/tester/new-statusline.js\"");
        let edited = s(&out).replace(&wrapped_lit(Some(USER_CMD), ShellKind::Bash), &json_string(&edited_cmd).unwrap());
        let restored = disconnect(edited.as_bytes(), Some(&rec)).unwrap().unwrap();
        assert_eq!(s(&restored), REALISTIC.replace(USER_LIT, r#""node \"C:/Users/tester/new-statusline.js\"""#));
    }

    #[test]
    fn disconnect_ignores_foreign_and_missing_statuslines() {
        assert_eq!(disconnect(REALISTIC.as_bytes(), None), Ok(None));
        assert_eq!(disconnect(b"{}", None), Ok(None));
        assert_eq!(disconnect(b"", None), Ok(None));
        let foreign = format!(
            "{{\"statusLine\": {{\"type\": \"command\", \"command\": {}}}}}",
            json_string("\"C:/x/other.exe\" --tee | a").unwrap()
        );
        assert_eq!(disconnect(foreign.as_bytes(), None), Ok(None));
    }

    #[test]
    fn disconnect_removes_an_inserted_statusline_wherever_it_moved() {
        let lit = wrapped_lit(None, ShellKind::Bash);
        let member = format!("\"statusLine\": {{\n    \"type\": \"command\",\n    \"command\": {lit}\n  }}");
        for (connected, expected) in [
            (
                format!("{{\n  \"a\": 1,\n  {member},\n  \"model\": \"x\"\n}}\n"),
                "{\n  \"a\": 1,\n  \"model\": \"x\"\n}\n",
            ),
            (format!("{{\n  {member},\n  \"model\": \"x\"\n}}\n"), "{\n  \"model\": \"x\"\n}\n"),
            (format!("{{\n  \"a\": 1,\n  {member}\n}}\n"), "{\n  \"a\": 1\n}\n"),
            (format!("{{\n  {member}\n}}\n"), "{}\n"),
            (format!("{{{member}}}"), "{}"),
        ] {
            assert_eq!(
                disconnect(connected.as_bytes(), None).unwrap().as_deref().map(s),
                Some(expected),
                "{connected}"
            );
        }
    }

    #[test]
    fn stale_records_are_not_trusted() {
        let (out, mut rec) = connect(REALISTIC.as_bytes(), SHIM, ShellKind::Bash, NOW).unwrap();
        // A raw literal that does not decode to the recorded original is ignored.
        rec.original_command_raw = Some("\"something else\"".into());
        assert_eq!(disconnect(&out, Some(&rec)).unwrap().as_deref().map(s), Some(REALISTIC));
        rec.original_command_raw = Some(format!("{USER_LIT} "));
        assert_eq!(disconnect(&out, Some(&rec)).unwrap().as_deref().map(s), Some(REALISTIC));
        rec.original_command_raw = Some("{\"injected\": 1}".into());
        assert_eq!(disconnect(&out, Some(&rec)).unwrap().as_deref().map(s), Some(REALISTIC));
    }

    #[test]
    fn scanner_decodes_escapes_and_rejects_malformed_strings() {
        let dec = |raw: &str| decode_literal(raw).ok();
        assert_eq!(dec(r#""a\"b\\c\/d\b\f\n\r\t""#).as_deref(), Some("a\"b\\c/d\u{8}\u{c}\n\r\t"));
        assert_eq!(dec(r#""\u00e9\u20AC\ud83d\ude00""#).as_deref(), Some("é€😀"));
        assert_eq!(dec("\"é😀\"").as_deref(), Some("é😀"));
        for bad in [
            r#""\ud83d""#,
            r#""\ude00""#,
            r#""\ud83d\u0041""#,
            r#""\x""#,
            r#""\u12""#,
            r#""\u12G4""#,
            "\"a\nb\"",
            "\"open",
            "\"a\" ",
            "a",
            "",
        ] {
            assert_eq!(dec(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn scanner_numbers_and_literals() {
        for ok in ["{\"a\": -0.5e+10}", "{\"a\": 0}", "{\"a\": 12E3, \"b\": [true, false, null, -1.25]}"] {
            assert!(json::document(ok.as_bytes()).is_ok(), "{ok}");
        }
        for bad in [
            "{\"a\": 01}",
            "{\"a\": 1.}",
            "{\"a\": -}",
            "{\"a\": 1e}",
            "{\"a\": tru}",
            "{\"a\": .5}",
            "{\"a\" 1}",
            "{\"a\": 1 \"b\": 2}",
        ] {
            assert!(json::document(bad.as_bytes()).is_err(), "{bad}");
        }
        let deep = format!("{{\"a\": {}{}}}", "[".repeat(300), "]".repeat(300));
        assert!(json::document(deep.as_bytes()).is_err());
    }

    // ---- property tests ----

    fn arb_text() -> impl Strategy<Value = String> {
        prop_oneof!["[a-z ]{0,12}", "\\PC{0,16}", Just("é😀\"\\/\t\n\u{1}".to_string()),]
    }

    fn arb_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            (-1e6f64..1e6).prop_map(Value::from),
            arb_text().prop_map(Value::String),
        ];
        leaf.prop_recursive(3, 16, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::vec(("[a-zA-Z_]{1,6}", inner), 0..4)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        })
    }

    /// `": "` / `", "` separators on one line.
    struct Spaced;
    impl serde_json::ser::Formatter for Spaced {
        fn begin_object_key<W: ?Sized + std::io::Write>(&mut self, w: &mut W, first: bool) -> std::io::Result<()> {
            if first { Ok(()) } else { w.write_all(b", ") }
        }
        fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
            w.write_all(b": ")
        }
        fn begin_array_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W, first: bool) -> std::io::Result<()> {
            if first { Ok(()) } else { w.write_all(b", ") }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Layout {
        Pretty(&'static str),
        Compact,
        Spaced,
    }

    fn render(value: &Value, layout: Layout) -> String {
        let mut buf = Vec::new();
        match layout {
            Layout::Pretty(unit) => {
                let fmt = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
                value.serialize(&mut serde_json::Serializer::with_formatter(&mut buf, fmt)).unwrap();
            }
            Layout::Compact => value.serialize(&mut serde_json::Serializer::new(&mut buf)).unwrap(),
            Layout::Spaced => value.serialize(&mut serde_json::Serializer::with_formatter(&mut buf, Spaced)).unwrap(),
        }
        String::from_utf8(buf).unwrap()
    }

    /// Escapes every non-ASCII char as `\uXXXX` (surrogate pairs above the BMP).
    fn ascii_literal(text: &str) -> String {
        let mut out = String::new();
        for c in json_string(text).unwrap().chars() {
            if c.is_ascii() {
                out.push(c);
            } else {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
        out
    }

    const PLACEHOLDER: &str = "__cuw_command_placeholder__";

    fn arb_settings() -> impl Strategy<Value = (Vec<u8>, bool)> {
        let layout = prop_oneof![
            Just(Layout::Pretty("  ")),
            Just(Layout::Pretty("    ")),
            Just(Layout::Pretty("\t")),
            Just(Layout::Compact),
            Just(Layout::Spaced),
        ];
        let command = prop_oneof![
            Just(None),
            Just(Some(USER_CMD.to_string())),
            "[a-z ./\"'-]{0,20}".prop_map(Some),
            arb_text().prop_map(Some),
        ];
        (
            prop::collection::vec(("[a-z]{1,6}", arb_value()), 0..5),
            layout,
            command,
            any::<(bool, bool, bool, bool, bool)>(),
            0usize..8,
        )
            .prop_map(|(members, layout, command, (crlf_eol, bom, trailing_nl, ascii, padding), pos)| {
                // Dedupe keys first, then place statusLine (if any) at `pos`.
                let mut members: Vec<(String, Value)> =
                    members.into_iter().collect::<Map<_, _>>().into_iter().collect();
                if command.is_some() {
                    let mut sl = Map::new();
                    if padding {
                        sl.insert("padding".into(), Value::from(0));
                    }
                    sl.insert("type".into(), Value::from("command"));
                    sl.insert("command".into(), Value::from(PLACEHOLDER));
                    members.insert(pos.min(members.len()), (STATUS_LINE.into(), Value::Object(sl)));
                }
                let canonical = !ascii;
                let mut text = render(&Value::Object(members.into_iter().collect()), layout);
                if let Some(cmd) = &command {
                    let lit = if ascii { ascii_literal(cmd) } else { json_string(cmd).unwrap() };
                    text = text.replace(&format!("\"{PLACEHOLDER}\""), &lit);
                }
                if trailing_nl {
                    text.push('\n');
                }
                if crlf_eol {
                    text = crlf(&text);
                }
                let bytes = if bom { with_bom(&text) } else { text.into_bytes() };
                (bytes, canonical || command.as_deref().is_none_or(str::is_ascii))
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 600, ..ProptestConfig::default() })]

        #[test]
        fn connect_disconnect_round_trip((input, canonical) in arb_settings(), shell in prop_oneof![
            Just(ShellKind::Bash), Just(ShellKind::Cmd), Just(ShellKind::Pwsh), Just(ShellKind::LegacyPowerShell)
        ]) {
            match connect(&input, SHIM, shell, NOW) {
                Ok((out, rec)) => {
                    let connected = matches!(status(&out), Ok(Status::Connected { .. }));
                    prop_assert!(connected);
                    prop_assert_eq!(&connect(&out, SHIM, shell, NOW).unwrap().0, &out);
                    prop_assert_eq!(disconnect(&out, Some(&rec)).unwrap(), Some(input.clone()));
                    let blank = rec.original_command.as_deref().is_some_and(|o| o.trim().is_empty());
                    if canonical && !blank {
                        prop_assert_eq!(disconnect(&out, None).unwrap(), Some(input.clone()));
                    }
                    // Moving the shim and disconnecting with the first record also round-trips.
                    let (moved, _) = connect(&out, SHIM2, shell, NOW).unwrap();
                    prop_assert_eq!(disconnect(&moved, Some(&rec)).unwrap(), Some(input));
                }
                Err(e) => prop_assert!(
                    matches!(e, SettingsError::Cmdline(CmdlineError::NeedsReview | CmdlineError::Unsupported)),
                    "{e:?}"
                ),
            }
        }

        #[test]
        fn scanner_agrees_with_serde(value in arb_value(), layout in prop_oneof![
            Just(Layout::Pretty("  ")), Just(Layout::Compact), Just(Layout::Spaced)
        ]) {
            let Value::Object(map) = value else { return Ok(()); };
            let text = render(&Value::Object(map), layout);
            // Compare against serde's own reading of the text (float parsing is not exact).
            let map: Map<String, Value> = serde_json::from_str(&text).unwrap();
            let obj = json::document(text.as_bytes()).unwrap();
            prop_assert_eq!(obj.members.len(), map.len());
            for (member, (key, v)) in obj.members.iter().zip(map.iter()) {
                prop_assert_eq!(&member.key, key);
                let parsed: Value = serde_json::from_str(&text[member.value.clone()]).unwrap();
                prop_assert_eq!(&parsed, v);
            }
        }

        #[test]
        fn string_scanner_matches_serde(text in "\\PC{0,24}", ascii in any::<bool>()) {
            let lit = if ascii { ascii_literal(&text) } else { json_string(&text).unwrap() };
            prop_assert_eq!(decode_literal(&lit).unwrap(), text);
        }

        #[test]
        fn never_panics_on_garbage(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
            let _ = status(&bytes);
            let _ = connect(&bytes, SHIM, ShellKind::Bash, NOW);
            let _ = disconnect(&bytes, None);
            let _ = json::document(&bytes);
        }
    }
}
