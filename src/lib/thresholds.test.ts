import { describe, expect, it } from "vitest";
import {
  DEFAULT_CTX_THRESHOLDS,
  DEFAULT_THRESHOLDS,
  addThreshold,
  normalizeThresholds,
  removeThreshold,
  setThreshold,
  thresholdBounds,
} from "./thresholds";

describe("thresholds list", () => {
  it("keeps any number of values, sorted and without repeats", () => {
    expect(normalizeThresholds([50, 60, 70])).toEqual([50, 60, 70]);
    expect(normalizeThresholds([90, 5, 90, 0, 101, 2.5])).toEqual([5, 90]);
    expect(normalizeThresholds([])).toEqual([]);
  });

  it("edits one value between its neighbours, the others untouched", () => {
    expect(setThreshold([50, 60, 70], 2, 75)).toEqual([50, 60, 75]);
    expect(setThreshold([5], 0, 3)).toEqual([3]);
    expect(thresholdBounds([50, 60, 70], 1)).toEqual([51, 69]);
    expect(thresholdBounds([50, 60, 70], 0)).toEqual([1, 59]);
    expect(thresholdBounds([50, 60, 70], 2)).toEqual([61, 100]);
    expect(thresholdBounds([5], 0)).toEqual([1, 100]);
  });

  it("removes down to none (no alerts)", () => {
    expect(removeThreshold([50, 60, 70], 1)).toEqual([50, 70]);
    expect(removeThreshold([80], 0)).toEqual([]);
  });

  it("adds a default first, then above the highest, then into the widest gap", () => {
    expect(addThreshold([], DEFAULT_THRESHOLDS)).toEqual([80]);
    expect(addThreshold([80], DEFAULT_THRESHOLDS)).toEqual([80, 95]);
    expect(addThreshold([80, 95], DEFAULT_THRESHOLDS)).toEqual([80, 95, 100]);
    expect(addThreshold([80, 90, 100], DEFAULT_CTX_THRESHOLDS)).toEqual([40, 80, 90, 100]);
    const full = Array.from({ length: 100 }, (_, i) => i + 1);
    expect(addThreshold(full, DEFAULT_THRESHOLDS)).toBeNull();
  });
});
