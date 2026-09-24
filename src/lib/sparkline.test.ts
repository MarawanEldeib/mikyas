import { describe, expect, it } from "vitest";
import { sparkGeometry, sparkY } from "./sparkline";
import type { SparkPoint } from "./types";

const OPTS = { width: 104, height: 44, padX: 2, padY: 2 };
const series = (vals: (number | null)[], step = 900_000, t0 = 1_000_000): SparkPoint[] =>
  vals.map((pct, i) => ({ t_ms: t0 + i * step, pct }));
const subpaths = (d: string) => (d.match(/M/g) ?? []).length;

describe("sparkY", () => {
  it("maps 0..100 bottom to top and clamps", () => {
    expect(sparkY(0, OPTS)).toBe(42);
    expect(sparkY(100, OPTS)).toBe(2);
    expect(sparkY(50, OPTS)).toBe(22);
    expect(sparkY(150, OPTS)).toBe(2);
    expect(sparkY(-20, OPTS)).toBe(42);
  });
});

describe("sparkGeometry", () => {
  it("draws a step path across the full width", () => {
    const g = sparkGeometry(series([0, 50, 50, 100]), OPTS);
    expect(g.runs).toBe(1);
    expect(g.line).toBe("M2 42H27V22H52H77V2H102");
    expect(g.dot).toEqual({ x: 102, y: 2 });
    expect(g.area).toBe("M2 42H27V22H52H77V2H102V42H2Z");
  });

  it("breaks the line at null gaps", () => {
    const g = sparkGeometry(series([10, 20, null, null, 30, 40, null, 50]), OPTS);
    expect(g.runs).toBe(3);
    expect(subpaths(g.line)).toBe(3);
    expect(subpaths(g.area)).toBe(3);
    // Each run's area closes back to its own start on the baseline.
    expect((g.area.match(/Z/g) ?? []).length).toBe(3);
    expect(g.dot?.x).toBe(102);
  });

  it("breaks the line where samples are missing in time", () => {
    const pts: SparkPoint[] = [
      { t_ms: 0, pct: 10 },
      { t_ms: 1000, pct: 20 },
      { t_ms: 5000, pct: 30 },
      { t_ms: 6000, pct: 40 },
    ];
    const g = sparkGeometry(pts, OPTS);
    expect(g.runs).toBe(2);
  });

  it("ends the dot at the last non-null value", () => {
    const g = sparkGeometry(series([10, 60, null, null]), OPTS);
    expect(g.dot).toEqual({ x: 52, y: sparkY(60, OPTS) });
  });

  it("returns nothing for an all-null series", () => {
    const g = sparkGeometry(series([null, null, null]), OPTS);
    expect(g).toEqual({ line: "", area: "", dot: null, runs: 0 });
  });

  it("returns nothing for an empty series", () => {
    expect(sparkGeometry([], OPTS)).toEqual({ line: "", area: "", dot: null, runs: 0 });
  });

  it("shows a lone sample as a dot at the right edge", () => {
    const g = sparkGeometry([{ t_ms: 5, pct: 40 }], OPTS);
    expect(g.line).toBe("");
    expect(g.dot).toEqual({ x: 102, y: sparkY(40, OPTS) });
  });

  it("draws a single valid point among gaps as one bucket", () => {
    const g = sparkGeometry(series([null, 70, null, null]), OPTS);
    expect(g.runs).toBe(1);
    expect(g.line).toBe(`M27 ${sparkY(70, OPTS)}H52`);
  });

  it("clamps out-of-range and ignores non-finite values", () => {
    const g = sparkGeometry(series([140, -10, Number.NaN, 20]), OPTS);
    expect(g.line.startsWith("M2 2H27V42H52")).toBe(true);
    expect(g.runs).toBe(2);
  });

  it("sorts unordered input by time", () => {
    const pts = series([10, 20, 30]).reverse();
    expect(sparkGeometry(pts, OPTS).line).toBe(sparkGeometry(series([10, 20, 30]), OPTS).line);
  });

  it("uses 96 points without gaps as one run", () => {
    const vals = Array.from({ length: 96 }, (_, i) => (i * 7) % 100);
    const g = sparkGeometry(series(vals), OPTS);
    expect(g.runs).toBe(1);
    expect(g.dot?.x).toBe(102);
  });
});
