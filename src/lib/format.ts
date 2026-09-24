// Pure display formatting. Every function takes `now` explicitly so the tick scheduler
// (tick.ts) and the tests agree on exactly when a string changes.

import { clampPct } from "./color";
import type { Burn, ResetInfo, SessionView, WindowKind, WindowView } from "./types";

export const SEC = 1_000;
export const MIN = 60 * SEC;
export const HOUR = 60 * MIN;
export const DAY = 24 * HOUR;

/** Countdowns switch to a per-second "m:ss" display below this many ms. */
export const SECONDS_BELOW = 10 * MIN;

export interface ClockOptions {
  /** BCP 47 locale; defaults to the user's. */
  locale?: string;
  /** IANA zone; defaults to the system zone. */
  timeZone?: string;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/**
 * Remaining time until a deadline, floored to the displayed unit:
 * `>= 1d` "2d 4h", `>= 1h` "3h 12m", `>= 10m` "42m", `< 10m` "9:05". Zero or negative → "0:00".
 */
export function formatCountdown(remainingMs: number): string {
  if (!Number.isFinite(remainingMs) || remainingMs <= 0) return "0:00";
  const r = remainingMs;
  if (r < SECONDS_BELOW) {
    const s = Math.floor(r / SEC);
    return `${Math.floor(s / 60)}:${pad2(s % 60)}`;
  }
  if (r < HOUR) return `${Math.floor(r / MIN)}m`;
  if (r < DAY) return `${Math.floor(r / HOUR)}h ${Math.floor(r / MIN) % 60}m`;
  return `${Math.floor(r / DAY)}d ${Math.floor(r / HOUR) % 24}h`;
}

/**
 * A static duration (e.g. "1h 20m before reset"): `>= 1d` "2d 4h", `>= 1h` "1h 20m",
 * `>= 1m` "42m", below that "<1m".
 */
export function formatSpan(ms: number): string {
  if (!Number.isFinite(ms) || ms < MIN) return "<1m";
  if (ms < HOUR) return `${Math.floor(ms / MIN)}m`;
  if (ms < DAY) return `${Math.floor(ms / HOUR)}h ${Math.floor(ms / MIN) % 60}m`;
  return `${Math.floor(ms / DAY)}d ${Math.floor(ms / HOUR) % 24}h`;
}

/** Relative age: "just now", "2m ago", "3h ago", "2d ago". Future times read as "just now". */
export function formatAge(atMs: number, now: number): string {
  const short = formatAgeShort(atMs, now);
  return short === "now" ? "just now" : `${short} ago`;
}

/** Compact age for badges: "now", "2m", "3h", "2d". */
export function formatAgeShort(atMs: number, now: number): string {
  const a = now - atMs;
  if (!Number.isFinite(a) || a < MIN) return "now";
  if (a < HOUR) return `${Math.floor(a / MIN)}m`;
  if (a < DAY) return `${Math.floor(a / HOUR)}h`;
  return `${Math.floor(a / DAY)}d`;
}

const fmtCache = new Map<string, Intl.DateTimeFormat>();
function dtf(opts: ClockOptions, extra: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = JSON.stringify([opts.locale ?? "", opts.timeZone ?? "", extra]);
  let f = fmtCache.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(opts.locale, { ...extra, timeZone: opts.timeZone });
    fmtCache.set(key, f);
  }
  return f;
}

/** True when the locale writes times on a 12-hour clock. */
export function uses12h(opts: ClockOptions = {}): boolean {
  const ro = dtf(opts, { hour: "numeric" }).resolvedOptions();
  if (ro.hour12 !== undefined) return ro.hour12;
  return ro.hourCycle === "h11" || ro.hourCycle === "h12";
}

/** Wall-clock time in the user's locale convention (24h "09:05" or 12h "9:05 AM"). */
export function formatTime(atMs: number, opts: ClockOptions = {}): string {
  // 24-hour locales pad the hour ("09:05") as Windows' short time format does.
  const hour = uses12h(opts) ? "numeric" : "2-digit";
  return dtf(opts, { hour, minute: "2-digit" }).format(atMs);
}

function dayKey(ms: number, opts: ClockOptions): string {
  return dtf({ timeZone: opts.timeZone, locale: "en-CA" }, { year: "numeric", month: "2-digit", day: "2-digit" }).format(ms);
}

/**
 * Reset clock: time only when it falls on the same calendar day as `now`, otherwise
 * "Sat 15:40" within the coming week and "12 Oct 15:40" beyond that.
 */
export function formatClock(atMs: number, now: number, opts: ClockOptions = {}): string {
  if (!Number.isFinite(atMs)) return "—";
  const time = formatTime(atMs, opts);
  if (dayKey(atMs, opts) === dayKey(now, opts)) return time;
  if (Math.abs(atMs - now) < 6.5 * DAY) {
    return `${dtf(opts, { weekday: "short" }).format(atMs)} ${time}`;
  }
  return `${dtf(opts, { day: "numeric", month: "short" }).format(atMs)} ${time}`;
}

/** Rounded percentage for display, clamped to 0..=100. */
export function formatPct(pct: number): string {
  return String(Math.round(clampPct(pct)));
}

/** Token counts: 1_000_000 → "1M", 200_000 → "200K", 1_500_000 → "1.5M". */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  if (n >= 1_000_000) return `${trimZero(n / 1_000_000)}M`;
  if (n >= 1_000) return `${trimZero(n / 1_000)}K`;
  return String(Math.round(n));
}

