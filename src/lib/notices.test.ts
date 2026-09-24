import { describe, expect, it } from "vitest";
import { notices } from "./notices";
import type { Snapshot } from "./types";

const base: Snapshot = {
  generated_ms: 0,
  windows: [],
  session: null,
  health: { desktop: { state: "ok", last_sample_ms: null }, cli_last_capture_ms: null, transcripts_last_activity_ms: null },
  warnings: [],
};

describe("notices", () => {
  it("is empty without warnings", () => {
    expect(notices(null)).toEqual([]);
    expect(notices(base)).toEqual([]);
  });

  it("lists explicit warnings, then Desktop health problems", () => {
    const s: Snapshot = {
      ...base,
      warnings: [{ type: "no_plan_limits" }, { type: "account_mismatch" }],
      health: { ...base.health, desktop: { state: "schema_changed", version: 3 } },
    };
    const ids = notices(s).map((n) => n.id);
    expect(ids).toEqual(["no_plan_limits", "account_mismatch", "schema_changed"]);
    expect(notices(s)[2].detail).toContain("v3");
  });

  it("reports an unreadable Desktop file", () => {
    const s: Snapshot = { ...base, health: { ...base.health, desktop: { state: "unreadable" } } };
    expect(notices(s).map((n) => n.id)).toEqual(["unreadable"]);
  });
});
