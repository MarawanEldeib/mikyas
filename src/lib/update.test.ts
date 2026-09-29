import { afterEach, describe, expect, it, vi } from "vitest";
import { MOCK_UPDATE, MOCK_UPDATE_ONE, createMockBackend } from "./mock";
import type { UiState, UpdateInfo } from "./types";
import { APP_VERSION, MAX_NOTES, MAX_NOTE_CHARS, bannerText, bannerVisible, latestUrl, updateRows, updateStatus } from "./update";

const page = (v: string) => `https://github.com/MarawanEldeib/mikyas/releases/tag/v${v}`;
const ONE: UpdateInfo = { latest: "0.2.0", count: 1, releases: [{ version: "0.2.0", url: page("0.2.0"), notes: ["a"] }], dismissed: false };
const THREE: UpdateInfo = {
  latest: "0.4.0",
  count: 3,
  releases: [
    { version: "0.4.0", url: page("0.4.0"), notes: ["x"] },
    { version: "0.3.1", url: page("0.3.1"), notes: [] },
    { version: "0.3.0", url: page("0.3.0"), notes: ["y", "z"] },
  ],
  dismissed: false,
};

const ui = (over: Partial<UiState> = {}): UiState => ({
  view: "card",
  pinned: true,
  click_through: false,
  hotkey_error: null,
  toggle_hotkey_error: null,
  dock_expanded: false,
  hidden_reason: "none",
  update: ONE,
  connection_lost: false,
  ...over,
});

describe("bannerVisible", () => {
  it("shows known updates on the card and the pill", () => {
    expect(bannerVisible(ui(), "off")).toBe(true);
    expect(bannerVisible(ui({ view: "pill" }), "off")).toBe(true);
    expect(bannerVisible(ui({ update: THREE }), "off")).toBe(true);
    expect(bannerVisible(ui({ update: null }), "off")).toBe(false);
  });

  it("stays out of the other views, ghost mode and the docked strip", () => {
    for (const view of ["settings", "sessions", "history"] as const) {
      expect(bannerVisible(ui({ view }), "off")).toBe(false);
    }
    expect(bannerVisible(ui({ click_through: true }), "off")).toBe(false);
    expect(bannerVisible(ui(), "right")).toBe(false);
    expect(bannerVisible(ui({ dock_expanded: true }), "right")).toBe(true);
  });

  it("never covers the card footer's health warnings (the pill's warning dot sits elsewhere)", () => {
    expect(bannerVisible(ui(), "off", true)).toBe(false);
    expect(bannerVisible(ui({ view: "pill" }), "off", true)).toBe(true);
    expect(bannerVisible(ui(), "off", false)).toBe(true);
  });

  it("hides after Later until Rust reports a newer version (dismissed is cleared then)", () => {
    expect(bannerVisible(ui({ update: { ...THREE, dismissed: true } }), "off")).toBe(false);
    expect(bannerVisible(ui({ view: "pill", update: { ...ONE, dismissed: true } }), "off")).toBe(false);
  });
});

