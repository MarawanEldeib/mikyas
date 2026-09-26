import { describe, expect, it, vi } from "vitest";
import { connectionBannerVisible, reconnect } from "./connection";
import type { UiState } from "./types";

const ui: UiState = {
  view: "card",
  pinned: true,
  click_through: false,
  hotkey_error: null,
  toggle_hotkey_error: null,
  dock_expanded: false,
  hidden_reason: "none",
  update: null,
  connection_lost: true,
};

describe("connectionBannerVisible", () => {
  it("shows while the connection is lost and the watchdog is on", () => {
    expect(connectionBannerVisible(ui, true)).toBe(true);
    expect(connectionBannerVisible({ ...ui, connection_lost: false }, true)).toBe(false);
    expect(connectionBannerVisible(ui, false)).toBe(false);
  });

  it("hides in click-through mode (its buttons could not be used)", () => {
    expect(connectionBannerVisible({ ...ui, click_through: true }, true)).toBe(false);
  });
});

describe("reconnect", () => {
  it("connects for real, then clears the banner", async () => {
    const calls: string[] = [];
    const api = {
      connectClaudeCode: vi.fn(async (dryRun: boolean) => void calls.push(`connect:${dryRun}`)),
      dismissConnectionWarning: vi.fn(async () => void calls.push("dismiss")),
    };
    expect(await reconnect(api)).toEqual({ state: "idle" });
    expect(calls).toEqual(["connect:false", "dismiss"]);
  });

  it("reports a failed connect and does not dismiss", async () => {
    const api = {
      connectClaudeCode: vi.fn(async () => {
        throw "settings.json keeps changing; try again in a moment";
      }),
      dismissConnectionWarning: vi.fn(async () => {}),
    };
    expect(await reconnect(api)).toEqual({ state: "error", message: "settings.json keeps changing; try again in a moment" });
    expect(api.dismissConnectionWarning).not.toHaveBeenCalled();
    const thrown = { connectClaudeCode: async () => Promise.reject(new Error("boom")), dismissConnectionWarning: async () => {} };
    expect(await reconnect(thrown)).toEqual({ state: "error", message: "boom" });
  });

  it("still succeeds when only the dismiss fails", async () => {
    const api = {
      connectClaudeCode: async () => {},
      dismissConnectionWarning: async () => Promise.reject(new Error("gone")),
    };
    expect(await reconnect(api)).toEqual({ state: "idle" });
  });
});
