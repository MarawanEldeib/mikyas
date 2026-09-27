//! Building and recognising the statusline command that routes Claude Code's statusline JSON
//! through `sovawatch-capture.exe` without changing what the user's own statusline prints.
//!
//! Forms (the shim path is always double-quoted and uses forward slashes):
//! - Bash / Cmd, simple original:   `"<shim>" --tee | <original>`            (WrapMode::Pipe)
//! - Bash / Cmd, compound original: `"<shim>" --tee | (<original>)`          (WrapMode::PipeGrouped)
//!   compound = an UNQUOTED `&&`, `||`, `;`, `&` or newline. Quote rules differ per shell: bash
//!   honours '…' and "…" (with `\` escapes outside single quotes); cmd honours only "…" (a `'` is a
//!   literal character, and `^` escapes the next char). For Cmd, an original containing an
//!   unquoted `(` or `)` cannot be grouped safely → `CmdlineError::NeedsReview`.
//! - Pwsh (PowerShell ≥ 7.4, byte-preserving native pipes), simple original:
//!   `& "<shim>" --tee | <original>`                                        (WrapMode::Pipe)
//!   A compound original under Pwsh (unquoted `;`, `&&`, `||`, newline) → use Argv.
//! - LegacyPowerShell (< 7.4 re-encodes native-to-native pipes and would corrupt non-ASCII JSON),
//!   or compound under Pwsh: `& "<shim>" -- <original>`                     (WrapMode::Argv)
//!   The shell tokenises `<original>` and the shim spawns those tokens itself. Compound originals
//!   cannot be expressed this way → `CmdlineError::Unsupported`.
//! - No original statusline: `"<shim>" --default` (Bash/Cmd) or `& "<shim>" --default`
//!   (PowerShell kinds)                                                      (WrapMode::Default)
//!
//! [`unwrap`] must recognise every form produced by [`wrap`] (for any shim path that passes
//! [`validate_shim_path`]) and return the original text EXACTLY (for PipeGrouped, the text
//! between the outer parentheses). `unwrap(wrap(x).command).original == x` is a property test.
//!
//! Implementation notes (where the forms above need care to stay correct):
//! - Because Argv cannot express a compound original either, a compound original under Pwsh
//!   is `Unsupported`, exactly like under LegacyPowerShell.
//! - `wrap` only groups when the parenthesised text is guaranteed to parse the same way. Bash
//!   originals that start with `(` (would form the arithmetic `((`), end inside a comment or an
//!   escape, leave a quote open or contain a heredoc → `NeedsReview`; so do compound Cmd
//!   originals that end in a dangling `&` / `||`.
//! - cmd.exe re-parses each side of a pipe in a child `cmd` after re-serialising it: unquoted `^`
//!   escapes would be consumed twice (`echo x^&y` would run `y`), redirections move to the end
//!   (into an open quote), paren blocks and `rem` / `::` comments change meaning, and `cmd /c`
//!   stops at a line break. So any Cmd original with an unquoted `^`, paren, `rem` / `::` word,
//!   an open quote or a line break → `NeedsReview`, in both Pipe forms. (That child also
//!   appends a space to the output of built-ins like `echo`; external programs are unaffected.)
//! - A simple Bash/Cmd original that starts with `(` and ends with `)` would make the Pipe form
//!   indistinguishable from PipeGrouped, so it is `NeedsReview` as well. So is a simple Bash
//!   original that starts with a comment or with `!` / `time` (only valid at a pipeline's start).
//! - PowerShell only runs a pipeline element as a command when it starts like one. Originals that
//!   are only comments or start with an expression (`$x`, `(…)`, `"…"`, `5`, `>…`) →
//!   `NeedsReview`. Under LegacyPowerShell the original becomes the shim's arguments, so a
//!   leading `&` / dot-source or a line break before the command → `NeedsReview` too.
//! - For the PowerShell kinds an unquoted `&` that is not the call operator (PowerShell 7's
//!   background operator, which ends the statement) also counts as compound; a line break right
//!   after `|` continues the pipeline and does not.
//! - Both PowerShell forms only work when the original's first command is a native program:
//!   after `| ` a cmdlet, script block or `.ps1` script gets the JSON as pipeline objects (a
//!   script reading `[Console]::In` sees nothing), and Argv can only spawn programs. Statement
//!   keywords (`if`, `try`, …), `.ps1` scripts, script blocks, built-in aliases and
//!   `<approved verb>-<noun>` cmdlet names → `NeedsReview` (`curl` / `wget` only under
//!   LegacyPowerShell: they are aliases in 5.1 but native programs in pwsh 7). So is a
//!   `-name:value` token under LegacyPowerShell: after `--` PowerShell passes it to the shim as
//!   two arguments.
//! - cmd.exe also mis-serialises an `if` command on the right of a pipe (`if exist x y` fails
//!   with "y was unexpected at this time"), so a Cmd original with an `if` command →
//!   `NeedsReview`.

use serde::{Deserialize, Serialize};

