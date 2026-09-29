// Pure display formatting. Every function takes `now` explicitly so the tick scheduler
// (tick.ts) and the tests agree on exactly when a string changes.

import { clampPct } from "./color";
import { dtf, type ClockOptions } from "./intl";
import type { Burn, ResetInfo, SessionView, WindowKind, WindowView } from "./types";

export const SEC = 1_000;
export const MIN = 60 * SEC;
export const HOUR = 60 * MIN;
export const DAY = 24 * HOUR;

/** Countdowns switch to a per-second "m:ss" display below this many ms. */
export const SECONDS_BELOW = 10 * MIN;

/**
 * An estimated reset whose margin is at least this wide is shown by day only ("~Thu"): its
 * time of day carries no information. Not tied to a window kind — a tighter estimate shows its
 * clock again. Mirrors `ROUGH_RESET_PM_MS` in crates/core/src/time.rs (toasts).
 */
export const ROUGH_RESET_PM = 12 * HOUR;

export type { ClockOptions };

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
 * `>= 1m` "42m", below that "<1m". A zero minor unit is dropped ("12h", "2d").
 */
export function formatSpan(ms: number): string {
  if (!Number.isFinite(ms) || ms < MIN) return "<1m";
  if (ms < HOUR) return `${Math.floor(ms / MIN)}m`;
  const major = ms < DAY ? `${Math.floor(ms / HOUR)}h` : `${Math.floor(ms / DAY)}d`;
  const minor = ms < DAY ? Math.floor(ms / MIN) % 60 : Math.floor(ms / HOUR) % 24;
  // Unlike a ticking countdown, a static span reads better without a zero tail ("12h", not "12h 0m").
  return minor === 0 ? major : `${major} ${minor}${ms < DAY ? "m" : "h"}`;
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

/** Whole calendar days from `from` to `to` in the zone of `opts` (DST-proof: compares dates). */
function calendarDays(to: number, from: number, opts: ClockOptions): number {
  const f = dtf({ timeZone: opts.timeZone, locale: "en-US" }, { year: "numeric", month: "numeric", day: "numeric" });
  const utcDay = (ms: number) => {
    const part = (type: string) => Number(f.formatToParts(ms).find((p) => p.type === type)?.value);
    return Date.UTC(part("year"), part("month") - 1, part("day"));
  };
  return Math.round((utcDay(to) - utcDay(from)) / DAY);
}

/** A letter of any script but Latin ("오후", "م", "शनि"). */
const NON_LATIN = /[^\P{L}\p{Script=Latin}]/u;

/** A week of UTC noons, Sunday first. */
const WEEK = Array.from({ length: 7 }, (_, i) => Date.UTC(2026, 0, 4 + i, 12));

/** Weekday style of the compact clock per locale; null keeps the full clock. */
const compactCache = new Map<string, "short" | "narrow" | null>();

/**
 * A 12-hour clock written in another script than Latin ("토 오후 9:36", "السبت ٩:٣٦ م",
 * "शनि 9:36 pm") runs past the card's reset and burn lines, so formatClock switches it to a
 * 24-hour time and, where the seven stay distinct, one-letter weekdays. Latin AM/PM and
 * 24-hour locales keep the full clock.
 */
function compactWeekday(opts: ClockOptions): "short" | "narrow" | null {
  const key = opts.locale ?? "";
  let style = compactCache.get(key);
  if (style === undefined) {
    const loc = { locale: opts.locale, timeZone: "UTC" };
    const sample = dtf(loc, { weekday: "short", hour: "numeric", minute: "2-digit" }).format(WEEK[6]);
    const narrow = new Set(WEEK.map((d) => dtf(loc, { weekday: "narrow" }).format(d)));
    style = uses12h(loc) && NON_LATIN.test(sample) ? (narrow.size === 7 ? "narrow" : "short") : null;
    compactCache.set(key, style);
  }
  return style;
}

/**
 * Reset clock: time only when it falls on the same calendar day as `now`, otherwise
 * "Sat 15:40" up to six calendar days away and "12 Oct 15:40" beyond that (compacted as above).
 */
export function formatClock(atMs: number, now: number, opts: ClockOptions = {}): string {
  if (!Number.isFinite(atMs)) return "—";
  const compact = compactWeekday(opts);
  const time = compact ? dtf(opts, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(atMs) : formatTime(atMs, opts);
  const days = Math.abs(calendarDays(atMs, now, opts));
  if (days === 0) return time;
  // By calendar day, not elapsed time: seven days on is the same weekday as today.
  if (days <= 6) {
    return `${dtf(opts, { weekday: compact ?? "short" }).format(atMs)} ${time}`;
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

/** True when the reset is only known to the day (see {@link ROUGH_RESET_PM}). */
export function isRough(reset: ResetInfo): boolean {
  return reset.type === "estimated" && reset.plus_minus_ms >= ROUGH_RESET_PM;
}

/** Day of a reset: "today", "Thu" up to six calendar days away, "12 Oct" beyond. */
export function formatDay(atMs: number, now: number, opts: ClockOptions = {}): string {
  if (!Number.isFinite(atMs)) return "—";
  const days = Math.abs(calendarDays(atMs, now, opts));
  if (days === 0) return "today";
  if (days <= 6) return dtf(opts, { weekday: "short" }).format(atMs);
  return dtf(opts, { day: "numeric", month: "short" }).format(atMs);
}

/** Whole days until a rough reset: "2d", "<1d" inside the last day. */
export function formatRoughCountdown(remainingMs: number): string {
  if (!Number.isFinite(remainingMs) || remainingMs < DAY) return "<1d";
  return `${Math.round(remainingMs / DAY)}d`;
}

/** When a reset happens, as precisely as it is known: "~Thu" when rough, else the clock. */
export function resetWhen(reset: ResetInfo, at: number, now: number, opts: ClockOptions = {}): string {
  if (isRough(reset)) return `~${formatDay(at, now, opts)}`;
  return `${isEstimated(reset) ? "~" : ""}${formatClock(at, now, opts)}`;
}

/** True when the window is waiting for fresh data after its reset time passed. */
export function awaitingReset(w: Pick<WindowView, "phase" | "reset">, now: number): boolean {
  if (w.phase === "reset_awaiting_data") return true;
  const at = resetAt(w.reset);
  return at !== null && at <= now;
}

/**
 * The window as it should be displayed at `now`. Between snapshots the reset instant can pass
 * on the UI's clock before Rust reports `reset_awaiting_data`; show it the way Rust will
 * (merge.rs: phase awaiting, pct 0, not stale, no limit/burn) rather than "100% · reset".
 */
export function liveWindow(w: WindowView, now: number): WindowView {
  if (w.phase === "reset_awaiting_data" || !awaitingReset(w, now)) return w;
  return { ...w, pct: 0, limit_reached: false, stale: false, burn: null, phase: "reset_awaiting_data" };
}

/** True when a data source's last update is at least `staleMin` minutes old. */
export function sourceStale(atMs: number, now: number, staleMin: number): boolean {
  // `>=` so the flip coincides with the minute tick that renders the matching age.
  return now - atMs >= staleMin * MIN;
}

/** Pill countdown: "3h 12m", "~3h 12m" when estimated, "reset" after it, "—" when unknown. */
export function pillCountdown(w: Pick<WindowView, "phase" | "reset">, now: number): string {
  if (awaitingReset(w, now)) return "reset";
  const at = resetAt(w.reset);
  if (at === null) return "—";
  if (isRough(w.reset)) return `~${formatRoughCountdown(at - now)}`;
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

/**
 * Card reset line: "resets 15:40 · in 3h 12m"; an estimate is marked once, on the clock
 * ("resets ~15:40 · in 3h 12m"), so 12-hour locales still fit beside the sparkline. Without
 * `clock` (a reached limit's line already names it): "resets in 3h 12m", "resets in ~3h 12m".
 */
export function resetLine(w: Pick<WindowView, "phase" | "reset">, now: number, opts: ClockOptions = {}, clock = true): string {
  if (awaitingReset(w, now)) return "reset — waiting for data";
  const at = resetAt(w.reset);
  if (at === null) return "reset time unknown";
  if (isRough(w.reset)) {
    const left = `in ~${formatRoughCountdown(at - now)}`;
    return clock ? `resets ${resetWhen(w.reset, at, now, opts)} · ${left}` : `resets ${left}`;
  }
  const t = isEstimated(w.reset) ? "~" : "";
  if (!clock) return `resets in ${t}${formatCountdown(at - now)}`;
  return `resets ${t}${formatClock(at, now, opts)} · in ${formatCountdown(at - now)}`;
}

/** Tooltip for an estimated reset, e.g. "Estimated from Claude Desktop history, ±25m (medium confidence)". */
export function estimateTooltip(reset: ResetInfo, now?: number, opts: ClockOptions = {}): string | undefined {
  if (reset.type !== "estimated") return undefined;
  const base = `Estimated from Claude Desktop history, ±${formatSpan(reset.plus_minus_ms)} (${reset.confidence} confidence)`;
  if (!isRough(reset) || now === undefined) return base;
  const from = formatClock(reset.at_ms - reset.plus_minus_ms, now, opts);
  const to = formatClock(reset.at_ms + reset.plus_minus_ms, now, opts);
  return `${base}: between ${from} and ${to}. Connect Claude Code for the exact time.`;
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
