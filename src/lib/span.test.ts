import { describe, expect, it } from "vitest";
import { durationOf, isUnscoped, mainKinds, spanLabel, spanShort } from "./span";

const H = 3_600_000;
const D = 24 * H;

describe("spanOf / durationOf", () => {
  it("reads any <number>_<unit> key, as Rust does", () => {
    expect(durationOf("five_hour")).toBe(5 * H);
    expect(durationOf("seven_day_opus")).toBe(7 * D);
    expect(durationOf("2_week")).toBe(14 * D);
    expect(durationOf("twenty_four_hour")).toBe(24 * H);
    expect(durationOf("twenty-four_hour")).toBe(24 * H);
    expect(durationOf("ninety_day_opus")).toBe(90 * D);
    expect(durationOf("one_month")).toBe(30 * D);
    expect(durationOf("thirty_minute")).toBe(30 * 60_000);
    expect(durationOf("daily")).toBe(D);
    expect(durationOf("weekly_opus")).toBe(7 * D);
    for (const unknown of [
      "spend_limit",
      "x",
      "",
      "zero_day",
      "seven",
      "_day",
      "1000_day",
      "day_seven",
      "twenty_ten_day",
      "constructor_day",
    ]) {
      expect(durationOf(unknown), unknown).toBeNull();
    }
  });

  it("names spans and scopes", () => {
    expect(spanLabel("seven_day_opus")).toBe("weekly Opus");
    expect(spanShort("thirty_day")).toBe("30d");
    expect(spanLabel("spend_limit")).toBe("spend limit");
    expect(isUnscoped("thirty_day")).toBe(true);
    expect(isUnscoped("seven_day_opus")).toBe(false);
    expect(isUnscoped("spend_limit")).toBe(false);
  });
});

describe("mainKinds", () => {
  it("takes the shortest and longest unscoped windows from the data", () => {
    expect(mainKinds(["five_hour", "seven_day", "seven_day_opus"])).toEqual(["five_hour", "seven_day"]);
    expect(mainKinds(["five_hour", "weekly", "seven_day_newmodel"])).toEqual(["five_hour", "weekly"]);
    expect(mainKinds(["session_x", "4_hour", "thirty_day", "seven_day"])).toEqual(["4_hour", "thirty_day"]);
    expect(mainKinds(["thirty_day"])).toEqual(["thirty_day"]);
  });
  it("prefers the built-in keys on ties and falls back sensibly", () => {
    expect(mainKinds(["1_week", "seven_day", "5_hour", "five_hour"])).toEqual(["five_hour", "seven_day"]);
    expect(mainKinds(["seven_day_opus", "five_hour_opus"])).toEqual(["five_hour_opus", "seven_day_opus"]);
    expect(mainKinds(["spend_limit", "x", "y"])).toEqual(["spend_limit", "x"]);
    expect(mainKinds([])).toEqual([]);
  });
});