/// File name of the capture shim executable.
pub const SHIM_EXE_NAME: &str = "sovawatch-capture.exe";
/// The shim's file name under the app's former name (Claude Usage Widget). Commands that run it
/// are still recognised ([`unwrap`]) so status, Disconnect and the move to SovaWatch work on
/// them, but [`wrap`] never writes it.
pub const LEGACY_SHIM_EXE_NAME: &str = "cuw-capture.exe";
/// Every shim file name [`unwrap`] recognises.
pub const RECOGNISED_SHIM_EXE_NAMES: &[&str] = &[SHIM_EXE_NAME, LEGACY_SHIM_EXE_NAME];

/// Whether a command-form shim path names the legacy shim ([`LEGACY_SHIM_EXE_NAME`]).
pub fn is_legacy_shim(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|name| name.eq_ignore_ascii_case(LEGACY_SHIM_EXE_NAME))
}

/// The shell Claude Code uses to run the statusline command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Bash,
    Cmd,
    /// PowerShell 7.4+.
    Pwsh,
    /// Windows PowerShell 5.1 or PowerShell < 7.4.
    LegacyPowerShell,
}

/// How the shim is spliced into the statusline command (see the module docs for each form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapMode {
    Pipe,
    PipeGrouped,
    Argv,
    Default,
}

/// Result of [`wrap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub command: String,
    pub mode: WrapMode,
}

/// Result of [`unwrap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwrapped {
    /// As written in the command (forward slashes).
    pub shim_path: String,
    pub mode: WrapMode,
    /// The user's original command; `None` for WrapMode::Default.
    pub original: Option<String>,
}

/// Why a statusline command could not be wrapped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CmdlineError {
    #[error("shim path contains characters that are unsafe in a shell command: {0:?}")]
    UnsafeShimPath(String),
    #[error("the existing statusline command needs manual review before wrapping")]
    NeedsReview,
    #[error("this shell cannot wrap a compound statusline command")]
    Unsupported,
    #[error("the existing statusline command is empty")]
    EmptyOriginal,
}

/// Converts a filesystem path to the form used in commands (forward slashes).
///
/// A verbatim prefix (`\\?\C:\…`, as returned by `std::fs::canonicalize` on Windows, or
/// `\\?\UNC\server\share\…`) is dropped first: cmd.exe cannot run `"//?/C:/…"`.
pub fn shim_path_for_command(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    let plain = match text.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    };
    plain.replace('\\', "/")
}

/// Characters that are special inside a double-quoted string (or that end one) in at least one
/// supported shell. `\` is included because command paths always use forward slashes and bash
/// collapses `\\` inside double quotes; the Unicode quotes are string delimiters in PowerShell.
/// `?` and `*` never occur in a Windows file name, only in a verbatim `//?/` prefix, which
/// cmd.exe cannot run.
const FORBIDDEN_SHIM_CHARS: &[char] = &[
    '%', '^', '!', '&', '|', '<', '>', '$', '`', '"', '\'', '(', ')', ';', '\\', '?', '*', '\u{2018}', '\u{2019}',
    '\u{201A}', '\u{201B}', '\u{201C}', '\u{201D}', '\u{201E}',
];

/// Rejects paths containing any of `% ^ ! & | < > $ \` " ' ( ) ;` or control characters, and
/// paths not ending in [`SHIM_EXE_NAME`] (case-insensitive).
///
/// Also rejects backslashes (command paths use forward slashes, see [`shim_path_for_command`]),
/// `?` / `*` (a verbatim `//?/` path) and PowerShell's typographic quotes, and requires
/// [`SHIM_EXE_NAME`] to be the whole final path component.
pub fn validate_shim_path(path: &str) -> Result<(), CmdlineError> {
    validate_shim_path_named(path, &[SHIM_EXE_NAME])
}

