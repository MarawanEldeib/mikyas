import { describe, expect, it } from "vitest";
import { CTX_MAX, CTX_MIN, CTX_PRESETS, formatCtxSize, normalizeModelId, parseCtxSize } from "./ctxSize";

describe("parseCtxSize", () => {
  it("accepts plain numbers, separators and K/M suffixes", () => {
    expect(parseCtxSize("400000")).toBe(400_000);
    expect(parseCtxSize("400,000")).toBe(400_000);
    expect(parseCtxSize(" 400 000 ")).toBe(400_000);
    expect(parseCtxSize("400_000")).toBe(400_000);
    expect(parseCtxSize("400K")).toBe(400_000);
    expect(parseCtxSize("400k")).toBe(400_000);
    expect(parseCtxSize("1M")).toBe(1_000_000);
    expect(parseCtxSize("1.5m")).toBe(1_500_000);
    expect(parseCtxSize("2 M")).toBe(2_000_000);
    expect(parseCtxSize("128.5K")).toBe(128_500);
  });

  it("enforces the 1K..100M bounds", () => {
    expect(parseCtxSize("1K")).toBe(CTX_MIN);
    expect(parseCtxSize("999")).toBeNull();
    expect(parseCtxSize("100M")).toBe(CTX_MAX);
    expect(parseCtxSize("100000001")).toBeNull();
    expect(parseCtxSize("0")).toBeNull();
  });

  it("rejects anything that is not a size", () => {
    for (const bad of ["", "abc", "-5K", "1e6", "1.2.3M", "5G", "K", "1MM", "0x10"]) {
      expect(parseCtxSize(bad), bad).toBeNull();
    }
  });
});

describe("formatCtxSize", () => {
  it("keeps the exact value in a short form", () => {
    expect(formatCtxSize(1_000_000)).toBe("1M");
    expect(formatCtxSize(1_500_000)).toBe("1.5M");
    expect(formatCtxSize(200_000)).toBe("200K");
    expect(formatCtxSize(128_500)).toBe("128500");
    expect(formatCtxSize(1_234_567)).toBe("1234567");
    expect(formatCtxSize(1_250_000)).toBe("1250K");
    expect(formatCtxSize(0)).toBe("0");
  });

  it("round-trips through parseCtxSize", () => {
    for (const n of [...CTX_PRESETS, CTX_MIN, CTX_MAX, 400_000, 1_500_000, 123_456, 2_500_000]) {
      expect(parseCtxSize(formatCtxSize(n))).toBe(n);
    }
  });
});

describe("normalizeModelId", () => {
  it("trims and drops the [1m] suffix", () => {
    expect(normalizeModelId(" claude-sonnet-5[1M] ")).toBe("claude-sonnet-5");
    expect(normalizeModelId("claude-opus-5-5")).toBe("claude-opus-5-5");
  });

  it("accepts provider model ids (Bedrock, Vertex, gateways)", () => {
    for (const id of ["us.anthropic.claude-opus-5-5-v1:0", "claude-opus-5-5@20260901", "anthropic/claude-opus-5-5"]) {
      expect(normalizeModelId(id)).toBe(id);
    }
    expect(normalizeModelId("us.anthropic.claude-opus-5-5-v1:0[1m]")).toBe("us.anthropic.claude-opus-5-5-v1:0");
    expect(normalizeModelId("x".repeat(129))).toBeNull();
  });

  it("rejects what is not a model id", () => {
    expect(normalizeModelId("not a model!")).toBeNull();
    expect(normalizeModelId("")).toBeNull();
    expect(normalizeModelId("[1m]")).toBeNull();
  });
});
