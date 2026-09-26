// Shared step-line builder for the sparkline and the history chart: each point holds its value
// until the next one (or for `step` ms); a null value, or a gap longer than 1.5 steps, breaks it.

import type { SparkPoint } from "./types";

export interface StepPath {
  /** Stroke path, one `M` subpath per contiguous run; "" without data. */
  line: string;
  /** Closed fill under each run, down to the 0% baseline. */
  area: string;
  /** Right end of the latest value, or null when every point is a gap. */
  last: { x: number; y: number; pct: number } | null;
  runs: number;
}

/** The value of a point, or null for a gap. */
export const valueOf = (p: SparkPoint): number | null => (p.pct === null || !Number.isFinite(p.pct) ? null : p.pct);

/** Builds the path over points sorted by time; `xOf`/`yOf` map a time and a percentage to the plot. */
export function stepPath(
  pts: readonly SparkPoint[],
  xOf: (t: number) => number,
  yOf: (pct: number) => number,
  step: number,
): StepPath {
  const base = yOf(0);
  let line = "";
  let area = "";
  let runs = 0;
  let run = ""; // current run's line commands
  let runStartX = 0;
  let lastY = Number.NaN;
  let last: StepPath["last"] = null;

  const closeRun = () => {
    if (!run) return;
    line += run;
    area += `${run}V${base}H${runStartX}Z`;
    runs++;
    run = "";
  };

  for (let i = 0; i < pts.length; i++) {
    const v = valueOf(pts[i]);
    if (v === null) {
      closeRun();
      continue;
    }
    const next = pts[i + 1];
    const adjacent = next !== undefined && next.t_ms - pts[i].t_ms <= step * 1.5;
    const xs = xOf(pts[i].t_ms);
    const xe = xOf(adjacent ? next.t_ms : pts[i].t_ms + step);
    const y = yOf(v);
    if (!run) {
      runStartX = xs;
      run = `M${xs} ${y}H${xe}`;
    } else {
      run += `${y === lastY ? "" : `V${y}`}H${xe}`;
    }
    lastY = y;
    last = { x: xe, y, pct: v };
    if (!adjacent) closeRun();
  }
  closeRun();
  return { line, area, last, runs };
}
