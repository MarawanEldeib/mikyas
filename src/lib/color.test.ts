import { describe, expect, it } from "vitest";
import { clampPct, fillColor, level, textColor } from "./color";

describe("level", () => {
  it("uses fixed bands: green < 40, orange 40–69, red >= 70", () => {
    expect(level(0)).toBe("ok");
    expect(level(39.9)).toBe("ok");
    expect(level(40)).toBe("warn");
    expect(level(69.99)).toBe("warn");
    expect(level(70)).toBe("crit");
    expect(level(100)).toBe("crit");
    expect(level(250)).toBe("crit");
  });

  it("treats non-finite input as 0", () => {
    expect(level(Number.NaN)).toBe("ok");
    expect(level(Number.POSITIVE_INFINITY)).toBe("ok");
    expect(level(-5)).toBe("ok");
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
