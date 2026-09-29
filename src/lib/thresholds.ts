// Alert thresholds as the Settings list edits them. Rust keeps the saved list exactly as the user
// set it (sorted, without repeats; empty = no alerts, one value = one alert, any number is fine),
// and the UI edits that list as it is: every value, add and remove, down to none.

/** Mirror mikyas_core `alerts::DEFAULT_THRESHOLDS` / `ctx_alerts::DEFAULT_CTX_THRESHOLDS` (the
 *  one source, which the app's settings use too); rust-sync.test.ts fails when they differ. */
export const DEFAULT_THRESHOLDS = [80, 95] as const;
export const DEFAULT_CTX_THRESHOLDS = [80, 90] as const;

/** The range a threshold may take (settings.rs keeps 1..=100). */
export const THRESHOLD_MIN = 1;
export const THRESHOLD_MAX = 100;

/** Sorted, without repeats, inside the range: the shape Rust stores. */
export function normalizeThresholds(list: readonly number[]): number[] {
  const kept = list.filter((v) => Number.isInteger(v) && v >= THRESHOLD_MIN && v <= THRESHOLD_MAX);
  return [...new Set(kept)].sort((a, b) => a - b);
}

/** The bounds of the value at `i` that keep the list ascending (between its neighbours). */
export function thresholdBounds(list: readonly number[], i: number): [number, number] {
  const min = i > 0 ? list[i - 1] + 1 : THRESHOLD_MIN;
  const max = i < list.length - 1 ? list[i + 1] - 1 : THRESHOLD_MAX;
  return [min, Math.max(min, max)];
}

/** `list` with the value at `i` replaced. */
export function setThreshold(list: readonly number[], i: number, value: number): number[] {
  return normalizeThresholds(list.map((v, j) => (j === i ? value : v)));
}

/** `list` without the value at `i`. */
export function removeThreshold(list: readonly number[], i: number): number[] {
  return list.filter((_, j) => j !== i);
}

/**
 * `list` with one more threshold: the first default not in it yet, else a value above the highest
 * (5 points up, capped at 100), else the middle of the widest free gap. `null` when every value
 * from 1 to 100 is already taken.
 */
export function addThreshold(list: readonly number[], defaults: readonly number[]): number[] | null {
  const have = normalizeThresholds(list);
  const taken = new Set(have);
  const pick =
    defaults.find((d) => !taken.has(d)) ??
    [Math.min(THRESHOLD_MAX, (have.at(-1) ?? 0) + 5)].find((v) => !taken.has(v)) ??
    widestGapMiddle(have);
  return pick === null ? null : normalizeThresholds([...have, pick]);
}

function widestGapMiddle(sorted: readonly number[]): number | null {
  const edges = [THRESHOLD_MIN - 1, ...sorted, THRESHOLD_MAX + 1];
  let best: number | null = null;
  let width = 1;
  for (let i = 1; i < edges.length; i++) {
    const gap = edges[i] - edges[i - 1];
    if (gap > width) {
      width = gap;
      best = Math.floor((edges[i] + edges[i - 1]) / 2);
    }
  }
  return best;
}
