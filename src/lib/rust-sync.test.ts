// Guards for values the UI mirrors from the Rust side (the source of truth): each test reads the
// Rust file and fails when the TypeScript copy drifts from it.

import { describe, expect, it } from "vitest";
import { CRIT_AT, WARN_AT } from "./color";
import { windowLabel, windowShort } from "./format";
import { createMockBackend } from "./mock";
import { MOCK_HISTORY_DAYS } from "./mock-history";
import { DEFAULT_CTX_THRESHOLDS, DEFAULT_THRESHOLDS } from "./thresholds";
import type { Settings } from "./types";
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

  it("the mock's card grows by the extra limit rows like window.rs", () => {
    const sum = (expr: string) => expr.split(/(?=[+-])/).reduce((a, term) => a + Number(term.replace(/\s/g, "")), 0);
    const block = sum(grab(win, /const CARD_EXTRA_H: f64 = ([\d.+\- ]+);/));
    const row = sum(grab(win, /const CARD_EXTRA_ROW_H: f64 = ([\d.+\- ]+);/));
    expect(css).toContain(`html.mock:has(.card[data-extra-rows]) { --add-extra: calc(${block}px + ${row}px * var(--extra-rows, 0)); }`);
    expect(win).toContain("h += CARD_EXTRA_H + CARD_EXTRA_ROW_H * extra_rows as f64;");
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

describe("default thresholds", () => {
  it("the UI and the browser mock use core's defaults (the one source the app's settings use)", async () => {
    const list = (path: string, name: string) =>
      JSON.parse(grab(source(path), new RegExp(String.raw`pub const ${name}: &\[u8\] = &(\[[\d, ]*\]);`))) as number[];
    const usage = list("crates/core/src/alerts.rs", "DEFAULT_THRESHOLDS");
    const ctx = list("crates/core/src/ctx_alerts.rs", "DEFAULT_CTX_THRESHOLDS");
    const settings = source("src-tauri/src/settings.rs");
    expect(settings).toContain("pub use mikyas_core::alerts::DEFAULT_THRESHOLDS;");
    expect(settings).toContain("pub use mikyas_core::ctx_alerts::DEFAULT_CTX_THRESHOLDS;");
    expect(source("crates/core/src/alerts.rs")).toContain("thresholds: DEFAULT_THRESHOLDS.to_vec()");
    const s = await createMockBackend(new URLSearchParams()).invoke<Settings>("get_settings");
    expect(DEFAULT_THRESHOLDS).toEqual(usage);
    expect(DEFAULT_CTX_THRESHOLDS).toEqual(ctx);
    expect(s.thresholds).toEqual(usage);
    expect(s.ctx_thresholds).toEqual(ctx);
  });
});

describe("types.rs window names", () => {
  it("span.ts names every key the way WindowKind::label / short_label do", () => {
    const rs = source("crates/core/src/engine/types.rs");
    const table = rs.slice(rs.indexOf("fn labels_are_derived_from_the_key"));
    const cases = [...table.slice(0, table.indexOf("];")).matchAll(/\("([^"]+)", "([^"]+)", "([^"]+)"\)/g)];
    expect(cases.length).toBeGreaterThanOrEqual(10);
    for (const [, key, label, short] of cases) {
      // Desktop's two-letter short keys are Rust-only seeds (Rust always sends their names).
      if (/^[a-z]{2}$/.test(key)) continue;
      expect(windowLabel(key), key).toBe(label);
      expect(windowShort(key), key).toBe(short);
    }
  });
});
