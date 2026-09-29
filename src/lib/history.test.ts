import { describe, expect, it } from "vitest";
import { DAY, HOUR } from "./format";
import {
  FETCH_DAYS,
  HOUR_LABEL_EVERY,
  LABEL_GAP,
  MAX_LABEL_DAYS,
  MIN_LABELLED_DAY,
  RANGES,
  addLocalDays,
  barScale,
  chartGeometry,
  dayBars,
  placeLabels,
  plotX,
  plotY,
  rangeDomain,
  rangesFor,
  resetXs,
  startOfLocalDay,
  summaryText,
  timeAxis,
  windowSummary,
  type PlotBox,
} from "./history";
import type { HistoryDay, SparkPoint } from "./types";

// Local wall-clock instants, so the tests hold in any time zone.
const local = (d: number, h = 0, m = 0) => new Date(2026, 8, d, h, m).getTime();
const NOW = local(24, 10, 17);
const TO = local(24, 11);

const BOX: PlotBox = { width: 110, height: 60, left: 10, right: 0, top: 5, bottom: 5 };
const hourly = (from: number, vals: (number | null)[]): SparkPoint[] => vals.map((pct, i) => ({ t_ms: from + i * HOUR, pct }));
const subpaths = (d: string) => (d.match(/M/g) ?? []).length;

describe("domain and scales", () => {
  it("slices the loaded range", () => {
    const data = { from_ms: TO - 14 * DAY, to_ms: TO };
    expect(rangeDomain(data, DAY)).toEqual({ from: TO - DAY, to: TO });
    expect(rangeDomain({ from_ms: TO - DAY, to_ms: TO }, 7 * DAY)).toEqual({ from: TO - DAY, to: TO });
    expect(RANGES.map((r) => r.key)).toEqual(["24h", "7d", "14d"]);
  });

  it("maps time and percent into the plot, clamped", () => {
    const d = { from: 0, to: 100 };
    expect(plotX(0, d, BOX)).toBe(10);
    expect(plotX(50, d, BOX)).toBe(60);
    expect(plotX(150, d, BOX)).toBe(110);
    expect(plotX(-5, d, BOX)).toBe(10);
    expect(plotX(5, { from: 0, to: 0 }, BOX)).toBe(10);
    expect(plotY(0, BOX)).toBe(55);
    expect(plotY(100, BOX)).toBe(5);
    expect(plotY(50, BOX)).toBe(30);
    expect(plotY(140, BOX)).toBe(5);
    expect(plotY(Number.NaN, BOX)).toBe(55);
  });
});

describe("chartGeometry", () => {
  const d = { from: 0, to: 4 * HOUR };

  it("draws hourly steps across the domain", () => {
    const g = chartGeometry(hourly(0, [0, 50, 50, 100]), d, BOX);
    expect(g.runs).toBe(1);
    expect(g.line).toBe("M10 55H35V30H60H85V5H110");
    expect(g.area).toBe("M10 55H35V30H60H85V5H110V55H10Z");
    expect(g.last).toEqual({ x: 110, y: 5, pct: 100 });
  });

  it("breaks at nulls and at missing hours", () => {
    const g = chartGeometry(hourly(0, [20, null, 40, 40]), d, BOX);
    expect(g.runs).toBe(2);
    expect(subpaths(g.line)).toBe(2);
    const missing = chartGeometry([hourly(0, [20])[0], hourly(0, [0, 0, 0, 60])[3]], d, BOX);
    expect(missing.runs).toBe(2);
    expect(missing.line).toBe(`M10 45H35M85 ${plotY(60, BOX)}H110`);
  });

  it("clips points to the domain", () => {
    // Starts half an hour before the domain; the last point would end half an hour after it.
    const pts = hourly(-HOUR / 2, [10, 10, 10, 10, 90]);
    const g = chartGeometry(pts, d, BOX);
    expect(g.line.startsWith("M10 ")).toBe(true);
    expect(g.last?.x).toBe(110);
    expect(g.last?.pct).toBe(90);
    // Entirely outside: nothing.
    expect(chartGeometry(hourly(5 * HOUR, [10]), d, BOX)).toEqual({ line: "", area: "", last: null, runs: 0 });
  });

  it("returns nothing without values", () => {
    expect(chartGeometry([], d, BOX).last).toBeNull();
    expect(chartGeometry(hourly(0, [null, null]), d, BOX).line).toBe("");
  });

  it("places reset ticks inside the domain only", () => {
    expect(resetXs([-1, 0, 2 * HOUR, 4 * HOUR, 5 * HOUR], d, BOX)).toEqual([10, 60, 110]);
  });
});

describe("local days", () => {
  it("finds local midnights", () => {
    expect(startOfLocalDay(NOW)).toBe(local(24));
    expect(addLocalDays(NOW, -1)).toBe(local(23));
    expect(addLocalDays(local(24), 7)).toBe(local(31));
    for (let i = -14; i < 0; i++) {
      const len = addLocalDays(NOW, i + 1) - addLocalDays(NOW, i);
      expect(len).toBeGreaterThanOrEqual(23 * HOUR);
      expect(len).toBeLessThanOrEqual(25 * HOUR);
    }
  });
});