describe("banner and list", () => {
  it("names one update or counts several", () => {
    expect(bannerText(ONE)).toBe("Update available: v0.2.0");
    expect(bannerText(THREE)).toBe("3 updates available");
    expect(bannerText(ONE, true)).toBe("v0.2.0 available");
    expect(bannerText(THREE, true)).toBe("3 updates");
  });

  it("Update opens the latest release page only", () => {
    expect(latestUrl(THREE)).toBe(page("0.4.0"));
    expect(latestUrl(null)).toBeNull();
    expect(latestUrl({ ...ONE, releases: [] })).toBeNull();
    expect(latestUrl({ ...ONE, releases: [{ version: "9.9.9", url: "https://example.com/x", notes: [] }] })).toBeNull();
  });

  it("lists every missed version newest first with its notes", () => {
    const rows = updateRows(THREE);
    expect(rows.map((r) => r.version)).toEqual(["0.4.0", "0.3.1", "0.3.0"]);
    expect(rows.map((r) => r.latest)).toEqual([true, false, false]);
    expect(rows[1].notes).toEqual([]);
    expect(rows[2].notes).toEqual(["y", "z"]);
    expect(updateRows(null)).toEqual([]);
  });

  it("keeps notes tiny: at most five, trimmed, clipped, blanks dropped", () => {
    const notes = ["  one ", "", "two", "three", "four", "five", "six", "ä".repeat(120)];
    const [row] = updateRows({ ...ONE, releases: [{ version: "0.2.0", url: page("0.2.0"), notes }] });
    expect(row.notes).toEqual(["one", "two", "three", "four", "five"]);
    const [long] = updateRows({ ...ONE, releases: [{ version: "0.2.0", url: page("0.2.0"), notes: ["ä".repeat(120)] }] });
    expect([...long.notes[0]].length).toBe(MAX_NOTE_CHARS);
    expect(long.notes[0].endsWith("…")).toBe(true);
  });
});

describe("updateStatus", () => {
  it("orders checking, failure, known updates, then up to date", () => {
    expect(updateStatus({ state: "idle" }, null)).toBeNull();
    expect(updateStatus({ state: "checking" }, ONE)).toBeNull();
    expect(updateStatus({ state: "error", message: "No releases published yet" }, ONE)).toEqual({
      tone: "warn",
      text: "No releases published yet",
    });
    expect(updateStatus({ state: "idle" }, ONE)).toEqual({ tone: "info", text: "Version 0.2.0 available" });
    expect(updateStatus({ state: "idle" }, THREE)).toEqual({ tone: "info", text: "3 updates available (newest 0.4.0)" });
    expect(updateStatus({ state: "done" }, { ...ONE, dismissed: true })?.tone).toBe("info");
    expect(updateStatus({ state: "done" }, null)).toEqual({ tone: "ok", text: "Up to date" });
  });

  it("knows this build's version", () => {
    expect(APP_VERSION).toMatch(/^\d+\.\d+\.\d+/);
  });
});

describe("mock update and hidden params", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts with known updates and a hidden reason when asked", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("update=1&hidden=fullscreen"));
    const state = await b.invoke<UiState>("get_ui_state");
    expect(state.update).toEqual(MOCK_UPDATE);
    expect(state.hidden_reason).toBe("fullscreen");
    expect((await createMockBackend(new URLSearchParams("update=one")).invoke<UiState>("get_ui_state")).update).toEqual(MOCK_UPDATE_ONE);
    const plain = await createMockBackend(new URLSearchParams("hidden=bogus")).invoke<UiState>("get_ui_state");
    expect(plain.update).toBeNull();
    expect(plain.hidden_reason).toBe("none");
  });

  it("mock data looks like what Rust sends", () => {
    for (const u of [MOCK_UPDATE, MOCK_UPDATE_ONE]) {
      expect(u.count).toBe(u.releases.length);
      expect(u.latest).toBe(u.releases[0].version);
      expect(latestUrl(u)).not.toBeNull();
      expect(u.releases.every((r) => r.notes.length <= MAX_NOTES && r.notes.every((n) => n.length <= MAX_NOTE_CHARS))).toBe(true);
    }
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

  it("Later hides the banner and survives another check", async () => {
    vi.useFakeTimers();
    const b = createMockBackend(new URLSearchParams("update=1"));
    await b.invoke("dismiss_update", { version: MOCK_UPDATE.latest });
    const state = await b.invoke<UiState>("get_ui_state");
    expect(state.update?.dismissed).toBe(true);
    expect(bannerVisible(state, "off")).toBe(false);
    const again = b.invoke<UpdateInfo | null>("check_updates_now");
    await vi.advanceTimersByTimeAsync(500);
    expect((await again)?.dismissed).toBe(true);
  });
});
