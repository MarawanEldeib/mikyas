// Countdown tick scheduler. Instead of a fixed interval, one setTimeout is armed for the
// next instant any displayed string changes (see format.ts for the matching units): every
// second only while a countdown is under 10 minutes, otherwise on minute/hour boundaries.
// Times are always recomputed from Date.now(), so drift and sleep/resume cannot accumulate.

import { DAY, HOUR, MIN, SEC, SECONDS_BELOW } from "./format";

/** Instants the UI renders relative to now. */
export interface TickTargets {
  /** Future instants shown as countdowns (reset times). */
  deadlines: readonly number[];
  /** Past instants shown as relative ages ("2m ago"). */
  pasts: readonly number[];
}

/** Lower bound for a scheduled delay (ms). */
export const MIN_DELAY = 50;
/** Upper bound for a scheduled delay (ms): re-evaluate at least once a minute. */
export const MAX_DELAY = MIN;

/** Milliseconds until `formatCountdown(deadline - now)` changes, or null once it has passed. */
export function countdownChangeIn(deadline: number, now: number): number | null {
  const r = deadline - now;
  if (!Number.isFinite(r) || r <= 0) return null;
  const unit = r < SECONDS_BELOW ? SEC : r < DAY ? MIN : HOUR;
  // Displays floor(r / unit); it changes 1 ms after r drops to the next multiple.
  return (r % unit) + 1;
}

/** Milliseconds until `formatAge(past, now)` changes. */
export function ageChangeIn(past: number, now: number): number | null {
  const a = now - past;
  if (!Number.isFinite(a)) return null;
  const unit = a < HOUR ? MIN : a < DAY ? HOUR : DAY;
  // `%` keeps the sign, so a future `past` (clock skew) waits until it is a minute old.
  return unit - (a % unit);
}

/** Smallest change delay over all targets (unclamped), or null when nothing will change. */
export function nextChangeDelay(now: number, targets: TickTargets): number | null {
  let best: number | null = null;
  const consider = (d: number | null) => {
    if (d !== null && (best === null || d < best)) best = d;
  };
  for (const t of targets.deadlines) consider(countdownChangeIn(t, now));
  for (const t of targets.pasts) consider(ageChangeIn(t, now));
  return best;
}

/** Clamps a delay into [MIN_DELAY, MAX_DELAY]. */
export function clampDelay(ms: number): number {
  return Math.min(MAX_DELAY, Math.max(MIN_DELAY, Math.ceil(ms)));
}

type VisibilityDoc = Pick<Document, "visibilityState" | "addEventListener" | "removeEventListener">;

export interface TickerDeps {
  now?: () => number;
  setTimeout?: (fn: () => void, ms: number) => unknown;
  clearTimeout?: (handle: unknown) => void;
  /** Visibility source; pass null to disable pausing. Defaults to `document` when present. */
  doc?: VisibilityDoc | null;
}

export interface Ticker {
  /** Re-arms the timer; call whenever the targets change. */
  refresh(): void;
  /** Cancels the timer and stops listening for visibility changes. */
  stop(): void;
}

/**
 * Calls `onTick(now)` exactly when a displayed countdown or age string changes. Paused while
 * the document is hidden; ticks immediately when it becomes visible again.
 */
export function createTicker(getTargets: () => TickTargets, onTick: (now: number) => void, deps: TickerDeps = {}): Ticker {
  const now = deps.now ?? (() => Date.now());
  const setT = deps.setTimeout ?? ((fn, ms) => globalThis.setTimeout(fn, ms));
  const clearT = deps.clearTimeout ?? ((h) => globalThis.clearTimeout(h as ReturnType<typeof setTimeout>));
  const doc = deps.doc === undefined ? (typeof document === "undefined" ? null : document) : deps.doc;
  let handle: unknown = null;
  let stopped = false;

  const clear = () => {
    if (handle !== null) clearT(handle);
    handle = null;
  };
  const hidden = () => doc?.visibilityState === "hidden";

  const schedule = () => {
    clear();
    if (stopped || hidden()) return;
    const d = nextChangeDelay(now(), getTargets());
    if (d === null) return;
    handle = setT(fire, clampDelay(d));
  };
  const fire = () => {
    handle = null;
    if (stopped) return;
    onTick(now());
    schedule();
  };
  const onVisibility = () => (hidden() ? clear() : fire());

  doc?.addEventListener("visibilitychange", onVisibility);
  schedule();

  return {
    refresh: schedule,
    stop() {
      stopped = true;
      clear();
      doc?.removeEventListener("visibilitychange", onVisibility);
    },
  };
}
