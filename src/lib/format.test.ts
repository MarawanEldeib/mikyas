import { describe, expect, it } from "vitest";
import {
  DAY,
  HOUR,
  MIN,
  SEC,
  burnText,
  ctxLabel,
  estimateTooltip,
  formatAge,
  formatAgeShort,
  formatClock,
  formatCountdown,
  formatPct,
  formatSpan,
  formatTime,
  formatTokens,
  liveWindow,
  modelLabel,
  pillCountdown,
  resetLine,
  sourceStale,
  splitUnits,
  windowLabel,
  windowShort,
} from "./format";
import type { Burn, ResetInfo, WindowView } from "./types";

// Thursday 2026-09-24 12:00 UTC.
const NOW = Date.UTC(2026, 8, 24, 12, 0, 0);
const GB = { locale: "en-GB", timeZone: "UTC" };
const US = { locale: "en-US", timeZone: "UTC" };

describe("formatCountdown", () => {
  it("uses d/h at a day or more", () => {
    expect(formatCountdown(2 * DAY + 4 * HOUR + 59 * MIN)).toBe("2d 4h");
    expect(formatCountdown(DAY)).toBe("1d 0h");
  });
  it("uses h/m from an hour", () => {
    expect(formatCountdown(3 * HOUR + 12 * MIN + 59 * SEC)).toBe("3h 12m");
    expect(formatCountdown(DAY - 1)).toBe("23h 59m");
    expect(formatCountdown(HOUR)).toBe("1h 0m");
  });
  it("uses minutes from ten minutes", () => {
    expect(formatCountdown(42 * MIN + 30 * SEC)).toBe("42m");
    expect(formatCountdown(HOUR - 1)).toBe("59m");
    expect(formatCountdown(10 * MIN)).toBe("10m");
  });
  it("uses m:ss below ten minutes", () => {
    expect(formatCountdown(10 * MIN - 1)).toBe("9:59");
    expect(formatCountdown(9 * MIN + 5 * SEC)).toBe("9:05");
    expect(formatCountdown(59 * SEC)).toBe("0:59");
    expect(formatCountdown(999)).toBe("0:00");
  });
  it("never goes negative or crashes", () => {
    expect(formatCountdown(0)).toBe("0:00");
    expect(formatCountdown(-5 * MIN)).toBe("0:00");
    expect(formatCountdown(Number.NaN)).toBe("0:00");
  });
});

describe("formatSpan", () => {
  it("formats static durations", () => {
    expect(formatSpan(HOUR + 20 * MIN)).toBe("1h 20m");
    expect(formatSpan(5 * MIN)).toBe("5m");
    expect(formatSpan(30 * SEC)).toBe("<1m");
    expect(formatSpan(3 * DAY + 2 * HOUR)).toBe("3d 2h");
  });
  it("drops a zero minor unit", () => {
    expect(formatSpan(12 * HOUR)).toBe("12h");
    expect(formatSpan(12 * HOUR + 30 * SEC)).toBe("12h");
    expect(formatSpan(2 * DAY + 20 * MIN)).toBe("2d");
    expect(formatSpan(HOUR + MIN)).toBe("1h 1m");
  });
});

describe("liveWindow", () => {
  const w: WindowView = {
    kind: "five_hour",
    pct: 100,
    reset: { type: "exact", at_ms: NOW + 5 * SEC },
    source: "cli",
    observed_at_ms: NOW - 20 * MIN,
    stale: true,
    limit_reached: true,
    phase: "active",
    burn: { slope_pct_per_h: 3, t100_ms: null, pct_at_reset: 110, hits_limit_before_reset: false },
    spark: [],
  };
  it("returns the window unchanged before its reset", () => {
    expect(liveWindow(w, NOW)).toBe(w);
  });
  it("shows a passed reset as 0% awaiting data, as Rust does", () => {
    const live = liveWindow(w, NOW + 5 * SEC);
    expect(live).toMatchObject({ pct: 0, limit_reached: false, stale: false, burn: null, phase: "reset_awaiting_data" });
    expect(live.spark).toBe(w.spark);
  });
  it("also applies to passed estimated resets", () => {
    const est: WindowView = { ...w, reset: { type: "estimated", at_ms: NOW - MIN, plus_minus_ms: 10 * MIN, confidence: "low" } };
    expect(liveWindow(est, NOW).pct).toBe(0);
  });
  it("keeps windows Rust already marked as awaiting", () => {
    const awaiting: WindowView = { ...w, pct: 0, limit_reached: false, stale: false, burn: null, phase: "reset_awaiting_data" };
    expect(liveWindow(awaiting, NOW)).toBe(awaiting);
  });
});

