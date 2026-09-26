// Usage-level colour bands. These are fixed display bands (green < 40, orange 40–69,
// red >= 70); the user's alert thresholds in Settings are a separate concept. Also the chrome
// accent list and the WCAG contrast maths its palette is checked with.

import type { Accent } from "./types";

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

/** Chrome accents in Settings order; each has `--accent-<name>` tokens in app.css. */
export const ACCENTS = ["auto", "blue", "violet", "teal", "green", "amber", "rose"] as const satisfies readonly Accent[];

export const ACCENT_NAMES: Record<Accent, string> = {
  auto: "Default",
  blue: "Blue",
  violet: "Violet",
  teal: "Teal",
  green: "Green",
  amber: "Amber",
  rose: "Rose",
};

export type Rgb = readonly [number, number, number];

/** Parses "#rrggbb" or "r g b" (as in --surface-rgb); null when malformed. */
export function parseRgb(s: string): Rgb | null {
  const t = s.trim();
  const hex = /^#([0-9a-f]{6})$/i.exec(t);
  if (hex) {
    const n = Number.parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const parts = t.split(/\s+/).map(Number);
  return parts.length === 3 && parts.every((p) => Number.isInteger(p) && p >= 0 && p <= 255) ? [parts[0], parts[1], parts[2]] : null;
}

/** `top` at opacity `alpha` composited over an opaque `bottom`. */
export function over(top: Rgb, alpha: number, bottom: Rgb): Rgb {
  const mix = (i: number) => Math.round(top[i] * alpha + bottom[i] * (1 - alpha));
  return [mix(0), mix(1), mix(2)];
}

/** WCAG relative luminance. */
export function luminance(c: Rgb): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
}

/** WCAG contrast ratio, 1..21. */
export function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
