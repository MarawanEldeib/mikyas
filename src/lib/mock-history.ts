// Browser-only mock of `get_history`: 14 days of simulated Claude use sampled every 15 minutes
// (bursts during waking hours, lighter weekends, the machine asleep on most nights, 5-hour
// windows that start on first use and reset five hours later, a fixed weekly reset), ending on
// the scenario's live values. The rows are aggregated the way crates/core/src/history.rs does.
// Deterministic for a given seed and load time; all data is synthetic.

import { DAY, HOUR, MIN } from "./format";
import { addLocalDays, startOfLocalDay } from "./history";
import type { HistoryData, HistoryDay, HistoryWindow, SparkPoint } from "./types";

export const MOCK_HISTORY_DAYS = 14;
const STEP = 15 * MIN;
const FIVE_H = 5 * HOUR;
const WEEK = 7 * DAY;
/** Same rules as history.rs: gap after 2 h without rows, 2-point drops, five-hour windows over
 *  after five hours without rows, 30-minute dedup. */
const MAX_CARRY = 2 * HOUR;
const RESET_DROP = 2;
const RESET_DEDUP = 30 * MIN;

export interface MockWindowSpec {
  kind: "five_hour" | "seven_day";
  /** Live value the series ends on. */
  pct: number;
  /** Reset instant relative to `t0` (negative = already passed); null = unknown. */
  resetIn: number | null;
  /** Integer values, as Claude Desktop reports them. */
  integers?: boolean;
  /** No rows during the last this many ms (stale data). */
  silentFor?: number;
}

export interface MockHistorySpec {
  windows: MockWindowSpec[];
  t0: number;
  now: number;
  days: number;
  seed: number;
}

interface Row {
  t: number;
  p: number;
  /** Exact reset time of the row's window instance, if known. */
  r: number | null;
}

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Next local full hour after `t`. */
function nextLocalHour(t: number): number {
  const d = new Date(t);
  d.setMinutes(0, 0, 0);
  return d.getTime() + HOUR;
}

/** Simulates both windows over [start, now]; returns their rows. */
function simulate(spec: MockHistorySpec, start: number): { five: Row[]; week: Row[] } {
  const rand = mulberry32(spec.seed);
  const fiveSpec = spec.windows.find((w) => w.kind === "five_hour");
  const weekSpec = spec.windows.find((w) => w.kind === "seven_day");
  // The live 5-hour window started five hours before its reset; nothing may start in the five
  // hours before that, so the previous window has expired by then.
  const curFiveStart = fiveSpec?.resetIn != null ? spec.t0 + fiveSpec.resetIn - FIVE_H : null;
  const weeklyAnchor = weekSpec?.resetIn != null ? spec.t0 + weekSpec.resetIn : null;
  const nextWeekly = (t: number) => (weeklyAnchor === null ? null : weeklyAnchor - Math.floor((weeklyAnchor - t) / WEEK) * WEEK);

  const dayMood = new Map<number, { intensity: number; sleeps: boolean }>();
  const mood = (t: number) => {
    const day = startOfLocalDay(t);
    let m = dayMood.get(day);
    if (!m) {
      const dow = new Date(day).getDay();
      const weekend = dow === 0 || dow === 6;
      m = { intensity: weekend ? 0.3 + rand() * 0.35 : 0.7 + rand() * 0.6, sleeps: rand() < 0.65 };
      dayMood.set(day, m);
    }
    return m;
  };

  const five: Row[] = [];
  const week: Row[] = [];
  let fiveStart: number | null = null;
  let fivePct = 0;
  let weekPct = 0;
  let weekReset = nextWeekly(start);
  for (let t = start; t <= spec.now; t += STEP) {
    if (weekReset !== null && t >= weekReset) {
      weekPct = 0;
      weekReset = nextWeekly(t);
    }
    if (fiveStart !== null && t >= fiveStart + FIVE_H) {
      fiveStart = null;
      fivePct = 0;
    }
    const m = mood(t);
    const hour = new Date(t).getHours();
    const forced = curFiveStart !== null && t >= curFiveStart && t < curFiveStart + STEP;
    const blocked = curFiveStart !== null && t < curFiveStart && t >= curFiveStart - FIVE_H;
    // The snapshot shows the user active right now, whatever the hour.
    const recent = t > spec.t0 - 6 * HOUR;
    const awake = recent || (hour >= 9 && hour <= 23);
    if (forced || (!blocked && rand() < (awake ? 0.45 * m.intensity : 0.02))) {
      fiveStart ??= forced && curFiveStart !== null ? curFiveStart : t;
      const inc = (1 + rand() * 5) * m.intensity;
      fivePct = Math.min(100, fivePct + inc);
      weekPct = Math.min(100, weekPct + inc * 0.1);
    }
    const asleep = m.sleeps && !recent && hour >= 1 && hour < 8;
    if (asleep) continue;
    if (fiveSpec && !(fiveSpec.silentFor && t > spec.now - fiveSpec.silentFor)) {
      five.push({ t, p: fivePct, r: fiveStart === null ? null : fiveStart + FIVE_H });
    }
    if (weekSpec && !(weekSpec.silentFor && t > spec.now - weekSpec.silentFor)) {
      week.push({ t, p: weekPct, r: weekReset });
    }
  }
  return { five, week };
}