/// [`validate_shim_path`] with any of `names` as the final path component.
fn validate_shim_path_named(path: &str, names: &[&str]) -> Result<(), CmdlineError> {
    let unsafe_path = || CmdlineError::UnsafeShimPath(path.to_owned());
    if path.chars().any(|c| c.is_control() || FORBIDDEN_SHIM_CHARS.contains(&c)) {
        return Err(unsafe_path());
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if !names.iter().any(|name| file_name.eq_ignore_ascii_case(name)) {
        return Err(unsafe_path());
    }
    Ok(())
}

/// True if `cmd` contains an unquoted command separator for `shell` (see module docs).
///
/// A newline only counts when it separates two non-blank parts (a trailing newline is not a
/// second command); for Cmd the same holds for `&`, `&&` and `||`. Redirections such as `2>&1`,
/// `&>file` and bash's `|&` are not separators. For the PowerShell kinds an unquoted `&` that is
/// not the call operator (the background operator) is one, and a line break right after `|` is
/// not.
pub fn is_compound(cmd: &str, shell: ShellKind) -> bool {
    scan(cmd, shell).compound
}

/// True if `cmd` contains an unquoted `(` or `)` for `shell`.
pub fn has_unquoted_parens(cmd: &str, shell: ShellKind) -> bool {
    scan(cmd, shell).parens
}

/// Builds the wrapped command. `original = None` (or only whitespace → `EmptyOriginal` is NOT
/// raised; whitespace-only is treated as None) produces the Default form. If `original` is
/// already wrapped (recognised by [`unwrap`]), re-wraps its original with the new shim/shell.
pub fn wrap(original: Option<&str>, shim_path: &str, shell: ShellKind) -> Result<Wrapped, CmdlineError> {
    validate_shim_path(shim_path)?;
    let mut original = original.map(str::to_owned);
    // Each unwrap strictly shortens the text, so this terminates.
    while let Some(inner) = original.as_deref().and_then(unwrap) {
        original = inner.original;
    }
    let powershell = matches!(shell, ShellKind::Pwsh | ShellKind::LegacyPowerShell);
    let head = if powershell { format!("& \"{shim_path}\"") } else { format!("\"{shim_path}\"") };
    let Some(original) = original.filter(|o| !o.trim().is_empty()) else {
        return Ok(Wrapped { command: format!("{head} --default"), mode: WrapMode::Default });
    };

    let s = scan(&original, shell);
    let (command, mode) = match shell {
        ShellKind::Bash | ShellKind::Cmd => {
            // cmd re-parses each side of a pipe in a child cmd.exe that re-serialises it first:
            // unquoted `^` escapes are consumed twice (`echo x^&y` would run `y`), redirections
            // move to the end (into an open quote), and paren blocks change meaning.
            // `cmd /c` also stops reading at a line break, and `rem` / `::` misbehave in a pipe.
            let cmd_unsafe = s.escapes
                || s.parens
                || s.unterminated
                || s.trailing_comment
                || s.if_command
                || original.contains(['\n', '\r']);
            if shell == ShellKind::Cmd && cmd_unsafe {
                return Err(CmdlineError::NeedsReview);
            }
            if s.compound {
                if !s.groupable(shell) || original.starts_with('(') {
                    return Err(CmdlineError::NeedsReview);
                }
                (format!("{head} --tee | ({original})"), WrapMode::PipeGrouped)
            } else {
                if looks_grouped(&original) || (shell == ShellKind::Bash && !bash_pipe_element(&original)) {
                    return Err(CmdlineError::NeedsReview);
                }
                (format!("{head} --tee | {original}"), WrapMode::Pipe)
            }
        }
        ShellKind::Pwsh => {
            if s.compound {
                return Err(CmdlineError::Unsupported);
            }
            if !pwsh_starts_with_command(&original, false) {
                return Err(CmdlineError::NeedsReview);
            }
            (format!("{head} --tee | {original}"), WrapMode::Pipe)
        }
        ShellKind::LegacyPowerShell => {
            if s.compound {
                return Err(CmdlineError::Unsupported);
            }
            if !pwsh_starts_with_command(&original, true) || s.colon_parameter {
                return Err(CmdlineError::NeedsReview);
            }
            (format!("{head} -- {original}"), WrapMode::Argv)
        }
    };
    Ok(Wrapped { command, mode })
}

/// Recognises a command produced by [`wrap`], by this app or under its former name (any of
/// [`RECOGNISED_SHIM_EXE_NAMES`]). Returns `None` for anything else.
pub fn unwrap(command: &str) -> Option<Unwrapped> {
    unwrap_with(command, RECOGNISED_SHIM_EXE_NAMES)
}

/// [`unwrap`] for shims named any of `names` only.
pub fn unwrap_with(command: &str, names: &[&str]) -> Option<Unwrapped> {
    let (powershell, rest) = match command.strip_prefix("& ") {
        Some(rest) => (true, rest),
        None => (false, command),
    };
    let rest = rest.strip_prefix('"')?;
    let close = rest.find('"')?;
    let shim_path = &rest[..close];
    validate_shim_path_named(shim_path, names).ok()?;
    let rest = &rest[close + 1..];

    let (mode, original) = if rest == " --default" {
        (WrapMode::Default, None)
    } else if let Some(tail) = rest.strip_prefix(" --tee | ") {
        match tail.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
            Some(inner) if !powershell => (WrapMode::PipeGrouped, Some(inner)),
            _ => (WrapMode::Pipe, Some(tail)),
        }
    } else {
        // Argv only exists in the PowerShell forms.
        let tail = rest.strip_prefix(" -- ").filter(|_| powershell)?;
        (WrapMode::Argv, Some(tail))
    };
    Some(Unwrapped { shim_path: shim_path.to_owned(), mode, original: original.map(str::to_owned) })
}

/// A simple original shaped like `(…)` would read back as PipeGrouped.
fn looks_grouped(original: &str) -> bool {
    original.starts_with('(') && original.ends_with(')')
}

/// Whether a simple bash original still runs the same after `… | `: it must not start with a
/// comment (nothing would follow the pipe) or with the pipeline-only reserved words `!` / `time`.
fn bash_pipe_element(original: &str) -> bool {
    let t = original.trim_start();
    let first_word = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
    !t.starts_with('#') && first_word != "!" && first_word != "time"
}

fn is_pwsh_single_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

fn is_pwsh_double_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201C}' | '\u{201D}' | '\u{201E}')
}

