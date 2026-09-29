// Context-window size overrides: free entry with quick picks, the same bounds as the Rust side
// (mikyas_core::engine::context::{MIN_CTX_TOKENS, MAX_CTX_TOKENS}).

export const CTX_MIN = 1_000;
export const CTX_MAX = 100_000_000;
/** Quick picks offered next to the free entry (any other size in range can be typed). */
export const CTX_PRESETS: readonly number[] = [200_000, 1_000_000];

export const CTX_SIZE_HINT = "Use a size like 400K or 1.5M (1K–100M tokens)";

const MULTIPLIER: Record<string, number> = { "": 1, k: 1_000, m: 1_000_000 };

/**
 * Tokens from what the user typed: "400000", "400,000", "400 000", "400K", "1.5M", "2m".
 * `null` when it is not a number or lies outside CTX_MIN..=CTX_MAX.
 */
export function parseCtxSize(input: string): number | null {
  const m = /^(\d+(?:\.\d+)?)([km]?)$/i.exec(input.replace(/[\s,_]/g, ""));
  if (!m) return null;
  const n = Math.round(Number(m[1]) * MULTIPLIER[m[2].toLowerCase()]);
  return n >= CTX_MIN && n <= CTX_MAX ? n : null;
}

/** Exact short form for an input field: 1000000 → "1M", 400000 → "400K", 1234567 → "1234567". */
export function formatCtxSize(n: number): string {
  if (n >= 1_000_000 && n % 100_000 === 0) return `${n / 1_000_000}M`;
  if (n > 0 && n % 1_000 === 0) return `${n / 1_000}K`;
  return String(n);
}

/** A model id the way overrides are keyed: trimmed, without `[1m]`; `null` if it is not an id. */
export function normalizeModelId(input: string): string | null {
  const id = input.trim().replace(/\[1m\]$/i, "");
  return /^[a-z0-9][a-z0-9._-]*$/i.test(id) ? id : null;
}
