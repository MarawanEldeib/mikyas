// Pure geometry and text for the History view: fixed-domain step/area charts (one point per
// hour, broken at gaps), reset ticks, local time labels, the per-day bars and summary lines.
// The view fetches FETCH_DAYS once; each range is a slice of that data, and ranges longer than
// the history keeps (`max_days` from Rust) are hidden.

import { clampPct } from "./color";
import { DAY, HOUR, formatPct } from "./format";
import { dtf, type ClockOptions } from "./intl";
import { plural } from "./plural";
import { stepPath, valueOf, type StepPath } from "./step";
import type { HistoryDay, HistoryWindow, SparkPoint } from "./types";

export type RangeKey = "24h" | "7d" | "14d";

export interface RangeSpec {
  key: RangeKey;
  /** Segmented-control text. */
  label: string;
  /** Accessible name of the range. */
  name: string;
  span: number;
  /** How many day bars to show. */
  bars: number;
}

export const RANGES: readonly RangeSpec[] = [
  { key: "24h", label: "24h", name: "last 24 hours", span: DAY, bars: 7 },
  { key: "7d", label: "7d", name: "last 7 days", span: 7 * DAY, bars: 7 },
  { key: "14d", label: "14d", name: "last 14 days", span: 14 * DAY, bars: 14 },
];

/** Days of history the view asks for: its longest range (Rust clamps the request to what the
 *  history keeps and returns that as `max_days`). */
export const FETCH_DAYS = Math.max(...RANGES.map((r) => r.span)) / DAY;

/** The ranges the loaded history can fill: those no longer than `maxDays` (always the shortest,
 *  and every range while it is unknown). */
export function rangesFor(maxDays: number | undefined): readonly RangeSpec[] {
  if (maxDays === undefined || !Number.isFinite(maxDays)) return RANGES;
  const fit = RANGES.filter((r) => r.span <= maxDays * DAY);
  return fit.length > 0 ? fit : RANGES.slice(0, 1);
}
/** Horizontal grid lines of the charts. */
export const GRID_PCTS = [50, 80, 100] as const;

export interface Domain {
  from: number;
  to: number;
}

/** The last `span` ms of the loaded data. */
export function rangeDomain(data: { from_ms: number; to_ms: number }, span: number): Domain {
  return { from: Math.max(data.from_ms, data.to_ms - span), to: data.to_ms };
}

/** Plot area inside an SVG of `width` × `height`: insets on each side. */
export interface PlotBox {
  width: number;
  height: number;
  left: number;
  right: number;
  top: number;
  bottom: number;
}

const round = (v: number) => Math.round(v * 100) / 100;

/** X of an instant (clamped to the plot). */
export function plotX(t: number, d: Domain, box: PlotBox): number {
  const w = box.width - box.left - box.right;
  const f = d.to > d.from ? (t - d.from) / (d.to - d.from) : 0;
  return round(box.left + Math.min(1, Math.max(0, f)) * w);
}

/** Y of a percentage (0 at the bottom of the plot, 100 at its top; clamped). */
export function plotY(pct: number, box: PlotBox): number {
  const h = box.height - box.top - box.bottom;
  return round(box.top + (1 - clampPct(pct) / 100) * h);
}

export type ChartGeometry = StepPath;

/**
 * Step chart over a fixed domain: each point holds its value for `step` ms (clipped to the
 * domain). A null point, or a missing stretch longer than 1.5 steps, breaks the line.
 */
export function chartGeometry(points: readonly SparkPoint[], d: Domain, box: PlotBox, step = HOUR): ChartGeometry {
  const pts = points.filter((p) => Number.isFinite(p.t_ms) && p.t_ms < d.to && p.t_ms + step > d.from).sort((a, b) => a.t_ms - b.t_ms);
  return stepPath(
    pts,
    (t) => plotX(t, d, box),
    (pct) => plotY(pct, box),
    step,
  );
}

