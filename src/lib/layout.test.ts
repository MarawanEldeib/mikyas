import { describe, expect, it } from "vitest";
import { CARD_ROWS, UI_SIZES, nearestUiSize, parseCardRows } from "./layout";

describe("nearestUiSize", () => {
  it("snaps any valid scale to a preset", () => {
    expect(UI_SIZES.map((s) => nearestUiSize(s.value))).toEqual([0.9, 1, 1.15, 1.3]);
    expect(nearestUiSize(0.85)).toBe(0.9);
    expect(nearestUiSize(0.96)).toBe(1);
    expect(nearestUiSize(1.1)).toBe(1.15);
    expect(nearestUiSize(1.25)).toBe(1.3);
    expect(nearestUiSize(9)).toBe(1.3);
  });

  it("falls back to Normal for garbage", () => {
    expect(nearestUiSize(Number.NaN)).toBe(1);
    expect(nearestUiSize(Number.POSITIVE_INFINITY)).toBe(1);
  });
});

describe("parseCardRows", () => {
  it("shows every row by default and exactly the listed ones otherwise", () => {
    expect(parseCardRows(null)).toEqual({ sparklines: true, burn: true, session: true, sources: true });
    expect(parseCardRows("sparklines, sources,bogus")).toEqual({ sparklines: true, burn: false, session: false, sources: true });
    expect(parseCardRows("")).toEqual({ sparklines: false, burn: false, session: false, sources: false });
    expect(parseCardRows("none")).toEqual(parseCardRows(""));
  });

  it("lists every row of the contract once", () => {
    expect(CARD_ROWS.map((r) => r.key).sort()).toEqual(["burn", "session", "sources", "sparklines"]);
  });
});
