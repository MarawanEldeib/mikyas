import { describe, expect, it } from "vitest";
import { allWindows, barWindow, extraWindows, mainWindows, mutedColor, sparkSpanText } from "./windows";

const w = (kind: string) => ({ kind });

describe("mainWindows", () => {
  it("picks the 5-hour then the weekly window, whatever the order", () => {
    expect(mainWindows([w("seven_day_opus"), w("seven_day"), w("five_hour")])).toEqual([w("five_hour"), w("seven_day")]);
  });
  it("keeps a single main window alone", () => {
    expect(mainWindows([w("seven_day_opus"), w("seven_day")])).toEqual([w("seven_day")]);
  });
  it("chooses from the data: renamed or new keys fill both slots, scoped ones do not", () => {
    expect(mainWindows([w("five_hour"), w("seven_day_newmodel"), w("weekly")])).toEqual([w("five_hour"), w("weekly")]);
    expect(mainWindows([w("thirty_day"), w("4_hour"), w("seven_day_x")])).toEqual([w("4_hour"), w("thirty_day")]);
  });
  it("follows Rust's is_main marks when present", () => {
    const marked = [
      { kind: "session_limit", is_main: true },
      { kind: "five_hour", is_main: false },
    ];
    expect(mainWindows(marked)).toEqual([marked[0]]);
    expect(extraWindows(marked)).toEqual([marked[1]]);
  });
  it("falls back to the first two windows without a known length", () => {
    expect(mainWindows([w("a"), w("b"), w("c")])).toEqual([w("a"), w("b")]);
  });
  it("is empty without windows", () => {
    expect(mainWindows([])).toEqual([]);
  });
});

describe("extraWindows", () => {
  it("keeps every other window in snapshot order", () => {
    expect(extraWindows([w("five_hour"), w("seven_day"), w("seven_day_opus"), w("spend_limit")])).toEqual([
      w("seven_day_opus"),
      w("spend_limit"),
    ]);
    expect(extraWindows([w("seven_day_opus"), w("seven_day")])).toEqual([w("seven_day_opus")]);
  });
  it("leaves out the fallback pair without a main window", () => {
    expect(extraWindows([w("a"), w("b"), w("c")])).toEqual([w("c")]);
    expect(extraWindows([w("five_hour"), w("seven_day")])).toEqual([]);
    expect(extraWindows([])).toEqual([]);
  });
  it("orders all windows main first", () => {
    expect(allWindows([w("seven_day_opus"), w("seven_day"), w("five_hour")])).toEqual([
      w("five_hour"),
      w("seven_day"),
      w("seven_day_opus"),
    ]);
  });
});

describe("barWindow", () => {
  it("is the weekly window, else the longest main one", () => {
    expect(barWindow([w("five_hour"), w("seven_day"), w("thirty_day")])).toEqual(w("seven_day"));
    expect(barWindow([w("five_hour"), w("weekly_all"), w("thirty_day")])).toEqual(w("thirty_day"));
    expect(barWindow([w("five_hour"), w("1_week")])).toEqual(w("1_week"));
    expect(barWindow([])).toBeNull();
  });
});

describe("sparkSpanText", () => {
  const H = 3_600_000;
  const pts = (span: number) => Array.from({ length: 96 }, (_, i) => ({ t_ms: (i * span) / 96, pct: 1 }));
  it("names the span Rust sends", () => {
    expect(sparkSpanText({ spark: [], spark_span_ms: 24 * H })).toBe("24 hours");
    expect(sparkSpanText({ spark: [], spark_span_ms: 7 * 24 * H })).toBe("7 days");
    expect(sparkSpanText({ spark: [], spark_span_ms: H })).toBe("hour");
  });
  it("reads the span from the points, else assumes a week", () => {
    expect(sparkSpanText({ spark: pts(24 * H) })).toBe("24 hours");
    expect(sparkSpanText({ spark: pts(7 * 24 * H) })).toBe("7 days");
    expect(sparkSpanText({ spark: [] })).toBe("7 days");
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
