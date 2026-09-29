import { describe, expect, it } from "vitest";
import { DEFAULT_CTX_THRESHOLDS, DEFAULT_THRESHOLDS, thresholdPair } from "./thresholds";

describe("thresholdPair", () => {
  it("shows the saved pair as it is", () => {
    expect(thresholdPair([70, 85], DEFAULT_THRESHOLDS)).toEqual([70, 85]);
    expect(thresholdPair([50, 60, 70], DEFAULT_THRESHOLDS)).toEqual([50, 60]);
  });

  it("fills a shorter list from the defaults, keeping the pair ascending", () => {
    expect(thresholdPair([], DEFAULT_THRESHOLDS)).toEqual([80, 95]);
    expect(thresholdPair(undefined, DEFAULT_CTX_THRESHOLDS)).toEqual([80, 90]);
    expect(thresholdPair([90], DEFAULT_THRESHOLDS)).toEqual([90, 95]);
    expect(thresholdPair([97], DEFAULT_THRESHOLDS)).toEqual([97, 98]);
    expect(thresholdPair([100], DEFAULT_CTX_THRESHOLDS)).toEqual([99, 100]);
  });
});