describe("timeAxis", () => {
  const opts = { locale: "en-US" };

  it("labels every 6 hours over 24h, with the weekday at midnight", () => {
    const d = { from: TO - DAY, to: TO };
    const axis = timeAxis(d, "24h", opts);
    expect(axis.lines).toEqual([local(24)]);
    expect(axis.labels.map((l) => l.text)).toEqual(["12 PM", "6 PM", "Thu", "6 AM"]);
    for (const l of axis.labels) expect(new Date(l.t).getHours() % HOUR_LABEL_EVERY).toBe(0);
    const gb = timeAxis(d, "24h", { locale: "en-GB" }).labels.map((l) => l.text);
    expect(gb).toEqual(["12", "18", "Thu", "06"]);
  });

  it("labels each day of a week once, centred on its visible part", () => {
    const d = { from: TO - 7 * DAY, to: TO };
    const axis = timeAxis(d, "7d", opts);
    expect(axis.lines).toHaveLength(7);
    for (const m of axis.lines) expect(new Date(m).getHours()).toBe(0);
    const texts = axis.labels.map((l) => l.text);
    expect(texts).toEqual(["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed", "Thu"]);
    // Today (00:00–11:00 visible) is centred at 05:30.
    expect(axis.labels.at(-1)?.t).toBe(local(24, 5, 30));
    for (const l of axis.labels) {
      expect(l.t).toBeGreaterThan(d.from);
      expect(l.t).toBeLessThan(d.to);
    }
  });

  it("skips days that are barely visible", () => {
    const early = local(24, 3); // only 3 h of today are in view
    const axis = timeAxis({ from: early - 7 * DAY, to: early }, "7d", opts);
    expect(axis.labels.at(-1)?.text).toBe("Wed");
    expect(3 * HOUR).toBeLessThan(MIN_LABELLED_DAY);
  });

  it("labels every other date over 14 days, counting back from today", () => {
    const axis = timeAxis({ from: TO - 14 * DAY, to: TO }, "14d", opts);
    const texts = axis.labels.map((l) => l.text);
    expect(texts).toEqual(["Sep 10", "Sep 12", "Sep 14", "Sep 16", "Sep 18", "Sep 20", "Sep 22", "Sep 24"]);
    expect(axis.lines).toHaveLength(14);
  });

  it("stops the day-label walk after MAX_LABEL_DAYS, however long the domain", () => {
    const axis = timeAxis({ from: TO - 1000 * DAY, to: TO }, "7d", opts);
    expect(axis.labels.length).toBeLessThanOrEqual(MAX_LABEL_DAYS + 1);
    expect(axis.labels.length).toBeGreaterThan(MAX_LABEL_DAYS - 10);
  });
});

describe("placeLabels", () => {
  const d = { from: 0, to: 100 };
  const width = (text: string) => text.length * 10;

  it("keeps labels inside the plot and drops the displaced one of an overlapping pair", () => {
    const labels = [
      { t: 2, text: "aa" }, // centred at 12, moved to 20: overlaps "bb"
      { t: 20, text: "bb" },
      { t: 72, text: "cc" },
      { t: 97, text: "dd" }, // centred at 107, moved to 100: closer to "cc" than the gap
    ];
    expect(placeLabels(labels, d, BOX, width)).toEqual([
      { t: 20, text: "bb", x: 30 },
      { t: 72, text: "cc", x: 82 },
    ]);
    // Moved labels that fit stay.
    expect(
      placeLabels(
        [
          { t: 0, text: "aa" },
          { t: 100, text: "bb" },
        ],
        d,
        BOX,
        width,
      ),
    ).toEqual([
      { t: 0, text: "aa", x: 20 },
      { t: 100, text: "bb", x: 100 },
    ]);
    // Equal claims: the earlier label stays; a label wider than the plot is centred.
    expect(
      placeLabels(
        [
          { t: 50, text: "aa" },
          { t: 52, text: "bb" },
        ],
        d,
        BOX,
        width,
      ).map((l) => l.text),
    ).toEqual(["aa"]);
    expect(placeLabels([{ t: 90, text: "x".repeat(20) }], d, BOX, width)).toEqual([{ t: 90, text: "x".repeat(20), x: 60 }]);
  });

  it("never overlaps on the real 14-day axis, at any hour of the day", () => {
    // HistoryChart's plot box; "Sep 12" is about 30 px wide at 10 px.
    const box: PlotBox = { width: 336, height: 72, left: 24, right: 4, top: 6, bottom: 16 };
    const measure = (text: string) => text.length * 5.2;
    for (let h = 0; h < 24; h++) {
      const to = local(26, h);
      const dom = { from: to - 14 * DAY, to };
      const placed = placeLabels(timeAxis(dom, "14d", { locale: "en-US" }).labels, dom, box, measure);
      expect(placed.length).toBeGreaterThanOrEqual(6);
      for (const l of placed) {
        expect(l.x - measure(l.text) / 2).toBeGreaterThanOrEqual(box.left);
        expect(l.x + measure(l.text) / 2).toBeLessThanOrEqual(box.width - box.right);
      }
      for (let i = 1; i < placed.length; i++) {
        const gap = placed[i].x - measure(placed[i].text) / 2 - (placed[i - 1].x + measure(placed[i - 1].text) / 2);
        expect(gap, `${h}:00 ${placed[i - 1].text}/${placed[i].text}`).toBeGreaterThanOrEqual(LABEL_GAP);
      }
    }
  });
});

