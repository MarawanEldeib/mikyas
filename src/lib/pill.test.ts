import { describe, expect, it } from "vitest";
import { HOUR, MIN } from "./format";
import { describeWindow, resetPhrase } from "./pill";
import { WORKED_SINCE_TIP } from "./worked";
import type { WindowView } from "./types";

// Thursday 2026-09-24 12:00 UTC.
const NOW = Date.UTC(2026, 8, 24, 12, 0, 0);

const win = (over: Partial<WindowView> = {}): WindowView => ({
  kind: "five_hour",
  pct: 42,
  reset: { type: "exact", at_ms: NOW + 3 * HOUR + 12 * MIN },
  source: "cli",
  observed_at_ms: NOW - MIN,
  stale: false,
  limit_reached: false,
  phase: "active",
  burn: null,
  spark: [],
  worked_since: false,
  ...over,
});

describe("resetPhrase", () => {
  it("counts down to a known reset", () => {
    expect(resetPhrase(win(), NOW)).toBe("resets in 3h 12m");
  });
  it("marks an estimated reset", () => {
    const reset = { type: "estimated", at_ms: NOW + 3 * HOUR + 12 * MIN, plus_minus_ms: 10 * MIN, confidence: "low" } as const;
    expect(resetPhrase(win({ reset }), NOW)).toBe("resets in ~3h 12m");
  });
  it("says the window reset and is waiting, not 'resets in reset'", () => {
    expect(resetPhrase(win({ phase: "reset_awaiting_data" }), NOW)).toBe("reset now, waiting for new data");
    // The reset instant passed on the UI clock before Rust reported it.
    expect(resetPhrase(win({ reset: { type: "exact", at_ms: NOW - MIN } }), NOW)).toBe("reset now, waiting for new data");
  });
  it("says the reset time is unknown, not 'resets in —'", () => {
    expect(resetPhrase(win({ reset: { type: "unknown" } }), NOW)).toBe("reset time unknown");
  });
});

describe("describeWindow", () => {
  it("reads usage, then the reset", () => {
    expect(describeWindow(win(), NOW)).toBe("5-hour limit 42% used, resets in 3h 12m");
  });
  it("uses the name Rust sends with the window", () => {
    expect(describeWindow(win({ kind: "thirty_day", label: "30-day" }), NOW)).toMatch(/^30-day limit 42% used/);
  });
  it("never reads 'resets in reset' or 'resets in —'", () => {
    const awaiting = describeWindow(win({ pct: 0, phase: "reset_awaiting_data" }), NOW);
    expect(awaiting).toBe("5-hour limit 0% used, reset now, waiting for new data");
    const unknown = describeWindow(win({ reset: { type: "unknown" } }), NOW);
    expect(unknown).toBe("5-hour limit 42% used, reset time unknown");
    for (const s of [awaiting, unknown]) expect(s).not.toMatch(/resets in (reset|—)/);
  });
  it("adds a reached limit, stale age and worked-since note", () => {
    const s = describeWindow(win({ limit_reached: true, stale: true, observed_at_ms: NOW - 2 * HOUR }), NOW);
    expect(s).toMatch(/^5-hour limit 42% used, limit reached, resets in 3h 12m, last updated /);
    expect(describeWindow(win({ worked_since: true }), NOW)).toContain(WORKED_SINCE_TIP);
  });
});
