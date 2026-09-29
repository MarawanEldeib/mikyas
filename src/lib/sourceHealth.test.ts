import { describe, expect, it } from "vitest";
import { formatAge } from "./format";
import { desktopHealthLine } from "./sourceHealth";

const NOW = 1_790_208_000_000;
const MIN = 60_000;

describe("desktopHealthLine", () => {
  it("describes a supported file", () => {
    expect(desktopHealthLine({ state: "ok", last_sample_ms: NOW - 6 * MIN }, NOW)).toEqual({
      tone: "ok",
      text: `Found · sample ${formatAge(NOW - 6 * MIN, NOW)}`,
    });
    expect(desktopHealthLine({ state: "ok", last_sample_ms: null, newer_version: null }, NOW)).toEqual({
      tone: "ok",
      text: "Found · no samples yet",
    });
  });

  it("says when a newer format is read best-effort", () => {
    const line = desktopHealthLine({ state: "ok", last_sample_ms: NOW - 6 * MIN, newer_version: 3 }, NOW);
    expect(line.tone).toBe("warn");
    expect(line.text).toBe(`Newer format (v3), read best-effort · sample ${formatAge(NOW - 6 * MIN, NOW)}`);
    expect(desktopHealthLine({ state: "ok", last_sample_ms: null, newer_version: 4 }, NOW).text).toBe(
      "Newer format (v4), read best-effort · no samples yet",
    );
  });

  it("covers the other states", () => {
    expect(desktopHealthLine({ state: "schema_changed", version: 3 }, NOW)).toEqual({ tone: "warn", text: "Unsupported format (v3)" });
    expect(desktopHealthLine({ state: "unreadable" }, NOW)).toEqual({ tone: "crit", text: "Found, but can't be read" });
    expect(desktopHealthLine({ state: "not_found" }, NOW)).toEqual({ tone: "off", text: "Not found" });
    expect(desktopHealthLine(undefined, NOW)).toEqual({ tone: "off", text: "Not found" });
  });
});
