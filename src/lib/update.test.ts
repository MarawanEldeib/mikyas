import { afterEach, describe, expect, it, vi } from "vitest";
import { MOCK_UPDATE, createMockBackend } from "./mock";
import type { UiState, UpdateInfo } from "./types";
import { APP_VERSION, bannerVisible, loadDismissed, saveDismissed, updateStatus } from "./update";

const UPDATE: UpdateInfo = { version: "0.2.0", url: "https://github.com/MarawanEldeib/claude-usage-widget/releases/tag/v0.2.0" };

const ui = (over: Partial<UiState> = {}): UiState => ({
  view: "card",
  pinned: true,
  click_through: false,
  hotkey_error: null,
  toggle_hotkey_error: null,
  dock_expanded: false,
  hidden_reason: "none",
  update: UPDATE,
  ...over,
});

describe("bannerVisible", () => {
  it("shows a known update on the card and the pill", () => {
    expect(bannerVisible(ui(), "off", null)).toBe(true);
    expect(bannerVisible(ui({ view: "pill" }), "off", null)).toBe(true);
    expect(bannerVisible(ui({ update: null }), "off", null)).toBe(false);
  });

  it("stays out of the other views, ghost mode and the docked strip", () => {
    for (const view of ["settings", "sessions", "history"] as const) {
      expect(bannerVisible(ui({ view }), "off", null)).toBe(false);
    }
    expect(bannerVisible(ui({ click_through: true }), "off", null)).toBe(false);
    expect(bannerVisible(ui(), "right", null)).toBe(false);
    expect(bannerVisible(ui({ dock_expanded: true }), "right", null)).toBe(true);
  });

  it("never covers the card footer's health warnings (the pill's warning dot sits elsewhere)", () => {
    expect(bannerVisible(ui(), "off", null, true)).toBe(false);
    expect(bannerVisible(ui({ view: "pill" }), "off", null, true)).toBe(true);
    expect(bannerVisible(ui(), "off", null, false)).toBe(true);
  });

  it("respects a dismissal of that version only", () => {
    expect(bannerVisible(ui(), "off", "0.2.0")).toBe(false);
    expect(bannerVisible(ui({ update: { ...UPDATE, version: "0.3.0" } }), "off", "0.2.0")).toBe(true);
  });
});

describe("dismissal storage", () => {
  it("round-trips through storage", () => {
    const map = new Map<string, string>();
    const store = { getItem: (k: string) => map.get(k) ?? null, setItem: (k: string, v: string) => void map.set(k, v) };
    expect(loadDismissed(store)).toBeNull();
    saveDismissed("0.2.0", store);
    expect(loadDismissed(store)).toBe("0.2.0");
  });

  it("never throws when storage is missing or blocked", () => {
    const blocked = {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("QuotaExceededError");
      },
    };
    expect(loadDismissed(blocked)).toBeNull();
    expect(() => saveDismissed("0.2.0", blocked)).not.toThrow();
    expect(loadDismissed(null)).toBeNull();
    expect(() => saveDismissed("0.2.0", null)).not.toThrow();
  });
});

describe("updateStatus", () => {
  it("orders checking, failure, a known update, then up to date", () => {
    expect(updateStatus({ state: "idle" }, null)).toBeNull();
    expect(updateStatus({ state: "checking" }, UPDATE)).toBeNull();
    expect(updateStatus({ state: "error", message: "No releases published yet" }, UPDATE)).toEqual({
      tone: "warn",
      text: "No releases published yet",
      url: null,
    });
    expect(updateStatus({ state: "idle" }, UPDATE)).toEqual({ tone: "info", text: "Version 0.2.0 available", url: UPDATE.url });
    expect(updateStatus({ state: "done" }, UPDATE)?.tone).toBe("info");
    expect(updateStatus({ state: "done" }, null)).toEqual({ tone: "ok", text: "Up to date", url: null });
  });

  it("knows this build's version", () => {
    expect(APP_VERSION).toMatch(/^\d+\.\d+\.\d+/);
  });
});

describe("mock update and hidden params", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts with a known update and a hidden reason when asked", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("update=1&hidden=fullscreen"));
    const state = await b.invoke<UiState>("get_ui_state");
    expect(state.update).toEqual(MOCK_UPDATE);
    expect(state.hidden_reason).toBe("fullscreen");
    const plain = await createMockBackend(new URLSearchParams("hidden=bogus")).invoke<UiState>("get_ui_state");
    expect(plain.update).toBeNull();
    expect(plain.hidden_reason).toBe("none");
  });

  it("answers Check now per mode", async () => {
    vi.useFakeTimers();
    const run = async (query: string) => {
      const b = createMockBackend(new URLSearchParams(query));
      const events: UiState[] = [];
      await b.listen<UiState>("ui-state", (u) => events.push(u));
      const result = b.invoke<UpdateInfo | null>("check_updates_now");
      await vi.advanceTimersByTimeAsync(500);
      return { result: await result, events };
    };
    const found = await run("");
    expect(found.result).toEqual(MOCK_UPDATE);
    expect(found.events.at(-1)?.update).toEqual(MOCK_UPDATE);
    expect((await run("update=none")).result).toBeNull();

    const b = createMockBackend(new URLSearchParams("update=error"));
    const failing = b.invoke("check_updates_now");
    const caught = failing.catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(500);
    expect(await caught).toMatch(/Couldn't reach GitHub/);
  });
});