/// Whether a (non-compound) PowerShell original begins with a command that stays one when it
/// follows `… | ` (Pipe) or, for `argv`, when its words become the shim's arguments after `-- `.
///
/// Rejected: nothing but comments; expressions (`$x`, `(…)`, `"…"`, `5`, `-1`, `>…`), which are
/// only allowed as the FIRST element of a pipeline. For Argv also the call / dot-source operators
/// (`&`, `. x`, `.$x`), which are only operators at a command's start, and a line break before
/// the command, which would end the shim's statement right after `--`. For both, a command that
/// is not a native program (see [`pwsh_native_name`]) or is a script block.
fn pwsh_starts_with_command(original: &str, argv: bool) -> bool {
    let c: Vec<char> = original.chars().collect();
    let mut i = 0;
    let mut newline_before = false;
    // Skip whitespace and comments.
    loop {
        match c.get(i) {
            Some('\n' | '\r') => {
                newline_before = true;
                i += 1;
            }
            Some(ch) if ch.is_whitespace() => i += 1,
            Some('<') if c.get(i + 1) == Some(&'#') => {
                match (i + 2..c.len().saturating_sub(1)).find(|&j| c[j] == '#' && c[j + 1] == '>') {
                    Some(end) => i = end + 2,
                    None => return false,
                }
            }
            Some('#') => match c[i..].iter().position(|&x| x == '\n' || x == '\r') {
                Some(nl) => i += nl,
                None => return false,
            },
            Some(_) => break,
            None => return false,
        }
    }
    let first = c[i];
    let second = c.get(i + 1).copied();
    let expression = first.is_ascii_digit()
        || (first == '.' && second.is_some_and(|n| n.is_ascii_digit()))
        || is_pwsh_single_quote(first)
        || is_pwsh_double_quote(first)
        || matches!(
            first,
            '$' | '@'
                | '('
                | '['
                | '{'
                | '-'
                | '+'
                | '!'
                | ','
                | '<'
                | '>'
                | '|'
                | '`'
                | '\u{2013}'
                | '\u{2014}'
                | '\u{2015}'
        );
    if expression {
        return false;
    }
    let dot_source = first == '.'
        && second.is_none_or(|n| {
            n.is_whitespace()
                || matches!(n, '$' | '(' | '{' | '@')
                || is_pwsh_single_quote(n)
                || is_pwsh_double_quote(n)
        });
    if argv && (newline_before || first == '&' || dot_source) {
        return false;
    }
    // The command name, after an optional call / dot-source operator.
    if first == '&' || dot_source {
        i += 1;
        while c.get(i).is_some_and(|ch| ch.is_whitespace()) {
            i += 1;
        }
    }
    let name: String = match c.get(i) {
        // A script block runs in-process: native programs inside it do not get the piped JSON.
        None | Some('{') => return false,
        // A computed command name (`& $cmd`, `& (…)`): nothing to check statically.
        Some('$' | '(' | '@') => return true,
        Some(&q) if is_pwsh_single_quote(q) || is_pwsh_double_quote(q) => {
            let single = is_pwsh_single_quote(q);
            let close = |x: char| if single { is_pwsh_single_quote(x) } else { is_pwsh_double_quote(x) };
            c[i + 1..].iter().take_while(|&&x| !close(x)).collect()
        }
        Some(_) => c[i..]
            .iter()
            .take_while(|&&x| !x.is_whitespace() && !matches!(x, '(' | ')' | '{' | '}' | ';' | ',' | '|' | '&'))
            .collect(),
    };
    // Argv is the LegacyPowerShell form.
    pwsh_native_name(&name, argv)
}

/// PowerShell statement keywords: at a statement's start they begin a statement, but after
/// `| ` or `-- ` they are just (unknown) command names. Checked against pwsh 7.6 and 5.1.
const PWSH_KEYWORDS: &[&str] = &[
    "begin",
    "break",
    "class",
    "clean",
    "configuration",
    "continue",
    "data",
    "define",
    "do",
    "dynamicparam",
    "end",
    "enum",
    "exit",
    "filter",
    "for",
    "foreach",
    "from",
    "function",
    "if",
    "param",
    "parallel",
    "process",
    "return",
    "sequence",
    "switch",
    "throw",
    "trap",
    "try",
    "using",
    "var",
    "while",
    "workflow",
];

