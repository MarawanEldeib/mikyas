// Window keys read generically, as Rust's `WindowKind` does (crates/core/src/engine/types.rs):
// a leading `<number>_<unit>` span (`five_hour`, `seven_day_opus`, `twenty_four_hour`, `2_week`,
// `thirty_minute`, `one_month`) or a period word (`daily`, `weekly_opus`), then an optional scope.
// Rust sends the names with every window; this is the fallback for data without them, and it
// keeps keys Claude adds later readable. rust-sync.test.ts checks it against Rust's label tests.

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;

interface Unit {
  word: string;
  letter: string;
  ms: number;
}

const UNITS = new Map<string, Unit>();
function unit(names: string[], word: string, letter: string, ms: number): Unit {
  const u = { word, letter, ms };
  for (const n of names) UNITS.set(n, u);
  return u;
}
unit(["minute", "minutes", "min", "mins"], "minute", "m", MINUTE);
const HOUR_U = unit(["hour", "hours", "h", "hr", "hrs"], "hour", "h", HOUR);
const DAY_U = unit(["day", "days", "d"], "day", "d", DAY);
const WEEK_U = unit(["week", "weeks", "w", "wk"], "week", "w", WEEK);
const MONTH_U = unit(["month", "months", "mo"], "month", "mo", 30 * DAY);
const PERIOD_WORDS = new Map<string, Unit>([
  ["hourly", HOUR_U],
  ["daily", DAY_U],
  ["weekly", WEEK_U],
  ["monthly", MONTH_U],
]);

const SMALL = [
  "zero",
  "one",
  "two",
  "three",
  "four",
  "five",
  "six",
  "seven",
  "eight",
  "nine",
  "ten",
  "eleven",
  "twelve",
  "thirteen",
  "fourteen",
  "fifteen",
  "sixteen",
  "seventeen",
  "eighteen",
  "nineteen",
];
const TENS = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

function small(s: string | undefined): number | null {
  const i = s === undefined ? -1 : SMALL.indexOf(s);
  return i < 0 ? null : i;
}
function tens(s: string | undefined): number | null {
  const i = s === undefined ? -1 : TENS.indexOf(s);
  return i < 0 ? null : 20 + 10 * i;
}
const digit = (n: number | null) => (n !== null && n >= 1 && n < 10 ? n : null);

/** The count at the start of `parts` and how many parts it used (Rust `parse_count`). */
function parseCount(parts: string[]): [number, number] | null {
  const first = parts[0];
  let n: number | null;
  let used = 1;
  const hyphen = first.indexOf("-");
  if (/^\d{1,3}$/.test(first)) n = Number(first);
  else if (small(first) !== null) n = small(first);
  else if (hyphen >= 0) {
    // `twenty-four`: a tens word and a unit word in one part.
    const t = tens(first.slice(0, hyphen));
    const u = digit(small(first.slice(hyphen + 1)));
    n = t !== null && u !== null ? t + u : null;
  } else {
    n = tens(first);
    const u = n === null ? null : digit(small(parts[1]));
    if (n !== null && u !== null) {
      n += u;
      used = 2;
    }
  }
  return n !== null && n > 0 ? [n, used] : null;
}

export interface Span {
  n: number;
  unit: Unit;
  ms: number;
  /** The scope after the span (`opus` in `seven_day_opus`), "" when none. */
  rest: string;
}

/** The leading span of a window key, or `null` when it has none (`spend_limit`). */
export function spanOf(key: string): Span | null {
  const parts = key.split("_");
  let n: number;
  let u: Unit | undefined;
  let used: number;
  const period = PERIOD_WORDS.get(parts[0]);
  if (period) {
    [n, u, used] = [1, period, 1];
  } else {
    const count = parseCount(parts);
    if (!count) return null;
    [n, used] = count;
    u = UNITS.get(parts[used] ?? "");
    if (!u) return null;
    used += 1;
  }
  const head = parts.slice(0, used).reduce((sum, p) => sum + p.length, 0) + used - 1;
  return { n, unit: u, ms: n * u.ms, rest: key.slice(head + 1) };
}

/** Nominal length of a window key in ms, or `null` when unknown. */
export function durationOf(key: string): number | null {
  return spanOf(key)?.ms ?? null;
}

/** True for a key with a known span and no scope (`five_hour`, `thirty_day`). */
export function isUnscoped(key: string): boolean {
  return spanOf(key)?.rest === "";
}

function withSuffix(base: string, rest: string): string {
  const words = rest
    .split("_")
    .filter((w) => w)
    .join(" ");
  return words ? `${base} ${words.charAt(0).toUpperCase()}${words.slice(1)}` : base;
}

/** Window name: "5-hour", "weekly", "weekly Opus", "30-day", else the key with spaces. */
export function spanLabel(key: string): string {
  const s = spanOf(key);
  if (!s) return key.replace(/_/g, " ").trim() || key;
  return withSuffix(s.ms === WEEK ? "weekly" : `${s.n}-${s.unit.word}`, s.rest);
}

/** Compact name: "5h", "7d Opus", "30d", else the label. */
export function spanShort(key: string): string {
  const s = spanOf(key);
  return s ? withSuffix(`${s.n}${s.unit.letter}`, s.rest) : spanLabel(key);
}

/**
 * The main windows of `kinds` (Rust `main_kinds`): of the keys with a known length and no scope,
 * the shortest and the longest; else the same over every key with a known length; else the
 * first two. `five_hour` / `seven_day` only break ties. Shortest first.
 */
export function mainKinds(kinds: readonly string[]): string[] {
  const timed = (unscopedOnly: boolean) =>
    kinds
      .filter((k) => !unscopedOnly || isUnscoped(k))
      .map((k) => [k, durationOf(k)] as const)
      .filter((p): p is readonly [string, number] => p[1] !== null);
  let pool = timed(true);
  if (!pool.length) pool = timed(false);
  if (!pool.length) return [...new Set(kinds)].slice(0, 2);
  const builtin = (k: string) => (k === "five_hour" || k === "seven_day" ? 0 : 1);
  const cmp = (a: readonly [string, number], b: readonly [string, number], dir: 1 | -1) =>
    dir * (a[1] - b[1]) || builtin(a[0]) - builtin(b[0]) || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0);
  const shortest = [...pool].sort((a, b) => cmp(a, b, 1))[0][0];
  const longest = [...pool].sort((a, b) => cmp(a, b, -1))[0][0];
  return longest === shortest ? [shortest] : [shortest, longest];
}
