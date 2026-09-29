// English count phrases ("1 warning", "0 warnings", "3 resets"), shared so no component
// hand-rolls its own rule (a `> 1` check reads "0 warning").

/** "1 reset", "0 resets", "2 resets"; `many` for irregular words (defaults to `one` + "s"). */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
