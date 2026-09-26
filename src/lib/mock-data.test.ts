import { afterEach, describe, expect, it, vi } from "vitest";
import { DAY, HOUR, MIN } from "./format";
import { startOfLocalDay } from "./history";
import { SCENARIOS, buildSnapshot, createMockBackend } from "./mock";
import { aggregate, mockHistory, type MockHistorySpec } from "./mock-history";
import type { HistoryData } from "./types";

const T0 = new Date(2026, 8, 24, 12, 0).getTime();

describe("mock sessions", () => {
  it("lists several sessions in the normal and high scenarios", () => {
    for (const [scenario, count] of [
      ["normal", 4],
      ["high", 3],
    ] as const) {
      const s = buildSnapshot(scenario, T0, T0);
      expect(s.sessions).toHaveLength(count);
      expect(s.sessions[0]).toEqual(s.session);
      const keys = s.sessions.map((x) => x.key);
      expect(new Set(keys).size).toBe(count);
      for (let i = 1; i < count; i++) {
        expect(s.sessions[i - 1].last_active_ms).toBeGreaterThanOrEqual(s.sessions[i].last_active_ms);
      }
      const concurrent = new Set(s.sessions.map((x) => x.concurrent));
      expect(concurrent.size).toBe(1);
    }
    const kinds = new Set(buildSnapshot("normal", T0, T0).sessions.map((s) => s.entrypoint));
    expect(kinds).toEqual(new Set(["cli", "desktop", "cowork"]));
  });

  it.each(SCENARIOS)("keeps the header session listed in %s", (scenario) => {
    const s = buildSnapshot(scenario, T0, T0);
    if (s.session) expect(s.sessions.find((x) => x.key === s.session?.key)).toEqual(s.session);
    else expect(s.sessions).toEqual([]);
  });
});

const spec = (over: Partial<MockHistorySpec> = {}): MockHistorySpec => ({
  windows: [
    { kind: "seven_day", pct: 59, resetIn: 2 * DAY + 4 * HOUR },
    { kind: "five_hour", pct: 29, resetIn: 3 * HOUR + 12 * MIN },
  ],
  t0: T0,
  now: T0,
  days: 14,
  seed: 7,
  ...over,
});

const lastValue = (data: HistoryData, kind: string) =>
  [...(data.windows.find((w) => w.kind === kind)?.points ?? [])].reverse().find((p) => p.pct !== null)?.pct;

describe("mockHistory", () => {
  it("produces 14 days of hourly points ending on the live values", () => {
    const data = mockHistory(spec());
    expect(data.windows.map((w) => w.kind)).toEqual(["five_hour", "seven_day"]);
    expect(data.to_ms - data.from_ms).toBe(14 * DAY);
    expect(data.to_ms).toBeGreaterThan(T0);
    expect(new Date(data.to_ms).getMinutes()).toBe(0);
    for (const w of data.windows) {
      expect(w.points).toHaveLength(14 * 24);
      expect(w.points.some((p) => p.pct === null)).toBe(true);
      for (const p of w.points) if (p.pct !== null) expect(p.pct).toBeGreaterThanOrEqual(0);
      for (const p of w.points) if (p.pct !== null) expect(p.pct).toBeLessThanOrEqual(100);
      expect([...w.resets_ms].sort((a, b) => a - b)).toEqual(w.resets_ms);
      for (const t of w.resets_ms) {
        expect(t).toBeGreaterThanOrEqual(data.from_ms);
        expect(t).toBeLessThanOrEqual(T0);
      }
      expect(w.days.map((d) => d.day_start_ms)).toEqual(w.days.map((d) => startOfLocalDay(d.day_start_ms)));
      for (const d of w.days) expect(d.consumed_pct).toBeGreaterThanOrEqual(0);
    }
    expect(lastValue(data, "five_hour")).toBeCloseTo(29, 0);
    expect(lastValue(data, "seven_day")).toBeCloseTo(59, 0);
    const [five, week] = data.windows;
    expect(five.resets_ms.length).toBeGreaterThan(5);
    expect(week.resets_ms).toHaveLength(2);
    expect(week.days.some((d) => d.consumed_pct > 0)).toBe(true);
  });

  it("is deterministic and slices by days", () => {
    expect(mockHistory(spec())).toEqual(mockHistory(spec()));
    const one = mockHistory(spec({ days: 1 }));
    expect(one.windows[0].points).toHaveLength(24);
    expect(mockHistory(spec({ days: 99 })).windows[0].points).toHaveLength(14 * 24);
    // A later call extends the same simulated past.
    const later = mockHistory(spec({ now: T0 + 10 * MIN }));
    expect(later.windows[1].resets_ms).toEqual(mockHistory(spec()).windows[1].resets_ms);
  });

  it("covers stale, integer and empty scenarios", () => {
    const stale = mockHistory(spec({ windows: [{ kind: "five_hour", pct: 41, resetIn: HOUR, silentFor: 3 * HOUR }] }));
    // The last row is 3 h old: carried for 2 h, then a gap.
    expect(stale.windows[0].points.at(-1)?.pct).toBeNull();
    expect(stale.windows[0].points.at(-2)?.pct).not.toBeNull();
    expect(lastValue(stale, "five_hour")).toBeCloseTo(41, 0);
    const desktop = mockHistory(spec({ windows: [{ kind: "seven_day", pct: 61, resetIn: 4 * DAY, integers: true }] }));
    expect(desktop.windows[0].points.every((p) => p.pct === null || Number.isInteger(p.pct))).toBe(true);
    expect(mockHistory(spec({ windows: [] })).windows).toEqual([]);
  });

  it("aggregates rows like the core", () => {
    const d0 = startOfLocalDay(T0);
    const rows = [
      { t: d0 + HOUR, p: 10, r: null },
      { t: d0 + 2 * HOUR, p: 50, r: null },
      { t: d0 + 3 * HOUR, p: 49.5, r: null },
      { t: d0 + 4 * HOUR, p: 5, r: null },
      { t: d0 + 5 * HOUR, p: 30, r: null },
    ];
    const w = aggregate("five_hour", rows, d0, d0 + 6 * HOUR, d0 + 6 * HOUR);
    expect(w.points.map((p) => p.pct)).toEqual([null, 10, 50, 49.5, 5, 30]);
    expect(w.resets_ms).toEqual([d0 + 4 * HOUR]);
    expect(w.days).toEqual([{ day_start_ms: d0, peak_pct: 50, consumed_pct: 70 }]);
  });
});

describe("mock get_history", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("answers for the scenario's windows", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    const b = createMockBackend(new URLSearchParams("scenario=high"));
    const pending = b.invoke<HistoryData>("get_history", { days: 14 });
    await vi.advanceTimersByTimeAsync(500);
    const data = await pending;
    expect(data.windows.map((w) => w.kind)).toEqual(["five_hour", "seven_day"]);
    expect(lastValue(data, "five_hour")).toBeCloseTo(83, 0);

    const empty = createMockBackend(new URLSearchParams("scenario=onboarding"));
    const none = empty.invoke<HistoryData>("get_history", { days: 14 });
    await vi.advanceTimersByTimeAsync(500);
    expect((await none).windows).toEqual([]);
  });
});
