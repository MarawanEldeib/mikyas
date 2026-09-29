// Which usage windows the compact views show, and their bar colour.

import { fillColor } from "./color";
import type { WindowView } from "./types";

/** The two main windows (Rust's `WindowKind::is_main`), in display order. */
export const MAIN_KINDS = ["five_hour", "seven_day"] as const;

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

/** The 5-hour and weekly windows (in that order), or the first two when neither exists. */
export function mainWindows<T extends { kind: string }>(ws: readonly T[]): T[] {
  const main = MAIN_KINDS.map((k) => ws.find((w) => w.kind === k)).filter((w): w is T => w !== undefined);
  return main.length ? main : ws.slice(0, 2);
}

/** Every window `mainWindows` leaves out (e.g. a weekly Opus limit), in snapshot order. */
export function extraWindows<T extends { kind: string }>(ws: readonly T[]): T[] {
  const main = mainWindows(ws);
  return ws.filter((w) => !main.includes(w));
}

/** Main windows first, then the rest (the history view charts them all). */
export function allWindows<T extends { kind: string }>(ws: readonly T[]): T[] {
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
