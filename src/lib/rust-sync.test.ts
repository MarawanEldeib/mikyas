// Guards for values the UI mirrors from the Rust side (the source of truth): each test reads the
// Rust file and fails when the TypeScript copy drifts from it.

import { describe, expect, it } from "vitest";
import { CRIT_AT, WARN_AT } from "./color";
import { MOCK_HISTORY_DAYS } from "./mock-history";
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

describe("level.rs", () => {
  const rs = source("crates/core/src/level.rs");

  it("colour bands start at the same %", () => {
    expect(WARN_AT).toBe(rustConst(rs, "WARN_AT"));
    expect(CRIT_AT).toBe(rustConst(rs, "CRIT_AT"));
  });

  it("rounds to the shown number the way the UI does", () => {
    expect(rs).toContain("pct.clamp(0.0, 100.0).round() as u8");
  });
});

describe("window sizes", () => {
  const win = source("src-tauri/src/window.rs");
  const dock = source("src-tauri/src/dock.rs");
  const css = source("src/app.css");

  /** The mock's `--mock-w` / `--mock-h` for a CSS selector (px, first term of a calc). */
  function mockSize(selector: string): [number, number] {
    const at = css.indexOf(`${selector} {`);
    expect(at, selector).toBeGreaterThanOrEqual(0);
    const rule = css.slice(at, css.indexOf("}", at));
    return [Number(grab(rule, /--mock-w: (?:calc\()?([\d.]+)px/)), Number(grab(rule, /--mock-h: (?:calc\()?([\d.]+)px/))];
  }

  it("the browser mock sizes every view like window.rs", () => {
    const views = [...win.matchAll(/ViewMode::(\w+) => \(([\d.]+), ([\d.]+)\)/g)];
    expect(views.length).toBeGreaterThanOrEqual(5);
    for (const [, name, w, h] of views) {
      expect(mockSize(`html.mock[data-view="${name.toLowerCase()}"]`), name).toEqual([Number(w), Number(h)]);
    }
  });

  it("the mock's card cuts match the rows window.rs removes", () => {
    const burn = grab(win, /const CARD_BURN_H: f64 = ([\d.* ]+);/)
      .split("*")
      .reduce((a, b) => a * Number(b.trim()), 1);
    expect(css).toContain(`html.mock:has(.card[data-no-burn]) { --cut-burn: ${burn}px; }`);
    expect(css).toContain(`html.mock:has(.card[data-no-session]) { --cut-session: ${rustConst(win, "CARD_SESSION_H")}px; }`);
  });

  it("the collapsed dock strip matches dock.rs", () => {
    const side = /Self::Left \| Self::Right => \(([\d.]+), ([\d.]+)\)/.exec(dock);
    const top = /Self::Top => \(([\d.]+), ([\d.]+)\)/.exec(dock.slice(dock.indexOf("fn strip_logical")));
    expect(side && top).toBeTruthy();
    expect(mockSize(`html.mock[data-dock-collapsed]:is([data-dock="left"], [data-dock="right"])`)).toEqual([
      Number(side![1]),
      Number(side![2]),
    ]);
    expect(mockSize(`html.mock[data-dock-collapsed][data-dock="top"]`)).toEqual([Number(top![1]), Number(top![2])]);
  });
});

describe("history.rs", () => {
  it("the browser mock keeps as many days as the real history", () => {
    const days = Number(grab(source("crates/core/src/history.rs"), /pub const RETAIN_MS: Ms = (\d+) \* DAY_MS;/));
    expect(MOCK_HISTORY_DAYS).toBe(days);
    expect(source("src-tauri/src/history_view.rs")).toContain("pub const MAX_DAYS: u32 = (RETAIN_MS / DAY_MS) as u32;");
  });
});
