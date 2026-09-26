import { describe, expect, it } from "vitest";
import { WORKED_SINCE_TIP, showWorkedSince } from "./worked";

describe("showWorkedSince", () => {
  const w = { worked_since: true, limit_reached: false, phase: "active" as const };
  it("follows the snapshot flag", () => {
    expect(showWorkedSince(w)).toBe(true);
    expect(showWorkedSince({ ...w, worked_since: false })).toBe(false);
  });
  it("is hidden on a reached limit and while awaiting data after a reset", () => {
    expect(showWorkedSince({ ...w, limit_reached: true })).toBe(false);
    expect(showWorkedSince({ ...w, phase: "reset_awaiting_data" })).toBe(false);
  });
  it("uses the agreed tooltip", () => {
    expect(WORKED_SINCE_TIP).toBe("Claude has worked since this reading, so the real value is higher");
  });
});
