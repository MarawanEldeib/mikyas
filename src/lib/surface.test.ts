import { describe, expect, it } from "vitest";
import { SURFACE, surfaceOf } from "./surface";

describe("surfaceOf", () => {
  it("uses the known surfaces", () => {
    expect(surfaceOf({ entrypoint: "cli", entrypoint_raw: "sdk-ts" })).toBe(SURFACE.cli);
    expect(surfaceOf({ entrypoint: "desktop" })).toBe(SURFACE.desktop);
  });
  it("names a surface it does not know after the transcript's own value", () => {
    const s = surfaceOf({ entrypoint: "unknown", entrypoint_raw: "claude-vscode" });
    expect(s).toMatchObject({ icon: SURFACE.unknown.icon, short: "Claude vscode", name: "Claude Code (claude-vscode)" });
    expect(surfaceOf({ entrypoint: "unknown", entrypoint_raw: null })).toBe(SURFACE.unknown);
    expect(surfaceOf({ entrypoint: "unknown", entrypoint_raw: "  " })).toBe(SURFACE.unknown);
  });
});