function trimZero(v: number): string {
  return (Math.round(v * 10) / 10).toString();
}

/** "Opus 5.5" from the display name, falling back to a tidied model id. */
export function modelName(s: Pick<SessionView, "display_name" | "model_id">): string {
  if (s.display_name) return s.display_name;
  if (s.model_id) return s.model_id.replace(/^claude-/, "");
  return "Unknown model";
}

/** Model chip text, e.g. "Opus 5.5 · 1M". */
export function modelLabel(s: Pick<SessionView, "display_name" | "model_id" | "ctx_size">): string {
  return `${modelName(s)} · ${formatTokens(s.ctx_size)}`;
}

/** Context label: "ctx 34%", "ctx ≈34%" when estimated, "ctx —" when unknown. */
export function ctxLabel(s: Pick<SessionView, "ctx_pct" | "ctx_is_estimate">): string {
  if (s.ctx_pct === null || !Number.isFinite(s.ctx_pct)) return "ctx —";
  return `ctx ${s.ctx_is_estimate ? "≈" : ""}${formatPct(s.ctx_pct)}%`;
}

/** Card section title for a window kind. */
export function windowLabel(kind: WindowKind): string {
  switch (kind) {
    case "five_hour":
      return "5-hour";
    case "seven_day":
      return "7-day";
    default:
      return kind.replace(/^seven_day_/, "7-day ").replace(/_/g, " ");
  }
}

/** Compact label for a window kind: "5h", "7d". */
export function windowShort(kind: WindowKind): string {
  switch (kind) {
    case "five_hour":
      return "5h";
    case "seven_day":
      return "7d";
    default:
      return windowLabel(kind);
  }
}

/** Reset instant, if known. */
export function resetAt(reset: ResetInfo): number | null {
  return reset.type === "unknown" ? null : reset.at_ms;
}

/** True when the reset time is estimated (shown with "~"). */
export function isEstimated(reset: ResetInfo): boolean {
  return reset.type === "estimated";
}

/** True when the window is waiting for fresh data after its reset time passed. */
export function awaitingReset(w: Pick<WindowView, "phase" | "reset">, now: number): boolean {
  if (w.phase === "reset_awaiting_data") return true;
  const at = resetAt(w.reset);
  return at !== null && at <= now;
}

/** Pill countdown: "3h 12m", "~3h 12m" when estimated, "reset" after it, "—" when unknown. */
export function pillCountdown(w: Pick<WindowView, "phase" | "reset">, now: number): string {
  if (awaitingReset(w, now)) return "reset";
  const at = resetAt(w.reset);
  if (at === null) return "—";
  return `${isEstimated(w.reset) ? "~" : ""}${formatCountdown(at - now)}`;
}

/** A run of a display string: digits (`unit: false`) or the letters/marks between them. */
export interface TextRun {
  text: string;
  unit: boolean;
}

/** Splits "~3h 12m" into number and unit runs so units can be typeset smaller. */
export function splitUnits(s: string): TextRun[] {
  return Array.from(s.matchAll(/(\d[\d:]*)|(\D+)/g), (m) => ({ text: m[0], unit: m[1] === undefined }));
}

/** Card reset line: "resets 15:40 · in 3h 12m" (with "~" when estimated). */
export function resetLine(w: Pick<WindowView, "phase" | "reset">, now: number, opts: ClockOptions = {}): string {
  if (awaitingReset(w, now)) return "reset — waiting for data";
  const at = resetAt(w.reset);
  if (at === null) return "reset time unknown";
  const t = isEstimated(w.reset) ? "~" : "";
  return `resets ${t}${formatClock(at, now, opts)} · in ${t}${formatCountdown(at - now)}`;
}

/** Tooltip for an estimated reset, e.g. "Estimated from Claude Desktop history, ±25m (medium confidence)". */
export function estimateTooltip(reset: ResetInfo): string | undefined {
  if (reset.type !== "estimated") return undefined;
  return `Estimated from Claude Desktop history, ±${formatSpan(reset.plus_minus_ms)} (${reset.confidence} confidence)`;
}

export type BurnTone = "crit" | "warn" | "muted";

export interface BurnText {
  text: string;
  tone: BurnTone;
}

/**
 * Burn forecast line. Hitting the limit: "At this pace 100% at 15:40 — 1h 20m before reset"
 * (red when under an hour away, else orange). Otherwise "On pace for ~72% at reset" (muted).
 * `null` when there is nothing useful to say.
 */
export function burnText(burn: Burn | null, reset: ResetInfo, now: number, opts: ClockOptions = {}): BurnText | null {
  if (!burn) return null;
  const at = resetAt(reset);
  const t100 = burn.t100_ms;
  if (t100 !== null && Number.isFinite(t100) && (burn.hits_limit_before_reset || at === null)) {
    const tone: BurnTone = t100 - now < HOUR ? "crit" : "warn";
    const when = formatClock(t100, now, opts);
    if (at !== null && at > t100) {
      return { text: `At this pace 100% at ${when} — ${formatSpan(at - t100)} before reset`, tone };
    }
    return { text: `At this pace 100% at ${when}`, tone };
  }
  if (burn.pct_at_reset !== null && Number.isFinite(burn.pct_at_reset)) {
    const pct = Math.round(clampPct(burn.pct_at_reset));
    return { text: `On pace for ~${pct}% at reset`, tone: pct >= 100 ? "warn" : "muted" };
  }
  return null;
}
