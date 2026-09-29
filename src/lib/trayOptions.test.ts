import { describe, expect, it } from "vitest";
import { trayOptions } from "./trayOptions";

const w = (kind: string, label?: string) => ({ kind, label });

describe("trayOptions", () => {
  it("offers every limit in the snapshot, main ones first", () => {
    const opts = trayOptions([w("seven_day_newmodel"), w("seven_day"), w("five_hour")], "worst");
    expect(opts.map((o) => o.value)).toEqual(["worst", "five_hour", "seven_day", "seven_day_newmodel", "off"]);
    expect(opts.map((o) => o.label)).toEqual(["Highest limit", "5-hour", "Weekly", "Weekly Newmodel", "Off (dot)"]);
  });

  it("uses Rust's names and new keys", () => {
    expect(trayOptions([w("thirty_day", "30-day")], "worst")[1]).toEqual({ value: "thirty_day", label: "30-day" });
  });

  it("keeps the saved choice listed while that limit is absent", () => {
    const opts = trayOptions([w("five_hour")], "seven_day_opus");
    expect(opts.map((o) => o.value)).toEqual(["worst", "five_hour", "seven_day_opus", "off"]);
    expect(opts[2].label).toBe("Weekly Opus (not reported now)");
    expect(trayOptions([], "off").map((o) => o.value)).toEqual(["worst", "off"]);
  });
});
