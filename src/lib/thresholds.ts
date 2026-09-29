// Alert thresholds as the Settings steppers show them. Rust keeps the saved list exactly as the
// user set it (empty = no alerts, one value = one alert, more are fine too); the steppers edit
// the first two, so a shorter list is shown with the defaults filling in, without saving them.

/** Mirror settings.rs `DEFAULT_THRESHOLDS` / `DEFAULT_CTX_THRESHOLDS`; rust-sync.test.ts fails when they differ. */
export const DEFAULT_THRESHOLDS = [80, 95] as const;
export const DEFAULT_CTX_THRESHOLDS = [80, 90] as const;

/** The ascending pair the two steppers show for `list` (1..=100, the second above the first). */
export function thresholdPair(list: readonly number[] | undefined, defaults: readonly [number, number]): [number, number] {
  const first = Math.min(99, Math.max(1, list?.[0] ?? defaults[0]));
  const second = Math.min(100, Math.max(first + 1, list?.[1] ?? defaults[1]));
  return [first, second];
}
