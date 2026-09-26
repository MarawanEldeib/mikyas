import { describe, expect, it } from "vitest";
import { mainWindows, mutedColor } from "./windows";

const w = (kind: string) => ({ kind });

describe("mainWindows", () => {
  it("picks the 5-hour then the weekly window, whatever the order", () => {
    expect(mainWindows([w("seven_day_opus"), w("seven_day"), w("five_hour")])).toEqual([w("five_hour"), w("seven_day")]);
  });
  it("keeps a single main window alone", () => {
    expect(mainWindows([w("seven_day_opus"), w("seven_day")])).toEqual([w("seven_day")]);
  });
  it("falls back to the first two windows without a main one", () => {
    expect(mainWindows([w("a"), w("b"), w("c")])).toEqual([w("a"), w("b")]);
  });
  it("is empty without windows", () => {
    expect(mainWindows([])).toEqual([]);
  });
});

describe("mutedColor", () => {
  const base = { pct: 96, stale: false, phase: "active" as const };
  it("uses the fill colour of the level", () => {
    expect(mutedColor(base)).toBe("var(--crit-fill)");
  });
  it("is neutral while stale or waiting for the first reading after a reset", () => {
    expect(mutedColor({ ...base, stale: true })).toBe("var(--fg-3)");
    expect(mutedColor({ ...base, phase: "reset_awaiting_data" })).toBe("var(--fg-3)");
  });
});
