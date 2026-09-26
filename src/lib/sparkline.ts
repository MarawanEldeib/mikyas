// Pure sparkline geometry: a step ("hold until the next sample") path over a fixed 0..100
// scale, broken at gaps. Each point covers the bucket from its own time to the next point's
// time; a `null` pct, or a missing stretch longer than 1.5 buckets, breaks the line.

import { clampPct } from "./color";
import { stepPath, valueOf } from "./step";
import type { SparkPoint } from "./types";

export interface SparkOptions {
  width: number;
  height: number;
  /** Horizontal inset so the current-point dot is not clipped. */
  padX?: number;
  /** Vertical inset so strokes at 0% / 100% are not clipped. */
  padY?: number;
}

export interface SparkGeometry {
  /** Stroke path (one `M` subpath per contiguous run); "" when there is no data. */
  line: string;
  /** Closed fill path under each run, down to the 0% baseline; "" when there is no data. */
  area: string;
  /** Latest value's position (end of its bucket), or null when every point is a gap. */
  dot: { x: number; y: number } | null;
  /** Number of contiguous runs (subpaths). */
  runs: number;
}

const round = (v: number) => Math.round(v * 100) / 100;

/** Y coordinate of a percentage within the plot (0% at the bottom, 100% at the top). */
export function sparkY(pct: number, opts: SparkOptions): number {
  const padY = opts.padY ?? 2;
  return round(padY + (1 - clampPct(pct) / 100) * (opts.height - 2 * padY));
}

function medianStep(ts: number[]): number | null {
  const diffs: number[] = [];
  for (let i = 1; i < ts.length; i++) {
    const d = ts[i] - ts[i - 1];
    if (d > 0) diffs.push(d);
  }
  if (diffs.length === 0) return null;
  diffs.sort((a, b) => a - b);
  return diffs[Math.floor(diffs.length / 2)];
}

/** Builds the step line, area and current-point dot for a series of spark points. */
export function sparkGeometry(points: readonly SparkPoint[], opts: SparkOptions): SparkGeometry {
  const empty: SparkGeometry = { line: "", area: "", dot: null, runs: 0 };
  const padX = opts.padX ?? 2;
  const pts = points
    .filter((p) => Number.isFinite(p.t_ms))
    .slice()
    .sort((a, b) => a.t_ms - b.t_ms);
  if (pts.length === 0) return empty;

  const step = medianStep(pts.map((p) => p.t_ms));
  if (step === null) {
    // One distinct instant: no width to draw a line over, so show the latest value as a dot.
    const last = [...pts].reverse().find((p) => valueOf(p) !== null);
    if (!last) return empty;
    return { ...empty, dot: { x: round(opts.width - padX), y: sparkY(valueOf(last) ?? 0, opts) } };
  }

  const t0 = pts[0].t_ms;
  const span = pts[pts.length - 1].t_ms + step - t0;
  const x = (t: number) => round(padX + ((t - t0) / span) * (opts.width - 2 * padX));
  const { line, area, last, runs } = stepPath(pts, x, (pct) => sparkY(pct, opts), step);
  return { line, area, dot: last && { x: last.x, y: last.y }, runs };
}
