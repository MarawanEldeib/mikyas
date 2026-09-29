import { describe, expect, it } from "vitest";
import { plural } from "./plural";

describe("plural", () => {
  it("uses the singular only for exactly one", () => {
    expect(plural(1, "warning")).toBe("1 warning");
    expect(plural(0, "warning")).toBe("0 warnings");
    expect(plural(2, "reset")).toBe("2 resets");
    expect(plural(1.5, "hour")).toBe("1.5 hours");
  });

  it("takes an irregular plural", () => {
    expect(plural(1, "entry", "entries")).toBe("1 entry");
    expect(plural(3, "entry", "entries")).toBe("3 entries");
  });
});
