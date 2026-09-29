// Which usage windows the compact views show, and their bar colour.

import { fillColor } from "./color";
import { mainKinds } from "./span";
import type { WindowView } from "./types";

/** The windows a strip shows before any data arrived (placeholders only; the real main windows
 *  come from the data). */
export const PLACEHOLDER_KINDS = ["five_hour", "seven_day"] as const;

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

/**
 * The main windows the compact views show, in display order (shortest, then longest). Rust marks
 * them (`is_main`, chosen from the data by `main_kinds`); data without the marks goes through the
 * same rule here (span.ts `mainKinds`), so renamed or new keys fill both slots either way.
 */
export function mainWindows<T extends { kind: string; is_main?: boolean }>(ws: readonly T[]): T[] {
  if (ws.some((w) => w.is_main !== undefined)) return ws.filter((w) => w.is_main);
  return mainKinds(ws.map((w) => w.kind)).flatMap((k) => ws.find((w) => w.kind === k) ?? []);
}

/** Every window `mainWindows` leaves out (e.g. a weekly Opus limit), in snapshot order. */
export function extraWindows<T extends { kind: string; is_main?: boolean }>(ws: readonly T[]): T[] {
  const main = mainWindows(ws);
  return ws.filter((w) => !main.includes(w));
}

/** The window the History view draws per-day bars for: the longest main window (today the
 *  weekly one), `seven_day` when present. */
export function barWindow<T extends { kind: string; is_main?: boolean }>(ws: readonly T[]): T | null {
  return ws.find((w) => w.kind === "seven_day") ?? mainWindows(ws).at(-1) ?? null;
}

/** Main windows first, then the rest (the history view charts them all). */
export function allWindows<T extends { kind: string; is_main?: boolean }>(ws: readonly T[]): T[] {
  return [...mainWindows(ws), ...extraWindows(ws)];
}

/** How far back a window's sparkline reaches, in words: "24 hours", "7 days". Without Rust's
 *  `spark_span_ms` (older data) the span is read from the points, else a week is assumed. */
export function sparkSpanText(w: Pick<WindowView, "spark" | "spark_span_ms">): string {
  let ms = w.spark_span_ms;
  if (!ms || !Number.isFinite(ms)) {
    const n = w.spark.length;
    ms = n > 1 ? ((w.spark[n - 1].t_ms - w.spark[0].t_ms) * n) / (n - 1) : 7 * DAY;
  }
  if (ms >= 2 * DAY) {
    const days = Math.round(ms / DAY);
    return `${days} days`;
  }
  const hours = Math.max(1, Math.round(ms / HOUR));
  return hours === 1 ? "hour" : `${hours} hours`;
}

/** Fill colour of a window; neutral while stale or waiting for the first reading after a reset. */
export function mutedColor(w: Pick<WindowView, "pct" | "stale" | "phase">): string {
  return w.stale || w.phase === "reset_awaiting_data" ? "var(--fg-3)" : fillColor(w.pct);
}
