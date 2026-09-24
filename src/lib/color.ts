// Usage-level colour bands. These are fixed display bands (green < 40, orange 40–69,
// red >= 70); the user's alert thresholds in Settings are a separate concept.

export type Level = "ok" | "warn" | "crit";

/** Lower bound (inclusive) of the orange band. */
export const WARN_AT = 40;
/** Lower bound (inclusive) of the red band. */
export const CRIT_AT = 70;

/** Colour band for a usage percentage. Non-finite input is treated as 0. */
export function level(pct: number): Level {
  const p = Number.isFinite(pct) ? pct : 0;
  if (p >= CRIT_AT) return "crit";
  if (p >= WARN_AT) return "warn";
  return "ok";
}

/** CSS colour for text drawn in a band's colour (AA on the widget surface). */
export function textColor(pct: number): string {
  return `var(--${level(pct)})`;
}

/** CSS colour for graphics (rings, bars, sparklines) in a band's colour. */
export function fillColor(pct: number): string {
  return `var(--${level(pct)}-fill)`;
}

/** Clamps a percentage into 0..=100 for display; non-finite input becomes 0. */
export function clampPct(pct: number): number {
  if (!Number.isFinite(pct)) return 0;
  return Math.min(100, Math.max(0, pct));
}
