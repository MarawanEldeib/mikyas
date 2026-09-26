// Which usage windows the compact views show, and their bar colour.

import { fillColor } from "./color";
import type { WindowView } from "./types";

const MAIN = ["five_hour", "seven_day"] as const;

/** The 5-hour and weekly windows (in that order), or the first two when neither exists. */
export function mainWindows<T extends { kind: string }>(ws: readonly T[]): T[] {
  const main = MAIN.map((k) => ws.find((w) => w.kind === k)).filter((w): w is T => w !== undefined);
  return main.length ? main : ws.slice(0, 2);
}

/** Fill colour of a window; neutral while stale or waiting for the first reading after a reset. */
export function mutedColor(w: Pick<WindowView, "pct" | "stale" | "phase">): string {
  return w.stale || w.phase === "reset_awaiting_data" ? "var(--fg-3)" : fillColor(w.pct);
}
