// Privacy grep (plan M6): the widget is token-free, so no code under src-tauri/, crates/ or
// src/ may name the credentials file, the OAuth usage endpoint, claude.ai, an Anthropic host, the
// session cookie or a cookie store. Comments and docs
// may (they explain what the widget never touches); the few code lines that must name one are
// listed in ALLOW. Scans the files git knows about (tracked, plus untracked ones not ignored).
//  - Only whole-line comments are skipped. A trailing `//` is not stripped: it could sit inside
//    a string ("https://…"), so a line with code on it is always checked.
//  - Binary files (a NUL byte) are skipped.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const ROOTS = ["src-tauri", "crates", "src"];
const PATTERNS = [
  /\.credentials/i,
  /api\/oauth/i,
  /claude\.ai/i,
  /anthropic\.com/i, // api.anthropic.com, console.anthropic.com, ...
  /sessionKey/i, // the claude.ai session cookie
  /[\\/]Network[\\/]Cookies/i, // Chromium/Electron cookie store (Claude Desktop's included)
];
const DOCS = /\.(md|txt)$/i;

// Each entry names a file and the exact code it may contain; `tests` limits it to the file's
// `#[cfg(test)] mod tests`. An entry that no longer matches anything fails too, so the list
// cannot go stale.
const ALLOW = [
  {
    file: "crates/core/src/saferead.rs",
    line: /^"\.credentials\.json",$/,
    why: "the read denylist entry",
  },
  {
    file: "crates/core/src/saferead.rs",
    line: /\.join\("\.credentials\.json"\)/,
    tests: true,
    why: "tests that the denylist refuses it",
  },
];

/** Whole-line comment syntax by file extension: line prefixes, and block comment delimiters. */
function commentSyntax(file) {
  const ext = file.slice(file.lastIndexOf(".") + 1).toLowerCase();
  const c = { line: ["//"], blocks: [["/*", "*/"]] };
  switch (ext) {
    case "rs":
    case "ts":
    case "js":
    case "mjs":
    case "css":
      return c;
    case "svelte":
    case "html":
      return { line: c.line, blocks: [...c.blocks, ["<!--", "-->"]] };
    case "toml":
    case "ps1":
      return { line: ["#"], blocks: [] };
    case "nsh":
      return { line: [";", "#"], blocks: c.blocks };
    default:
      return { line: [], blocks: [] }; // JSON, fixtures: data, every line is checked
  }
}

/** The code on each line of `text` (whole-line comments removed), with 1-based line numbers. */
function codeLines(file, text) {
  const { line, blocks } = commentSyntax(file);
  const out = [];
  let close = null; // inside a block comment that ends with this
  text.split(/\r?\n/).forEach((raw, i) => {
    let t = raw.trim();
    for (;;) {
      if (close) {
        const end = t.indexOf(close);
        if (end < 0) return;
        t = t.slice(end + close.length).trim(); // code after the comment still counts
        close = null;
      }
      const block = blocks.find(([open]) => t.startsWith(open));
      if (!block) break;
      t = t.slice(block[0].length);
      close = block[1];
    }
    if (!t || line.some((p) => t.startsWith(p))) return;
    out.push({ n: i + 1, text: t });
  });
  return out;
}

/** 1-based line of a Rust file's `#[cfg(test)]` that opens `mod tests` (the rest of the file is
 * its tests), or Infinity. */
function testsStart(text) {
  const lines = text.split(/\r?\n/).map((l) => l.trim());
  const i = lines.findIndex((l, k) => l === "#[cfg(test)]" && /^mod tests\b/.test(lines[k + 1] ?? ""));
  return i < 0 ? Infinity : i + 1;
}

function listFiles() {
  const out = execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", ...ROOTS], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return [...new Set(out.split("\0").filter(Boolean))];
}

const hits = [];
const used = new Set();
for (const file of listFiles()) {
  if (DOCS.test(file)) continue;
  let buf;
  try {
    buf = readFileSync(file);
  } catch {
    continue; // deleted in the working tree
  }
  if (buf.subarray(0, 8192).includes(0)) continue;
  const source = buf.toString("utf8");
  const tests = testsStart(source);
  for (const { n, text } of codeLines(file, source)) {
    if (!PATTERNS.some((p) => p.test(text))) continue;
    const allowed = ALLOW.findIndex((a) => a.file === file && a.line.test(text) && (!a.tests || n > tests));
    if (allowed >= 0) used.add(allowed);
    else hits.push(`${file}:${n}: ${text}`);
  }
}
const stale = ALLOW.filter((_, i) => !used.has(i)).map((a) => `${a.file}: ${a.line} (${a.why})`);

if (hits.length || stale.length) {
  if (hits.length) console.error("Privacy grep: code names a credential file, api/oauth, claude.ai, an Anthropic host, sessionKey or a cookie store:\n  " + hits.join("\n  "));
  if (stale.length) console.error("Privacy grep: allowlist entries that match nothing:\n  " + stale.join("\n  "));
  process.exit(1);
}
console.log(`Privacy grep: clean (${used.size} allowlisted line patterns).`);
