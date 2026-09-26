import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionStatus, Settings, Snapshot, UiState } from "./types";

const listeners: { snapshot?: (s: Snapshot) => void; ui?: (u: UiState) => void } = {};
const api = {
  getSnapshot: vi.fn(),
  getSettings: vi.fn(),
  getUiState: vi.fn(),
  connectionStatus: vi.fn(),
  setView: vi.fn(),
  setPinned: vi.fn(),
  updateSettings: vi.fn(),
  disconnectClaudeCode: vi.fn(),
};
vi.mock("./ipc", () => ({
  inTauri: false,
  api,
  onSnapshot: async (cb: (s: Snapshot) => void) => {
    listeners.snapshot = cb;
    return () => delete listeners.snapshot;
  },
  onUiState: async (cb: (u: UiState) => void) => {
    listeners.ui = cb;
    return () => delete listeners.ui;
  },
}));

// vitest runs without the Svelte compiler (vitest.config.ts): plain values stand in for the runes,
// which is enough for the store's logic (reactivity is not under test here).
const rune = <T>(v: T): T => v;
vi.stubGlobal("$state", Object.assign(rune, { raw: rune }));
const { AppState } = await import("./stores.svelte");

const UI: UiState = {
  view: "card",
  pinned: true,
  click_through: false,
  hotkey_error: null,
  toggle_hotkey_error: null,
  dock_expanded: false,
  hidden_reason: "none",
  update: null,
  connection_lost: false,
};
const SETTINGS = { view: "card", opacity: 1 } as unknown as Settings;
const snap = (tag: number) => ({ tag, windows: [] }) as unknown as Snapshot;
const conn = (state: string) => ({ state }) as unknown as ConnectionStatus;

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.resetAllMocks();
  api.getSnapshot.mockResolvedValue(snap(1));
  api.getSettings.mockResolvedValue(SETTINGS);
  api.getUiState.mockResolvedValue(UI);
  api.connectionStatus.mockResolvedValue(conn("connected"));
  api.setView.mockResolvedValue(undefined);
  api.setPinned.mockResolvedValue(undefined);
});

describe("init", () => {
  it("keeps state pushed while the initial fetch was in flight", async () => {
    const fetched = deferred<Snapshot>();
    api.getSnapshot.mockReturnValue(fetched.promise);
    const app = new AppState();
    const done = app.init();
    await vi.waitFor(() => expect(listeners.ui).toBeDefined());
    listeners.snapshot?.(snap(2));
    listeners.ui?.({ ...UI, view: "pill" });
    fetched.resolve(snap(1));
    await done;
    expect(app.snapshot).toEqual(snap(2));
    expect(app.ui.view).toBe("pill");
    app.dispose();
  });

  it("can be retried after a failure", async () => {
    api.getSettings.mockRejectedValueOnce(new Error("boom"));
    const app = new AppState();
    await app.init();
    expect(app.ready).toBe(false);
    expect(app.error).toBe("boom");
    await app.init();
    expect(app.ready).toBe(true);
    expect(app.error).toBeNull();
    app.dispose();
  });
});

describe("optimistic changes", () => {
  it("rolls the pin back when the command fails", async () => {
    const app = new AppState();
    await app.init();
    api.setPinned.mockRejectedValueOnce(new Error("nope"));
    await app.togglePinned();
    expect(app.ui.pinned).toBe(true);
    expect(app.error).toBe("nope");
    app.dispose();
  });

  it("rolls a failed settings change back", async () => {
    const app = new AppState();
    await app.init();
    api.updateSettings.mockRejectedValueOnce(new Error("nope"));
    await app.updateSettings({ opacity: 0.5 });
    expect(app.settings?.opacity).toBe(1);
    app.dispose();
  });

  it("does not let an older response overwrite a newer change", async () => {
    const app = new AppState();
    await app.init();
    const first = deferred<Settings>();
    const second = deferred<Settings>();
    api.updateSettings.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const a = app.updateSettings({ opacity: 0.5 });
    const b = app.updateSettings({ opacity: 0.7 });
    first.resolve({ ...SETTINGS, opacity: 0.5 });
    await a;
    expect(app.settings?.opacity).toBe(0.7);
    second.resolve({ ...SETTINGS, opacity: 0.7 });
    await b;
    expect(app.settings?.opacity).toBe(0.7);
    app.dispose();
  });
});

describe("connection status", () => {
  it("refreshes when Settings opens and when the watchdog flips", async () => {
    const app = new AppState();
    await app.init();
    api.connectionStatus.mockResolvedValue(conn("foreign"));
    await app.setView("settings");
    await vi.waitFor(() => expect(app.connection).toEqual(conn("foreign")));
    api.connectionStatus.mockResolvedValue(conn("not_configured"));
    listeners.ui?.({ ...app.ui, connection_lost: true });
    await vi.waitFor(() => expect(app.connection).toEqual(conn("not_configured")));
    app.dispose();
  });

  it("disconnects through the store and re-reads the status even on failure", async () => {
    const app = new AppState();
    await app.init();
    api.disconnectClaudeCode.mockRejectedValueOnce(new Error("locked"));
    api.connectionStatus.mockResolvedValue(conn("not_configured"));
    await expect(app.disconnect()).rejects.toThrow("locked");
    expect(app.connection).toEqual(conn("not_configured"));
    app.dispose();
  });
});
