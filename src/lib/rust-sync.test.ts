// Guards for values the UI mirrors from the Rust side (the source of truth): each test reads the
// Rust file and fails when the TypeScript copy drifts from it.

import { describe, expect, it } from "vitest";
import { MAX_NOTES, MAX_NOTE_CHARS, RELEASES_PREFIX } from "./update";

type NodeFs = { readFileSync(path: URL, encoding: "utf8"): string };
const fs = (globalThis as unknown as { process: { getBuiltinModule(id: "node:fs"): NodeFs } }).process.getBuiltinModule("node:fs");

/** A repository file's text (path from the repository root). */
function source(path: string): string {
  return fs.readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
}

/** The first capture of `re` in `text`; fails the test when it is missing. */
function grab(text: string, re: RegExp): string {
  const m = re.exec(text);
  if (!m) throw new Error(`pattern not found: ${re}`);
  return m[1];
}

/** A Rust numeric `const NAME: T = <literal>;` (underscores and a trailing `.0` allowed). */
function rustConst(text: string, name: string): number {
  return Number(grab(text, new RegExp(String.raw`const ${name}: \w+ = ([\d_.]+);`)).replaceAll("_", ""));
}

describe("updates.rs", () => {
  const rs = source("src-tauri/src/updates.rs");

  it("releases live in the same repository", () => {
    const repo = grab(rs, /macro_rules! repo \{\s*\(\) => \{\s*"([^"]+)"/);
    expect(rs).toContain('RELEASES_PREFIX: &str = concat!("https://github.com/", repo!(), "/releases/")');
    expect(RELEASES_PREFIX).toBe(`https://github.com/${repo}/releases/`);
  });

  it("notes are cut the same way", () => {
    expect(MAX_NOTES).toBe(rustConst(rs, "MAX_NOTES"));
    expect(MAX_NOTE_CHARS).toBe(rustConst(rs, "MAX_NOTE_CHARS"));
  });
});