/** Rescales the rows of the latest window instance so the series ends on `pct`. */
function endOn(rows: Row[], pct: number): void {
  if (rows.length === 0) return;
  let first = rows.length - 1;
  while (first > 0 && rows[first - 1].p <= rows[first].p && rows[first - 1].r === rows[first].r) first--;
  const end = rows[rows.length - 1].p;
  const cur = rows.slice(first);
  cur.forEach((row, i) => {
    row.p = end > 0 ? (row.p / end) * pct : (pct * (i + 1)) / cur.length;
  });
}

/** The core's aggregation (history.rs `History::view`) over `rows`. */
export function aggregate(kind: string, rows: readonly Row[], from: number, to: number, now: number): HistoryWindow {
  const points: SparkPoint[] = [];
  let next = 0;
  let last: Row | null = null;
  while (next < rows.length && rows[next].t < from) last = rows[next++];
  for (let start = from; start < to; start += HOUR) {
    const end = Math.min(start + HOUR, to);
    let max: number | null = null;
    while (next < rows.length && (rows[next].t < end || (end === to && rows[next].t <= to))) {
      max = Math.max(max ?? 0, rows[next].p);
      last = rows[next++];
    }
    const carried = last !== null && start - last.t <= MAX_CARRY ? last.p : null;
    points.push({ t_ms: start, pct: max ?? carried });
  }

  const dayStarts: number[] = [];
  for (let d = startOfLocalDay(from); d < to; d = addLocalDays(d, 1)) dayStarts.push(d);
  const days: HistoryDay[] = dayStarts.map((day_start_ms) => ({ day_start_ms, peak_pct: 0, consumed_pct: 0, samples: 0 }));
  const passed = Math.min(to, now);
  const marks: number[] = [];
  let known: number | null = null;
  let prev: Row | null = null;
  let high = 0;
  for (const row of rows) {
    if (row.t > to) break;
    const exactReset = prev !== null && known !== null && prev.t < known && known <= row.t;
    const dropped = prev !== null && prev.p - row.p >= RESET_DROP;
    const expired = kind === "five_hour" && prev !== null && row.t - prev.t > FIVE_H ? prev : null;
    if (!exactReset) {
      const mark = expired ? (expired.p > 0 ? Math.min(expired.t + FIVE_H, row.t) : null) : dropped ? row.t : null;
      if (mark !== null && mark >= from && mark <= to) marks.push(mark);
    }
    const fresh = prev === null || exactReset || dropped || expired !== null;
    const consumed = prev === null ? 0 : fresh ? row.p : Math.max(0, row.p - high);
    high = fresh ? row.p : Math.max(high, row.p);
    let i = dayStarts.length - 1;
    while (i >= 0 && dayStarts[i] > row.t) i--;
    if (i >= 0) {
      days[i].consumed_pct += consumed;
      days[i].peak_pct = Math.max(days[i].peak_pct, row.p);
      days[i].samples++;
    }
    if (row.r !== null) {
      if (known !== null && known !== row.r && known <= row.t && known >= from && known <= passed) marks.push(known);
      known = row.r;
    }
    prev = row;
  }
  if (known !== null && known >= from && known <= passed) marks.push(known);
  marks.sort((a, b) => a - b);
  const resets_ms: number[] = [];
  for (const t of marks) if (resets_ms.length === 0 || t - resets_ms[resets_ms.length - 1] > RESET_DEDUP) resets_ms.push(t);
  for (const d of days) d.consumed_pct = Math.round(d.consumed_pct * 10) / 10;
  return { kind, points, resets_ms, days };
}

/** A `get_history` result for the scenario windows. */
export function mockHistory(spec: MockHistorySpec): HistoryData {
  const days = Math.min(MOCK_HISTORY_DAYS, Math.max(1, Math.round(Number.isFinite(spec.days) ? spec.days : MOCK_HISTORY_DAYS)));
  const to = nextLocalHour(spec.now);
  const from = to - days * DAY;
  // Anchored to the load time, so later calls extend the same simulated past.
  const start = Math.floor((spec.t0 - (MOCK_HISTORY_DAYS + 1) * DAY) / STEP) * STEP;
  const rows = simulate(spec, start);
  const windows: HistoryWindow[] = [];
  for (const w of spec.windows) {
    const series = w.kind === "five_hour" ? rows.five : rows.week;
    // A window that just reset (0%) already ends on 0: its last instance expired.
    if (w.pct > 0) endOn(series, w.pct);
    for (const row of series) row.p = w.integers ? Math.round(row.p) : Math.round(row.p * 10) / 10;
    windows.push(aggregate(w.kind, series, from, to, spec.now));
  }
  windows.sort((a, b) => (a.kind === "five_hour" ? -1 : b.kind === "five_hour" ? 1 : 0));
  return { from_ms: from, to_ms: to, windows, max_days: MOCK_HISTORY_DAYS };
}