describe("sourceStale", () => {
  it("flips exactly when the age reaches the stale threshold", () => {
    expect(sourceStale(NOW - 15 * MIN + 1, NOW, 15)).toBe(false);
    expect(sourceStale(NOW - 15 * MIN, NOW, 15)).toBe(true);
  });
});

describe("ages", () => {
  it("formats relative ages", () => {
    expect(formatAge(NOW - 20 * SEC, NOW)).toBe("just now");
    expect(formatAge(NOW - 2 * MIN - 59 * SEC, NOW)).toBe("2m ago");
    expect(formatAge(NOW - 3 * HOUR, NOW)).toBe("3h ago");
    expect(formatAge(NOW - 2 * DAY - HOUR, NOW)).toBe("2d ago");
  });
  it("formats compact badge ages", () => {
    expect(formatAgeShort(NOW - 9 * MIN, NOW)).toBe("9m");
    expect(formatAgeShort(NOW + 5 * MIN, NOW)).toBe("now");
  });
});

describe("clock", () => {
  it("follows the locale's 24h / 12h convention", () => {
    const at = Date.UTC(2026, 8, 24, 15, 40);
    expect(formatTime(at, GB)).toBe("15:40");
    expect(formatTime(at, US)).toMatch(/^3:40\sPM$/u);
  });
  it("shows only the time on the same day", () => {
    expect(formatClock(Date.UTC(2026, 8, 24, 15, 40), NOW, GB)).toBe("15:40");
  });
  it("adds the weekday within the coming week", () => {
    expect(formatClock(Date.UTC(2026, 8, 26, 15, 40), NOW, GB)).toBe("Sat 15:40");
    expect(formatClock(Date.UTC(2026, 8, 25, 0, 30), NOW, GB)).toBe("Fri 00:30");
  });
  it("adds the date beyond a week", () => {
    expect(formatClock(Date.UTC(2026, 9, 12, 9, 5), NOW, GB)).toBe("12 Oct 09:05");
  });
  it("respects the time zone for day boundaries", () => {
    const tokyo = { locale: "en-GB", timeZone: "Asia/Tokyo" };
    // 12:00 UTC is 21:00 in Tokyo; 16:00 UTC is already the next day there.
    expect(formatClock(Date.UTC(2026, 8, 24, 16, 0), NOW, tokyo)).toBe("Fri 01:00");
  });
});

describe("labels", () => {
  it("formats percentages", () => {
    expect(formatPct(29.4)).toBe("29");
    expect(formatPct(99.6)).toBe("100");
    expect(formatPct(123)).toBe("100");
    expect(formatPct(-1)).toBe("0");
  });
  it("formats token counts", () => {
    expect(formatTokens(1_000_000)).toBe("1M");
    expect(formatTokens(200_000)).toBe("200K");
    expect(formatTokens(1_500_000)).toBe("1.5M");
    expect(formatTokens(512)).toBe("512");
    expect(formatTokens(0)).toBe("0");
  });
  it("builds the model chip label", () => {
    expect(modelLabel({ display_name: "Opus 5.5", model_id: "claude-opus-5-5", ctx_size: 1_000_000 })).toBe("Opus 5.5 · 1M");
    expect(modelLabel({ display_name: null, model_id: "claude-sonnet-5", ctx_size: 200_000 })).toBe("sonnet-5 · 200K");
    expect(modelLabel({ display_name: null, model_id: null, ctx_size: 200_000 })).toBe("Unknown model · 200K");
  });
  it("builds the context label", () => {
    expect(ctxLabel({ ctx_pct: 34.2, ctx_is_estimate: false })).toBe("ctx 34%");
    expect(ctxLabel({ ctx_pct: 34.2, ctx_is_estimate: true })).toBe("ctx ≈34%");
    expect(ctxLabel({ ctx_pct: null, ctx_is_estimate: false })).toBe("ctx —");
  });
  it("names windows", () => {
    expect(windowLabel("five_hour")).toBe("5-hour");
    expect(windowLabel("seven_day")).toBe("7-day");
    expect(windowLabel("seven_day_opus")).toBe("7-day opus");
    expect(windowShort("five_hour")).toBe("5h");
    expect(windowShort("seven_day")).toBe("7d");
  });
});