/// Built-in aliases and functions of pwsh 7.6 and Windows PowerShell 5.1 (on Windows), which
/// resolve to cmdlets / script functions rather than native programs.
const PWSH_ALIASES: &[&str] = &[
    "?",
    "%",
    "ac",
    "asnp",
    "cat",
    "cd",
    "cd..",
    "cd\\",
    "cd~",
    "cfs",
    "chdir",
    "clc",
    "clear",
    "clhy",
    "cli",
    "clp",
    "cls",
    "clv",
    "cnsn",
    "compare",
    "copy",
    "cp",
    "cpi",
    "cpp",
    "cvpa",
    "dbp",
    "del",
    "diff",
    "dir",
    "dnsn",
    "ebp",
    "echo",
    "epal",
    "epcsv",
    "epsn",
    "erase",
    "etsn",
    "exsn",
    "fc",
    "fhx",
    "fl",
    "foreach",
    "ft",
    "fw",
    "gal",
    "gbp",
    "gc",
    "gci",
    "gcm",
    "gcs",
    "gdr",
    "gerr",
    "ghy",
    "gi",
    "gjb",
    "gl",
    "gm",
    "gmo",
    "gp",
    "gps",
    "gpv",
    "group",
    "gsn",
    "gsnp",
    "gsv",
    "gu",
    "gv",
    "gwmi",
    "h",
    "help",
    "history",
    "icm",
    "iex",
    "ihy",
    "ii",
    "importsystemmodules",
    "ipal",
    "ipcsv",
    "ipmo",
    "ipsn",
    "irm",
    "ise",
    "iwmi",
    "iwr",
    "kill",
    "lp",
    "ls",
    "man",
    "md",
    "measure",
    "mi",
    "mkdir",
    "more",
    "mount",
    "move",
    "mp",
    "mv",
    "nal",
    "ndr",
    "ni",
    "nmo",
    "npssc",
    "nsn",
    "nv",
    "ogv",
    "oh",
    "oss",
    "pause",
    "popd",
    "prompt",
    "ps",
    "pushd",
    "pwd",
    "r",
    "rbp",
    "rcjb",
    "rcsn",
    "rd",
    "rdr",
    "ren",
    "ri",
    "rjb",
    "rm",
    "rmdir",
    "rmo",
    "rni",
    "rnp",
    "rp",
    "rsn",
    "rsnp",
    "rujb",
    "rv",
    "rvpa",
    "rwmi",
    "sajb",
    "sal",
    "saps",
    "sasv",
    "sbp",
    "sc",
    "select",
    "set",
    "shcm",
    "si",
    "sl",
    "sleep",
    "sls",
    "sort",
    "sp",
    "spjb",
    "spps",
    "spsv",
    "start",
    "sujb",
    "sv",
    "swmi",
    "tabexpansion2",
    "tee",
    "trcm",
    "type",
    "where",
    "wjb",
    "write",
];

/// Aliases of `Invoke-WebRequest` in Windows PowerShell 5.1 only; pwsh 7 runs curl.exe / wget.exe.
const LEGACY_ONLY_ALIASES: &[&str] = &["curl", "wget"];

/// PowerShell's approved verbs (`Get-Verb`) plus the unapproved ones built-in commands use
/// (`ForEach-Object`, `Where-Object`, `Sort-Object`, `Tee-Object`, …): `<verb>-<noun>` names
/// are cmdlets or functions.
const PWSH_VERBS: &[&str] = &[
    "add",
    "approve",
    "assert",
    "backup",
    "block",
    "build",
    "checkpoint",
    "clear",
    "close",
    "compare",
    "complete",
    "compress",
    "confirm",
    "connect",
    "convert",
    "convertfrom",
    "convertto",
    "copy",
    "debug",
    "delete",
    "deny",
    "deploy",
    "disable",
    "disconnect",
    "dismount",
    "edit",
    "enable",
    "enter",
    "exit",
    "expand",
    "export",
    "find",
    "flush",
    "foreach",
    "format",
    "get",
    "grant",
    "group",
    "hide",
    "import",
    "initialize",
    "install",
    "invoke",
    "join",
    "limit",
    "lock",
    "measure",
    "merge",
    "mount",
    "move",
    "new",
    "open",
    "optimize",
    "out",
    "ping",
    "pop",
    "protect",
    "publish",
    "push",
    "read",
    "receive",
    "redo",
    "register",
    "remove",
    "rename",
    "repair",
    "request",
    "reset",
    "resize",
    "resolve",
    "restart",
    "restore",
    "resume",
    "revoke",
    "save",
    "search",
    "select",
    "send",
    "set",
    "show",
    "skip",
    "sort",
    "split",
    "start",
    "step",
    "stop",
    "submit",
    "suspend",
    "switch",
    "sync",
    "tee",
    "test",
    "trace",
    "unblock",
    "undo",
    "uninstall",
    "unlock",
    "unprotect",
    "unpublish",
    "unregister",
    "update",
    "use",
    "wait",
    "watch",
    "where",
    "write",
];

/// Whether a PowerShell command name can be a native program. Rejected: statement keywords,
/// `.ps1` scripts (they read the JSON from `[Console]::In`, which the shim has already drained
/// in the Pipe form and which Argv cannot spawn), built-in aliases / functions (for `legacy`
/// also [`LEGACY_ONLY_ALIASES`]), and `<approved verb>-<noun>` cmdlet names.
fn pwsh_native_name(name: &str, legacy: bool) -> bool {
    let lower = name.to_lowercase();
    if lower.ends_with(".ps1")
        || PWSH_KEYWORDS.contains(&lower.as_str())
        || PWSH_ALIASES.contains(&lower.as_str())
        || (legacy && LEGACY_ONLY_ALIASES.contains(&lower.as_str()))
    {
        return false;
    }
    let cmdlet = lower.split_once('-').is_some_and(|(verb, noun)| {
        PWSH_VERBS.contains(&verb) && !noun.is_empty() && !noun.contains(['.', '/', '\\', ':'])
    });
    !cmdlet
}

