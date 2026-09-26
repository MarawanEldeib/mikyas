// Options of Settings → Layout that need logic: the UI size presets and the optional card rows.

import type { CardRows } from "./types";

/** UI size presets; `Settings.ui_scale` itself may be any value in 0.85..1.3. */
export const UI_SIZES = [
  { value: 0.9, label: "Small" },
  { value: 1, label: "Normal" },
  { value: 1.15, label: "Large" },
  { value: 1.3, label: "Extra" },
] as const;

/** The preset closest to `scale` (a hand-edited 0.85 shows as Small); Normal when invalid. */
export function nearestUiSize(scale: number): number {
  if (!Number.isFinite(scale)) return 1;
  let best: number = UI_SIZES[0].value;
  for (const { value } of UI_SIZES) {
    if (Math.abs(value - scale) < Math.abs(best - scale)) best = value;
  }
  return best;
}

export const CARD_ROWS: readonly { key: keyof CardRows; label: string }[] = [
  { key: "sparklines", label: "Sparklines" },
  { key: "burn", label: "Burn forecast" },
  { key: "session", label: "Session header" },
  { key: "sources", label: "Data sources" },
];

/**
 * Card rows from a comma-separated list of the rows to show, e.g. "sparklines,sources"
 * (the browser mock's `?rows=`). Missing = every row; "" or "none" = none; unknown names are
 * ignored.
 */
export function parseCardRows(list: string | null): CardRows {
  const shown = new Set(list === null ? CARD_ROWS.map((r) => r.key) : list.split(",").map((s) => s.trim()));
  return {
    sparklines: shown.has("sparklines"),
    burn: shown.has("burn"),
    session: shown.has("session"),
    sources: shown.has("sources"),
  };
}
