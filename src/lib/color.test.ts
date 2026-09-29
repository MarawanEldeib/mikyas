import { describe, expect, it } from "vitest";
import { formatPct } from "./format";
import {
  ACCENTS,
  ACCENT_NAMES,
  CRIT_AT,
  WARN_AT,
  bandOffset,
  clampPct,
  contrast,
  fillColor,
  level,
  over,
  parseRgb,
  textColor,
  type Rgb,
} from "./color";

describe("bandOffset", () => {
  it("switches bands where the shown number does", () => {
    // 39.5 shows "40" (orange); the gradient must already be orange there.
    expect(level(WARN_AT - 0.5)).toBe("warn");
    expect(level(WARN_AT - 0.51)).toBe("ok");
    expect(bandOffset(WARN_AT)).toBeCloseTo((WARN_AT - 0.5) / 100);
    expect(level(CRIT_AT - 0.5)).toBe("crit");
    expect(bandOffset(CRIT_AT)).toBeCloseTo((CRIT_AT - 0.5) / 100);
  });
});

describe("level", () => {
  it("uses fixed bands: green < 40, orange 40–69, red >= 70", () => {
    expect(level(0)).toBe("ok");
    expect(level(39.4)).toBe("ok");
    expect(level(39.5)).toBe("warn"); // shown as 40
    expect(level(40)).toBe("warn");
    expect(level(69.4)).toBe("warn");
    expect(level(69.5)).toBe("crit"); // shown as 70
    expect(level(70)).toBe("crit");
    expect(level(100)).toBe("crit");
    expect(level(250)).toBe("crit");
  });

  it("treats non-finite input as 0", () => {
    expect(level(Number.NaN)).toBe("ok");
    expect(level(Number.POSITIVE_INFINITY)).toBe("ok");
    expect(level(-5)).toBe("ok");
  });

  it("agrees with the number shown", () => {
    for (let p = -2; p <= 102; p += 0.05) {
      const shown = Number(formatPct(p));
      expect(level(p), `${p}`).toBe(level(shown));
    }
  });
});

describe("colour tokens", () => {
  it("maps bands to CSS variables", () => {
    expect(textColor(10)).toBe("var(--ok)");
    expect(textColor(55)).toBe("var(--warn)");
    expect(fillColor(90)).toBe("var(--crit-fill)");
  });
});

describe("clampPct", () => {
  it("clamps into 0..100", () => {
    expect(clampPct(-3)).toBe(0);
    expect(clampPct(42.5)).toBe(42.5);
    expect(clampPct(140)).toBe(100);
    expect(clampPct(Number.NaN)).toBe(0);
  });
});

describe("contrast maths", () => {
  it("parses colours and composites", () => {
    expect(parseRgb("#60cdff")).toEqual([96, 205, 255]);
    expect(parseRgb(" 30 30 32 ")).toEqual([30, 30, 32]);
    expect(parseRgb("#fff")).toBeNull();
    expect(parseRgb("1 2")).toBeNull();
    expect(over([255, 255, 255], 0.5, [0, 0, 0])).toEqual([128, 128, 128]);
  });

  it("matches the WCAG reference values", () => {
    expect(contrast([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5);
    expect(contrast([255, 255, 255], [255, 255, 255])).toBe(1);
    // #767676 on white is the classic 4.54:1.
    expect(contrast([118, 118, 118], [255, 255, 255])).toBeCloseTo(4.54, 2);
  });
});

// Vitest blanks CSS imports (even ?raw), so the stylesheet is read from disk. There are no Node
// typings in this project; the one fs call is typed here.
type NodeFs = { readFileSync(path: URL, encoding: "utf8"): string };
const nodeProcess = (globalThis as unknown as { process: { getBuiltinModule(id: "node:fs"): NodeFs } }).process;
const css = nodeProcess.getBuiltinModule("node:fs").readFileSync(new URL("../app.css", import.meta.url), "utf8");

/** `--name: value;` declarations of the first `:root { … }` block at or after `from`. */
function rootTokens(from: number): Map<string, string> {
  const start = css.indexOf(":root {", from);
  const body = css.slice(start, css.indexOf("}", start));
  return new Map(Array.from(body.matchAll(/--([\w-]+):\s*([^;]+);/g), (m) => [m[1], m[2].trim()]));
}

function rgba(v: string): { rgb: Rgb; a: number } {
  const m = /^rgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)$/.exec(v);
  if (!m) throw new Error(`not an rgb() with alpha: ${v}`);
  return { rgb: [Number(m[1]), Number(m[2]), Number(m[3])], a: Number(m[4]) };
}

describe("accent palette (app.css)", () => {
  const themes = {
    dark: rootTokens(0),
    light: rootTokens(css.indexOf("@media (prefers-color-scheme: light)")),
  };

  it.each(Object.entries(themes))("is WCAG AA on the %s surface", (_theme, tokens) => {
    const get = (name: string): Rgb => {
      const c = parseRgb(tokens.get(name) ?? tokens.get(name.replace(/^accent-/, "")) ?? "");
      if (!c) throw new Error(`missing or malformed --${name}`);
      return c;
    };
    // The opaque surface over a white or black wallpaper, and a settings group on top of it.
    const surface = get("surface-rgb");
    const card = rgba(tokens.get("fill-card") ?? "");
    const surfaces = [[255, 255, 255] as Rgb, [0, 0, 0] as Rgb].flatMap((wall) => {
      const s = over(surface, 0.97, wall);
      return [s, over(card.rgb, card.a, s)];
    });
    for (const accent of ACCENTS) {
      const [fill, hover, on] =
        accent === "auto"
          ? [get("accent"), get("accent-hover"), get("on-accent")]
          : [get(`accent-${accent}`), get(`accent-${accent}-hover`), get(`accent-${accent}-on`)];
      for (const s of surfaces) {
        expect(contrast(fill, s), `${accent} on the surface`).toBeGreaterThanOrEqual(3);
        expect(contrast(hover, s), `${accent} hover on the surface`).toBeGreaterThanOrEqual(3);
      }
      expect(contrast(on, fill), `${accent} text on the accent`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(on, hover), `${accent} text on the hover`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("maps every named accent onto the chrome tokens", () => {
    for (const accent of ACCENTS.filter((a) => a !== "auto")) {
      expect(css).toContain(
        `:root[data-accent="${accent}"] { --accent: var(--accent-${accent}); --accent-hover: var(--accent-${accent}-hover); --on-accent: var(--accent-${accent}-on); }`,
      );
    }
    expect(Object.keys(ACCENT_NAMES)).toEqual([...ACCENTS]);
  });
});