/// What the quote-aware scan of a command found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Scan {
    /// An unquoted command separator.
    compound: bool,
    /// An unquoted `(` or `)`.
    parens: bool,
    /// The text ends with an unquoted escape character (it would escape our closing paren).
    trailing_escape: bool,
    /// An unquoted escape character anywhere.
    escapes: bool,
    /// A quote (or comment block / here-string) is still open at the end of the text.
    unterminated: bool,
    /// A line comment runs to the end of the text (it would swallow our closing paren). For cmd:
    /// an unquoted `rem` / `::` word anywhere.
    trailing_comment: bool,
    /// A bash heredoc (`<<`): its terminator line must stand alone, so it cannot be grouped.
    heredoc: bool,
    /// A cmd `&`, `&&`, `|` or `||` with no command after it (a syntax error inside `( … )`).
    trailing_operator: bool,
    /// A cmd `if` command: cmd.exe mis-serialises `if exist` / `if defined` / `equ` … when it
    /// re-parses the right side of a pipe ("… was unexpected at this time").
    if_command: bool,
    /// A PowerShell `-name:value` parameter token. After `--` PowerShell passes it to a native
    /// program as two arguments (`-name:` and `value`), so the Argv form would change it.
    colon_parameter: bool,
}

impl Scan {
    /// Whether `(` + text + `)` parses as the same commands for `shell`.
    fn groupable(&self, shell: ShellKind) -> bool {
        let base = !self.trailing_escape && !self.unterminated && !self.trailing_comment;
        match shell {
            ShellKind::Bash => base && !self.heredoc,
            ShellKind::Cmd => base && !self.parens && !self.trailing_operator,
            ShellKind::Pwsh | ShellKind::LegacyPowerShell => false,
        }
    }
}

fn scan(cmd: &str, shell: ShellKind) -> Scan {
    let c: Vec<char> = cmd.chars().collect();
    match shell {
        ShellKind::Bash => scan_bash(&c),
        ShellKind::Cmd => scan_cmd(&c),
        ShellKind::Pwsh | ShellKind::LegacyPowerShell => scan_pwsh(&c),
    }
}