describe("reset lines", () => {
  const exact: ResetInfo = { type: "exact", at_ms: NOW + 3 * HOUR + 40 * MIN };
  const est: ResetInfo = { type: "estimated", at_ms: NOW + 3 * HOUR + 40 * MIN, plus_minus_ms: 25 * MIN, confidence: "medium" };

  it("formats exact resets", () => {
    expect(resetLine({ phase: "active", reset: exact }, NOW, GB)).toBe("resets 15:40 · in 3h 40m");
    expect(pillCountdown({ phase: "active", reset: exact }, NOW)).toBe("3h 40m");
  });
  it("marks estimated resets with ~", () => {
    expect(resetLine({ phase: "active", reset: est }, NOW, GB)).toBe("resets ~15:40 · in 3h 40m");
    expect(resetLine({ phase: "active", reset: est }, NOW, US)).toMatch(/^resets ~3:40\sPM · in 3h 40m$/u);
    expect(pillCountdown({ phase: "active", reset: est }, NOW)).toBe("~3h 40m");
    expect(estimateTooltip(est)).toBe("Estimated from Claude Desktop history, ±25m (medium confidence)");
    expect(estimateTooltip(exact)).toBeUndefined();
  });
  it("handles unknown and passed resets", () => {
    expect(resetLine({ phase: "active", reset: { type: "unknown" } }, NOW)).toBe("reset time unknown");
    expect(pillCountdown({ phase: "active", reset: { type: "unknown" } }, NOW)).toBe("—");
    expect(resetLine({ phase: "reset_awaiting_data", reset: exact }, NOW)).toBe("reset — waiting for data");
    expect(resetLine({ phase: "active", reset: { type: "exact", at_ms: NOW - 1 } }, NOW)).toBe("reset — waiting for data");
    expect(pillCountdown({ phase: "reset_awaiting_data", reset: exact }, NOW)).toBe("reset");
  });
});

describe("splitUnits", () => {
  it("separates numbers from unit letters", () => {
    expect(splitUnits("~3h 12m")).toEqual([
      { text: "~", unit: true },
      { text: "3", unit: false },
      { text: "h ", unit: true },
      { text: "12", unit: false },
      { text: "m", unit: true },
    ]);
    expect(splitUnits("9:05")).toEqual([{ text: "9:05", unit: false }]);
    expect(splitUnits("reset")).toEqual([{ text: "reset", unit: true }]);
    expect(splitUnits("")).toEqual([]);
  });
});

describe("burnText", () => {
  const reset: ResetInfo = { type: "exact", at_ms: NOW + 3 * HOUR };
  const burn = (b: Partial<Burn>): Burn => ({
    slope_pct_per_h: 10,
    t100_ms: null,
    pct_at_reset: null,
    hits_limit_before_reset: false,
    ...b,
  });

  it("is hidden without a forecast", () => {
    expect(burnText(null, reset, NOW)).toBeNull();
    expect(burnText(burn({}), reset, NOW)).toBeNull();
  });
  it("warns when the limit is hit before the reset", () => {
    const b = burn({ t100_ms: NOW + HOUR + 40 * MIN, hits_limit_before_reset: true, pct_at_reset: 140 });
    expect(burnText(b, reset, NOW, GB)).toEqual({ text: "At this pace 100% at 13:40 — 1h 20m before reset", tone: "warn" });
  });
  it("turns red when the limit is under an hour away", () => {
    const b = burn({ t100_ms: NOW + 30 * MIN, hits_limit_before_reset: true });
    expect(burnText(b, reset, NOW, GB)?.tone).toBe("crit");
  });
  it("omits the reset gap when the reset is unknown", () => {
    const b = burn({ t100_ms: NOW + 2 * HOUR, hits_limit_before_reset: false });
    expect(burnText(b, { type: "unknown" }, NOW, GB)).toEqual({ text: "At this pace 100% at 14:00", tone: "warn" });
  });
  it("shows the projected value otherwise, clamped", () => {
    expect(burnText(burn({ pct_at_reset: 71.6 }), reset, NOW)).toEqual({ text: "On pace for ~72% at reset", tone: "muted" });
    expect(burnText(burn({ pct_at_reset: 180 }), reset, NOW)?.text).toBe("On pace for ~100% at reset");
  });
});