/** X positions of the resets inside the domain. */
export function resetXs(resets: readonly number[], d: Domain, box: PlotBox): number[] {
  return resets.filter((t) => t >= d.from && t <= d.to).map((t) => plotX(t, d, box));
}

/** Local midnight at or before `t`. */
export function startOfLocalDay(t: number): number {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** The local midnight `n` calendar days after the one at or before `t` (DST-safe). */
export function addLocalDays(t: number, n: number): number {
  const d = new Date(startOfLocalDay(t));
  d.setDate(d.getDate() + n);
  return d.getTime();
}

export interface TimeAxis {
  /** Vertical guide lines (local midnights inside the domain). */
  lines: number[];
  /** Labels, each centred on `t`. */
  labels: { t: number; text: string }[];
}

/** A day label needs at least this much of its day inside the domain. */
export const MIN_LABELLED_DAY = 10 * HOUR;
/** The 24h axis labels every this many local hours (00, 06, 12, 18). */
export const HOUR_LABEL_EVERY = 6;
/** Safety cap on the day-label walk (far more days than any range), so a broken clock or
 *  domain can never spin it for long. */
export const MAX_LABEL_DAYS = 400;

/**
 * X-axis for a range in the user's locale: every HOUR_LABEL_EVERY hours for 24h (the weekday at midnight),
 * one weekday per day for 7d, every other date for 14d (counted back from today). Day labels
 * are centred on the visible part of their day; days showing less than MIN_LABELLED_DAY are
 * not labelled.
 */
export function timeAxis(d: Domain, range: RangeKey, opts: ClockOptions = {}): TimeAxis {
  const lines: number[] = [];
  for (let m = addLocalDays(d.from, 1); m < d.to; m = addLocalDays(m, 1)) lines.push(m);
  const labels: TimeAxis["labels"] = [];
  if (range === "24h") {
    const hour = dtf(opts, { hour: "numeric" });
    const weekday = dtf(opts, { weekday: "short" });
    // Local hour boundaries (not UTC ones: half-hour zones exist).
    const first = new Date(d.from);
    first.setMinutes(0, 0, 0);
    for (let t = first.getTime() < d.from ? first.getTime() + HOUR : first.getTime(); t < d.to; t += HOUR) {
      const h = new Date(t).getHours();
      if (h % HOUR_LABEL_EVERY !== 0 || t - d.from < HOUR || d.to - t < HOUR) continue;
      labels.push({ t, text: h === 0 ? weekday.format(t) : hour.format(t) });
    }
    return { lines, labels };
  }
  const fmt = range === "7d" ? dtf(opts, { weekday: "short" }) : dtf(opts, { day: "numeric", month: "short" });
  let fromEnd = 0;
  for (let m = startOfLocalDay(d.to - 1); m + DAY > d.from && fromEnd <= MAX_LABEL_DAYS; m = addLocalDays(m, -1), fromEnd++) {
    const start = Math.max(m, d.from);
    const end = Math.min(addLocalDays(m, 1), d.to);
    if (end - start < MIN_LABELLED_DAY || (range === "14d" && fromEnd % 2 !== 0)) continue;
    labels.unshift({ t: (start + end) / 2, text: fmt.format(m + DAY / 2) });
  }
  return { lines, labels };
}

/** Least space between two axis labels (a few spaces, so two dates never read as one). */
export const LABEL_GAP = 8;

export interface PlacedLabel {
  t: number;
  text: string;
  /** Centre of the label. */
  x: number;
}

/**
 * Positions axis labels: centred on their instant, moved inside the plot where they would stick
 * out, and dropped where they would come closer than LABEL_GAP to a neighbour. Of two such labels
 * the one moved further from its instant (a partial day at an edge) gives way; on a tie the
 * earlier one stays. `measure` returns a label's width.
 */
export function placeLabels(
  labels: readonly { t: number; text: string }[],
  d: Domain,
  box: PlotBox,
  measure: (text: string) => number,
): PlacedLabel[] {
  const lo = box.left;
  const hi = box.width - box.right;
  const kept: { t: number; text: string; x: number; w: number; shift: number }[] = [];
  for (const l of [...labels].sort((a, b) => a.t - b.t)) {
    const w = measure(l.text);
    const at = plotX(l.t, d, box);
    const x = w >= hi - lo ? (lo + hi) / 2 : Math.min(hi - w / 2, Math.max(lo + w / 2, at));
    const next = { t: l.t, text: l.text, x, w, shift: Math.abs(x - at) };
    let keep = true;
    for (let prev = kept.at(-1); prev && next.x - next.w / 2 < prev.x + prev.w / 2 + LABEL_GAP; prev = kept.at(-1)) {
      if (next.shift >= prev.shift) {
        keep = false;
        break;
      }
      kept.pop();
    }
    if (keep) kept.push(next);
  }
  return kept.map(({ t, text, x }) => ({ t, text, x: round(x) }));
}

export interface DayBar {
  start: number;
  /** Share of the limit used that day (0..100). */
  value: number;
  peak: number;
  /** False for a day without any recorded row (unlike a day at 0%). */
  hasData: boolean;
  today: boolean;
  /** Short axis label ("Mon", or "M" when narrow). */
  label: string;
  /** Compact detail line, e.g. "Thu, Sep 24 · 21%" or "Thu, Sep 24 · no data". */
  detail: string;
  /** Accessible description. */
  full: string;
}

/** The last `count` days as bars; `today` is the day containing `now`. `limit` names the window
 *  the days belong to ("weekly", "30-day"). */
export function dayBars(days: readonly HistoryDay[], count: number, now: number, opts: ClockOptions = {}, limit = "weekly"): DayBar[] {
  const label = dtf(opts, { weekday: count > 7 ? "narrow" : "short" });
  const long = dtf(opts, { weekday: "short", day: "numeric", month: "short" });
  const shown = days.slice(Math.max(0, days.length - count));
  return shown.map((day, i) => {
    const next = shown[i + 1]?.day_start_ms ?? addLocalDays(day.day_start_ms, 1);
    const value = clampPct(day.consumed_pct);
    const peak = clampPct(day.peak_pct);
    const hasData = day.samples > 0;
    const amount = hasData ? `${formatPct(value)}%` : "no data";
    const used = hasData ? `${amount} of the ${limit} limit used` : amount;
    return {
      start: day.day_start_ms,
      value,
      peak,
      hasData,
      today: now >= day.day_start_ms && now < next,
      label: label.format(day.day_start_ms),
      detail: `${long.format(day.day_start_ms)} · ${amount}`,
      full: `${long.format(day.day_start_ms)}: ${used}${peak > 0 ? ` (peak ${formatPct(peak)}%)` : ""}`,
    };
  });
}

/** Top of the bar scale: the largest value rounded up to 5, at least 10. */
export function barScale(values: readonly number[]): number {
  const max = Math.max(0, ...values.filter(Number.isFinite));
  return Math.max(10, Math.ceil(max / 5) * 5);
}

export interface WindowSummary {
  /** Highest hourly value in the domain, or null without data. */
  peak: number | null;
  resets: number;
}

export function windowSummary(w: Pick<HistoryWindow, "points" | "resets_ms">, d: Domain): WindowSummary {
  let peak: number | null = null;
  for (const p of w.points) {
    const v = valueOf(p);
    if (v !== null && p.t_ms >= d.from && p.t_ms < d.to) peak = Math.max(peak ?? 0, clampPct(v));
  }
  return { peak, resets: w.resets_ms.filter((t) => t >= d.from && t <= d.to).length };
}

/** "no resets", "1 reset", "4 resets". */
export function resetsText(n: number): string {
  return n === 0 ? "no resets" : plural(n, "reset");
}

/** "5-hour peak 93% · 4 resets", or "No 5-hour data" without any. */
export function summaryText(label: string, s: WindowSummary): string {
  if (s.peak === null) return `No ${label} data`;
  return `${label} peak ${formatPct(s.peak)}% · ${resetsText(s.resets)}`;
}
