import { afterEach, describe, expect, it, vi } from "vitest";
import { DAY, HOUR, MIN, SEC, formatAge, formatCountdown } from "./format";
import { MAX_DELAY, MIN_DELAY, ageChangeIn, clampDelay, countdownChangeIn, createTicker, nextChangeDelay } from "./tick";

const NOW = Date.UTC(2026, 8, 24, 12, 0, 0);

describe("countdownChangeIn", () => {
  it("ticks every second under ten minutes", () => {
    expect(countdownChangeIn(NOW + 9 * MIN + 5 * SEC + 300, NOW)).toBe(301);
    expect(countdownChangeIn(NOW + 5 * SEC, NOW)).toBe(1);
  });
  it("ticks on minute boundaries from ten minutes to a day", () => {
    expect(countdownChangeIn(NOW + 3 * HOUR + 12 * MIN + 20 * SEC, NOW)).toBe(20 * SEC + 1);
    expect(countdownChangeIn(NOW + 42 * MIN, NOW)).toBe(1);
  });
  it("ticks on hour boundaries from a day", () => {
    expect(countdownChangeIn(NOW + 2 * DAY + 4 * HOUR + 30 * MIN, NOW)).toBe(30 * MIN + 1);
  });
  it("stops once the deadline passed", () => {
    expect(countdownChangeIn(NOW, NOW)).toBeNull();
    expect(countdownChangeIn(NOW - MIN, NOW)).toBeNull();
    expect(countdownChangeIn(Number.NaN, NOW)).toBeNull();
  });
  it("lands exactly on the next displayed change", () => {
    for (const r of [7 * SEC + 1, 10 * MIN, 10 * MIN + 500, HOUR + 30 * SEC, DAY, DAY + 3 * HOUR + 1, 3 * DAY + 17]) {
      const d = countdownChangeIn(NOW + r, NOW);
      expect(d).not.toBeNull();
      const before = formatCountdown(r - (d as number) + 1);
      const after = formatCountdown(r - (d as number));
      expect(before).toBe(formatCountdown(r));
      expect(after).not.toBe(before);
    }
  });
});

describe("ageChangeIn", () => {
  it("changes on the minute below an hour", () => {
    expect(ageChangeIn(NOW - 20 * SEC, NOW)).toBe(40 * SEC);
    expect(ageChangeIn(NOW - 2 * MIN - 15 * SEC, NOW)).toBe(45 * SEC);
  });
  it("changes on the hour below a day and daily beyond", () => {
    expect(ageChangeIn(NOW - 3 * HOUR - 10 * MIN, NOW)).toBe(50 * MIN);
    expect(ageChangeIn(NOW - 2 * DAY - HOUR, NOW)).toBe(23 * HOUR);
  });
  it("waits for future timestamps to become a minute old", () => {
    expect(ageChangeIn(NOW + 5 * SEC, NOW)).toBe(65 * SEC);
  });
  it("lands exactly on the next displayed change", () => {
    for (const a of [0, 59 * SEC, 61 * SEC, HOUR - 1, HOUR, 5 * HOUR + 1, DAY + 7]) {
      const d = ageChangeIn(NOW - a, NOW) as number;
      expect(formatAge(NOW - a, NOW + d - 1)).toBe(formatAge(NOW - a, NOW));
      expect(formatAge(NOW - a, NOW + d)).not.toBe(formatAge(NOW - a, NOW));
    }
  });
});

describe("nextChangeDelay", () => {
  it("takes the soonest change over all targets", () => {
    const d = nextChangeDelay(NOW, {
      deadlines: [NOW + 3 * HOUR + 30 * SEC, NOW + 2 * DAY + 10 * MIN],
      pasts: [NOW - 2 * MIN - 50 * SEC],
    });
    expect(d).toBe(10 * SEC);
  });
  it("is per-second only while a countdown is under ten minutes", () => {
    expect(nextChangeDelay(NOW, { deadlines: [NOW + 5 * MIN + 400], pasts: [] })).toBe(401);
    expect(nextChangeDelay(NOW, { deadlines: [NOW + 50 * MIN + 400], pasts: [] })).toBe(401);
    expect(nextChangeDelay(NOW, { deadlines: [NOW + 50 * MIN + 30 * SEC], pasts: [] })).toBe(30 * SEC + 1);
  });
  it("returns null when nothing will change", () => {
    expect(nextChangeDelay(NOW, { deadlines: [], pasts: [] })).toBeNull();
    expect(nextChangeDelay(NOW, { deadlines: [NOW - 1], pasts: [] })).toBeNull();
  });
  it("clamps delays", () => {
    expect(clampDelay(1)).toBe(MIN_DELAY);
    expect(clampDelay(30 * MIN)).toBe(MAX_DELAY);
    expect(clampDelay(1234.2)).toBe(1235);
  });
});

describe("createTicker", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  function fakeDoc() {
    const listeners = new Set<() => void>();
    return {
      visibilityState: "visible" as DocumentVisibilityState,
      addEventListener: (_: string, fn: () => void) => listeners.add(fn),
      removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
      fire() {
        for (const l of listeners) l();
      },
      listeners,
    };
  }

  it("fires at each change and recomputes from the clock", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    const ticks: number[] = [];
    const deadline = NOW + 10 * MIN + 2 * SEC;
    const t = createTicker(() => ({ deadlines: [deadline], pasts: [] }), (n) => ticks.push(n), {
      doc: null,
    });
    vi.advanceTimersByTime(2 * SEC);
    expect(ticks).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(ticks).toEqual([NOW + 2 * SEC + 1]);
    // Now under ten minutes: ticks every second.
    vi.advanceTimersByTime(3 * SEC);
    expect(ticks.length).toBe(4);
    t.stop();
    vi.advanceTimersByTime(MIN);
    expect(ticks.length).toBe(4);
  });

  it("pauses while hidden and ticks on becoming visible", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    const doc = fakeDoc();
    const ticks: number[] = [];
    const t = createTicker(() => ({ deadlines: [NOW + HOUR], pasts: [] }), (n) => ticks.push(n), {
      doc: doc as unknown as Document,
    });
    doc.visibilityState = "hidden";
    doc.fire();
    vi.advanceTimersByTime(10 * MIN);
    expect(ticks).toEqual([]);
    doc.visibilityState = "visible";
    doc.fire();
    expect(ticks).toEqual([NOW + 10 * MIN]);
    t.stop();
    expect(doc.listeners.size).toBe(0);
  });

  it("re-arms on refresh with new targets", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    let targets = { deadlines: [] as number[], pasts: [] as number[] };
    const ticks: number[] = [];
    const t = createTicker(() => targets, (n) => ticks.push(n), { doc: null });
    vi.advanceTimersByTime(5 * MIN);
    expect(ticks).toEqual([]);
    targets = { deadlines: [], pasts: [Date.now() - 30 * SEC] };
    t.refresh();
    vi.advanceTimersByTime(30 * SEC);
    expect(ticks.length).toBe(1);
    t.stop();
  });
});