/// Index of the first char at or after `from` that satisfies `close`, skipping the char after
/// each `escape`. `None` if the region never closes.
fn find_close(c: &[char], from: usize, close: impl Fn(char) -> bool, escape: Option<char>) -> Option<usize> {
    let mut j = from;
    while j < c.len() {
        if Some(c[j]) == escape {
            j += 2;
            continue;
        }
        if close(c[j]) {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Whether a newline at `i` separates two non-blank parts of the command.
fn separates(c: &[char], i: usize) -> bool {
    c[..i].iter().any(|ch| !ch.is_whitespace()) && c[i + 1..].iter().any(|ch| !ch.is_whitespace())
}

fn scan_bash(c: &[char]) -> Scan {
    let mut s = Scan::default();
    let mut i = 0;
    let mut word_start = true; // the next char begins a word (so `#` would start a comment)
    let mut dollar = false; // the previous char was an unquoted `$` (for `$'…'`)
    let mut redirect = false; // the previous char was an unquoted `<` or `>` (for `>&`)
    while i < c.len() {
        let at_word_start = std::mem::replace(&mut word_start, false);
        let after_dollar = std::mem::take(&mut dollar);
        let after_redirect = std::mem::take(&mut redirect);
        let next = c.get(i + 1).copied();
        match c[i] {
            '\\' => {
                // Escapes the next char; `\` + newline is a line continuation.
                if next.is_none() {
                    s.trailing_escape = true;
                }
                i += 2;
                continue;
            }
            '\'' | '"' | '`' => {
                let q = c[i];
                let escape = if q == '\'' && !after_dollar { None } else { Some('\\') };
                match find_close(c, i + 1, |x| x == q, escape) {
                    Some(end) => i = end + 1,
                    None => {
                        s.unterminated = true;
                        break;
                    }
                }
                continue;
            }
            '#' if at_word_start => match c[i..].iter().position(|&x| x == '\n') {
                Some(nl) => {
                    i += nl;
                    continue;
                }
                None => {
                    s.trailing_comment = true;
                    break;
                }
            },
            ';' => {
                s.compound = true;
                word_start = true;
            }
            '&' => {
                if after_redirect {
                    // `2>&1`, `<&0`: file-descriptor duplication.
                } else if next == Some('&') {
                    s.compound = true;
                    i += 1;
                } else if next == Some('>') {
                    // `&>file` / `&>>file`: redirect stdout and stderr.
                    redirect = true;
                    i += 1;
                } else {
                    s.compound = true;
                }
                word_start = true;
            }
            '|' => {
                if next == Some('|') {
                    s.compound = true;
                    i += 1;
                } else if next == Some('&') {
                    // `|&` pipes stderr too; still one pipeline.
                    i += 1;
                }
                word_start = true;
            }
            '\n' => {
                if separates(c, i) {
                    s.compound = true;
                }
                word_start = true;
            }
            '(' | ')' => {
                s.parens = true;
                word_start = true;
            }
            '<' => {
                if next == Some('<') {
                    if c.get(i + 2) == Some(&'<') {
                        i += 2; // `<<<` here-string: a plain word follows.
                    } else {
                        s.heredoc = true;
                        i += 1;
                    }
                }
                redirect = true;
                word_start = true;
            }
            '>' => {
                redirect = true;
                word_start = true;
            }
            ' ' | '\t' => word_start = true,
            '$' => dollar = true,
            _ => {}
        }
        i += 1;
    }
    s
}

fn scan_cmd(c: &[char]) -> Scan {
    let mut s = Scan::default();
    let mut i = 0;
    let mut redirect = false;
    // Nothing but whitespace, `@` and the delimiters cmd skips before a command name since the
    // start or an operator: the next word is the command name.
    let mut command_start = true;
    // Whether anything but whitespace follows index `j`.
    let more_after = |j: usize| c.get(j + 1..).is_some_and(|rest| rest.iter().any(|ch| !ch.is_whitespace()));
    while i < c.len() {
        let after_redirect = std::mem::take(&mut redirect);
        let next = c.get(i + 1).copied();
        // `rem` / `::` comment out the rest of the line (inside `( … )` including the `)`).
        // Checked at every unquoted word, since redirections may precede the command name.
        if (i == 0 || !c[i - 1].is_alphanumeric()) && cmd_comment_at(c, i) {
            s.trailing_comment = true;
        }
        if command_start && !(c[i].is_whitespace() || matches!(c[i], '@' | ';' | ',' | '=')) {
            command_start = false;
            s.if_command |= cmd_if_at(c, i);
        }
        match c[i] {
            '^' => {
                s.escapes = true;
                if next.is_none() {
                    s.trailing_escape = true;
                }
                i += 2;
                continue;
            }
            '"' => {
                match find_close(c, i + 1, |x| x == '"', None) {
                    Some(end) => i = end + 1,
                    None => {
                        s.unterminated = true;
                        break;
                    }
                }
                continue;
            }
            // cmd ignores a trailing `&` / `&&` / `||` with no command after it, so only one that
            // separates two commands is compound (a dangling one cannot be grouped, though).
            '&' => {
                if !after_redirect {
                    if next == Some('&') {
                        i += 1;
                    }
                    if more_after(i) {
                        s.compound = true;
                    } else {
                        s.trailing_operator = true;
                    }
                    command_start = true;
                }
            }
            '|' => {
                let double = next == Some('|');
                if double {
                    i += 1;
                }
                if !more_after(i) {
                    s.trailing_operator = true;
                } else if double {
                    s.compound = true;
                }
                command_start = true;
            }
            ';' => s.compound = true,
            '\n' => {
                if separates(c, i) {
                    s.compound = true;
                }
                command_start = true;
            }
            '(' => {
                s.parens = true;
                command_start = true;
            }
            ')' => s.parens = true,
            '<' | '>' => redirect = true,
            _ => {}
        }
        i += 1;
    }
    s
}

/// The cmd `if` command starts at `c[i]` (cmd ends the word `if` at whitespace, `;`, `,`, `=`
/// or `(`; `if/i`, `if.exe` or `if"x"` are other command names).
fn cmd_if_at(c: &[char], i: usize) -> bool {
    c.get(i..i + 2).is_some_and(|w| w.iter().collect::<String>().eq_ignore_ascii_case("if"))
        && c.get(i + 2).is_none_or(|ch| ch.is_whitespace() || matches!(ch, ';' | ',' | '=' | '('))
}

/// A cmd `rem` or `::` comment starts at `c[i]`.
fn cmd_comment_at(c: &[char], i: usize) -> bool {
    let rem = c.get(i..i + 3).is_some_and(|w| w.iter().collect::<String>().eq_ignore_ascii_case("rem"))
        && c.get(i + 3).is_none_or(|ch| !ch.is_alphanumeric());
    rem || c.get(i..i + 2) == Some(&[':', ':'][..])
}

fn scan_pwsh(c: &[char]) -> Scan {
    let mut s = Scan::default();
    let mut i = 0;
    // A `#` here starts a comment (start of text, or after whitespace / an operator).
    let mut token_start = true;
    // Only trivia since the start or a separator, `|`, `(` or `{`: a `&` here is the call
    // operator. Anywhere else it is PowerShell 7's background operator, which ends the statement.
    let mut command_start = true;
    // Only trivia since an unquoted single `|`: a line break continues the pipeline.
    let mut after_pipe = false;
    // Index of the char that began the current token.
    let mut token_begin = 0;
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        // Trivia (whitespace, comments) leaves the states as they are.
        if ch.is_whitespace() {
            if matches!(ch, '\n' | '\r') && !after_pipe && separates(c, i) {
                s.compound = true;
                command_start = true;
            }
            token_start = true;
            i += 1;
            continue;
        }
        if ch == '<' && next == Some('#') && token_start {
            match (i + 2..c.len().saturating_sub(1)).find(|&j| c[j] == '#' && c[j + 1] == '>') {
                Some(end) => {
                    i = end + 2;
                    token_start = true;
                    continue;
                }
                None => {
                    s.unterminated = true;
                    break;
                }
            }
        }
        if ch == '#' && token_start {
            match c[i..].iter().position(|&x| x == '\n' || x == '\r') {
                Some(nl) => {
                    i += nl;
                    continue;
                }
                None => {
                    s.trailing_comment = true;
                    break;
                }
            }
        }
        // Everything below is part of a word or an operator.
        let was_command_start = std::mem::replace(&mut command_start, false);
        let at_token_start = std::mem::replace(&mut token_start, false);
        if at_token_start {
            token_begin = i;
            s.colon_parameter |= pwsh_colon_parameter_at(c, i);
        }
        after_pipe = false;
        if ch == '`' {
            if next.is_none() {
                s.trailing_escape = true;
            }
            i += 2;
            continue;
        }
        if ch == '@' && at_token_start && next.is_some_and(|q| is_pwsh_single_quote(q) || is_pwsh_double_quote(q)) {
            match here_string_end(c, i) {
                Some(Some(end)) => {
                    i = end;
                    token_start = true;
                    continue;
                }
                Some(None) => {
                    s.unterminated = true;
                    break;
                }
                None => {}
            }
        }
        if is_pwsh_single_quote(ch) || is_pwsh_double_quote(ch) {
            // `''` / `""` inside a string are escaped quotes; closing and reopening at the same
            // spot yields the same quoted/unquoted classification.
            let found = if is_pwsh_single_quote(ch) {
                find_close(c, i + 1, is_pwsh_single_quote, None)
            } else {
                find_close(c, i + 1, is_pwsh_double_quote, Some('`'))
            };
            match found {
                // A string that began a token also ends it (`"x"#c` has a comment); one embedded in
                // a word (`a"x"#c`) does not.
                Some(end) => {
                    i = end + 1;
                    token_start = at_token_start;
                }
                None => {
                    s.unterminated = true;
                    break;
                }
            }
            continue;
        }
        match ch {
            ';' => {
                s.compound = true;
                command_start = true;
            }
            '&' | '|' if next == Some(ch) => {
                s.compound = true;
                command_start = true;
                i += 1;
            }
            '|' => {
                after_pipe = true;
                command_start = true;
            }
            '&' if !was_command_start && !pwsh_merge_redirect(c, i, token_begin) => {
                s.compound = true;
                command_start = true;
            }
            '(' => {
                s.parens = true;
                command_start = true;
            }
            ')' => s.parens = true,
            '{' => command_start = true,
            _ => {}
        }
        token_start = matches!(ch, ';' | '|' | '&' | '(' | ')' | '{' | '}' | ',');
        i += 1;
    }
    s
}

fn is_pwsh_dash(c: char) -> bool {
    matches!(c, '-' | '\u{2013}' | '\u{2014}' | '\u{2015}')
}

/// Whether a PowerShell parameter token with a colon argument (`-name:value`, `-name:`) starts
/// at `c[i]`. The name starts with a letter, `_` or `?` and runs up to whitespace, a quote or
/// one of `(){};,|&.[`; `$` and `` ` `` do not end it (`-a$env:X` is `-a$env:` + `X`).
fn pwsh_colon_parameter_at(c: &[char], i: usize) -> bool {
    if !is_pwsh_dash(c[i]) || !c.get(i + 1).is_some_and(|&n| n.is_alphabetic() || n == '_' || n == '?') {
        return false;
    }
    let ends = |ch: char| {
        ch.is_whitespace()
            || is_pwsh_single_quote(ch)
            || is_pwsh_double_quote(ch)
            || matches!(ch, '(' | ')' | '{' | '}' | ';' | ',' | '|' | '&' | '.' | '[')
    };
    c[i + 1..].iter().take_while(|&&ch| !ends(ch)).any(|&ch| ch == ':')
}

/// Whether the `&` at `c[i]` belongs to a PowerShell merging redirection such as `2>&1` or
/// `*>&1`. PowerShell only lexes those as one token when they start a token (`x2>&1` is the word
/// `x2>` followed by the background operator); `token_begin` is where the current token began.
fn pwsh_merge_redirect(c: &[char], i: usize, token_begin: usize) -> bool {
    i >= 2
        && token_begin == i - 2
        && (c[i - 2].is_ascii_digit() || c[i - 2] == '*')
        && c[i - 1] == '>'
        && c.get(i + 1).is_some_and(char::is_ascii_digit)
}

/// If `c[at..]` opens a PowerShell here-string (`@'` or `@"` followed only by spaces/tabs up to
/// the end of the line), returns `Some(index after its closing '@ / "@)` or `Some(None)` if it
/// never closes. Returns `None` when it is not a here-string header.
fn here_string_end(c: &[char], at: usize) -> Option<Option<usize>> {
    let double = is_pwsh_double_quote(*c.get(at + 1)?);
    let mut j = at + 2;
    while matches!(c.get(j), Some(' ' | '\t')) {
        j += 1;
    }
    match c.get(j) {
        Some('\n') => j += 1,
        Some('\r') if c.get(j + 1) == Some(&'\n') => j += 2,
        _ => return None,
    }
    let closes = |q: char| if double { is_pwsh_double_quote(q) } else { is_pwsh_single_quote(q) };
    // The terminator is a quote + `@` at the start of a line.
    while j < c.len() {
        let line_start = matches!(c[j - 1], '\n' | '\r');
        if line_start && closes(c[j]) && c.get(j + 1) == Some(&'@') {
            return Some(Some(j + 2));
        }
        j += 1;
    }
    Some(None)
}