describe("dayBars", () => {
  const days: HistoryDay[] = Array.from({ length: 15 }, (_, i) => ({
    day_start_ms: local(10 + i),
    peak_pct: 40 + i,
    consumed_pct: i === 3 ? 0 : i * 1.5,
    samples: 4,
  }));

  it("takes the last days and marks today", () => {
    const bars = dayBars(days, 7, NOW, { locale: "en-US" });
    expect(bars).toHaveLength(7);
    expect(bars.map((b) => b.label)).toEqual(["Fri", "Sat", "Sun", "Mon", "Tue", "Wed", "Thu"]);
    expect(bars.map((b) => b.today)).toEqual([false, false, false, false, false, false, true]);
    expect(bars[6]).toMatchObject({ value: 21, peak: 54, hasData: true });
    expect(bars[6].full).toBe("Thu, Sep 24: 21% of the weekly limit used (peak 54%)");
    expect(bars[6].detail).toBe("Thu, Sep 24 · 21%");
    expect(bars.every((b, i) => b.start === local(18 + i))).toBe(true);
  });

  it("uses narrow labels for two weeks and describes empty days", () => {
    const bars = dayBars(days, 14, NOW, { locale: "en-US" });
    expect(bars).toHaveLength(14);
    expect(bars[0].label).toBe("F");
    // No rows at all is "no data"; rows without any rise are a real 0%.
    const empty = dayBars([{ day_start_ms: local(24), peak_pct: 0, consumed_pct: 0, samples: 0 }], 7, NOW, { locale: "en-US" });
    expect(empty[0]).toMatchObject({ hasData: false, detail: "Thu, Sep 24 · no data", full: "Thu, Sep 24: no data" });
    const idle = dayBars([{ day_start_ms: local(24), peak_pct: 12, consumed_pct: 0.2, samples: 3 }], 7, NOW, { locale: "en-US" });
    expect(idle[0]).toMatchObject({ hasData: true, detail: "Thu, Sep 24 · 0%" });
    expect(idle[0].full).toBe("Thu, Sep 24: 0% of the weekly limit used (peak 12%)");
    expect(dayBars(days, 7, local(30), {}).some((b) => b.today)).toBe(false);
    expect(dayBars([], 7, NOW)).toEqual([]);
  });

  it("scales bars to a round maximum", () => {
    expect(barScale([])).toBe(10);
    expect(barScale([2, 3])).toBe(10);
    expect(barScale([12.2, 3])).toBe(15);
    expect(barScale([35, Number.NaN])).toBe(35);
  });
});

describe("summaries", () => {
  const d = { from: 0, to: 4 * HOUR };

  it("finds the peak and counts resets in the domain", () => {
    const w = { points: hourly(-HOUR, [99, 20, null, 93, 40]), resets_ms: [-1, HOUR, 2 * HOUR, 5 * HOUR] };
    expect(windowSummary(w, d)).toEqual({ peak: 93, resets: 2 });
    expect(windowSummary({ points: hourly(0, [null]), resets_ms: [] }, d)).toEqual({ peak: null, resets: 0 });
  });

  it("writes the summary line", () => {
    expect(summaryText("5-hour", { peak: 92.6, resets: 4 })).toBe("5-hour peak 93% · 4 resets");
    expect(summaryText("5-hour", { peak: 12, resets: 1 })).toBe("5-hour peak 12% · 1 reset");
    expect(summaryText("Weekly", { peak: 0, resets: 0 })).toBe("Weekly peak 0% · no resets");
    expect(summaryText("5-hour", { peak: null, resets: 0 })).toBe("No 5-hour data");
  });
});

describe("rangesFor", () => {
  it("asks for the longest range and shows every range the history can fill", () => {
    expect(FETCH_DAYS * DAY).toBe(Math.max(...RANGES.map((r) => r.span)));
    expect(rangesFor(undefined)).toBe(RANGES);
    expect(rangesFor(Number.NaN)).toBe(RANGES);
    expect(rangesFor(FETCH_DAYS)).toEqual(RANGES);
    expect(rangesFor(30)).toEqual(RANGES);
  });

  it("hides ranges longer than the history keeps, but always keeps the shortest", () => {
    expect(rangesFor(7).map((r) => r.key)).toEqual(["24h", "7d"]);
    expect(rangesFor(3).map((r) => r.key)).toEqual(["24h"]);
    expect(rangesFor(0).map((r) => r.key)).toEqual(["24h"]);
  });
});
