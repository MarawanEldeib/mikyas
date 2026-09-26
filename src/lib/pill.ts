// Screen-reader text of one pill window (its aria-label). The visible countdown can be "reset"
// or "—", which read badly after "resets in", so the reset part has its own phrases.
import { awaitingReset, estimateTooltip, formatAge, formatPct, pillCountdown, resetAt, windowLabel } from "./format";
import type { WindowView } from "./types";
import { WORKED_SINCE_TIP, showWorkedSince } from "./worked";

/** "resets in 3h 12m", "reset now, waiting for new data" or "reset time unknown". */
export function resetPhrase(w: Pick<WindowView, "phase" | "reset">, now: number): string {
  if (awaitingReset(w, now)) return "reset now, waiting for new data";
  if (resetAt(w.reset) === null) return "reset time unknown";
  return `resets in ${pillCountdown(w, now)}`;
}

/** "5-hour limit 42% used, resets in 3h 12m" plus limit, worked-since, stale and estimate notes. */
export function describeWindow(w: WindowView, now: number): string {
  const parts = [`${windowLabel(w.kind)} limit ${formatPct(w.pct)}% used`];
  if (w.limit_reached) parts.push("limit reached");
  if (showWorkedSince(w)) parts.push(WORKED_SINCE_TIP);
  parts.push(resetPhrase(w, now));
  if (w.stale) parts.push(`last updated ${formatAge(w.observed_at_ms, now)}`);
  const est = estimateTooltip(w.reset);
  if (est) parts.push(est);
  return parts.join(", ");
}
