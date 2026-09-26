import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  COLLAPSE_DELAY_MS,
  bindPointer,
  canCollapse,
  canExpand,
  createDockController,
  dockState,
  startDock,
  type DockState,
} from "./dock";
import type { ViewMode } from "./types";

function setup(initial: Partial<DockState> = {}) {
  const state: DockState = { docked: true, expanded: false, clickThrough: false, view: "card", ...initial };
  const calls: boolean[] = [];
  const ctl = createDockController({
    state: () => state,
    setExpanded: (v) => {
      calls.push(v);
      state.expanded = v;
    },
  });
  return { state, calls, ctl };
}

describe("dock rules", () => {
  it("derives the state from the app store", () => {
    const ui = { dock_expanded: true, click_through: false, view: "pill" as ViewMode };
    expect(dockState({ settings: { dock: "left" }, ui })).toEqual({ docked: true, expanded: true, clickThrough: false, view: "pill" });
    expect(dockState({ settings: null, ui }).docked).toBe(false);
    expect(dockState({ settings: { dock: "off" }, ui }).docked).toBe(false);
  });

  it("never slides out in click-through, and only the pill and card slide in", () => {
    const base: DockState = { docked: true, expanded: false, clickThrough: false, view: "card" };
    expect(canExpand(base)).toBe(true);
    expect(canExpand({ ...base, clickThrough: true })).toBe(false);
    expect(canExpand({ ...base, docked: false })).toBe(false);
    expect(canExpand({ ...base, expanded: true })).toBe(false);
    const out = { ...base, expanded: true };
    expect(canCollapse(out)).toBe(true);
    expect(canCollapse({ ...out, view: "pill" })).toBe(true);
    for (const view of ["settings", "sessions", "history"] as const) expect(canCollapse({ ...out, view })).toBe(false);
    expect(canCollapse({ ...out, docked: false })).toBe(false);
  });
});

describe("createDockController", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("slides out immediately and back in after the grace period", () => {
    const { calls, ctl } = setup();
    ctl.expand();
    expect(calls).toEqual([true]);
    ctl.expand();
    expect(calls).toEqual([true]);

    ctl.leave();
    expect(ctl.pending).toBe(true);
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS - 1);
    expect(calls).toEqual([true]);
    vi.advanceTimersByTime(1);
    expect(calls).toEqual([true, false]);
    expect(ctl.pending).toBe(false);
  });

  it("stays out when the pointer comes back in time", () => {
    const { calls, ctl } = setup({ expanded: true });
    ctl.leave();
    vi.advanceTimersByTime(300);
    ctl.hold();
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS * 2);
    expect(calls).toEqual([]);
    // A second leave restarts the full delay.
    ctl.leave();
    vi.advanceTimersByTime(300);
    ctl.leave();
    vi.advanceTimersByTime(300);
    expect(calls).toEqual([]);
    vi.advanceTimersByTime(150);
    expect(calls).toEqual([false]);
  });

  it("rechecks when the timer fires", () => {
    const { state, calls, ctl } = setup({ expanded: true });
    ctl.leave();
    state.view = "settings";
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS);
    expect(calls).toEqual([]);
  });

  it("does nothing when not docked, in click-through or in Settings", () => {
    const off = setup({ docked: false });
    off.ctl.expand();
    expect(off.calls).toEqual([]);
    const ghost = setup({ clickThrough: true });
    ghost.ctl.expand();
    expect(ghost.calls).toEqual([]);
    const settings = setup({ expanded: true, view: "settings" });
    settings.ctl.leave();
    expect(settings.ctl.pending).toBe(false);
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS);
    expect(settings.calls).toEqual([]);
  });

  it("dispose cancels a pending slide-in", () => {
    const { calls, ctl } = setup({ expanded: true });
    ctl.leave();
    ctl.dispose();
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS);
    expect(calls).toEqual([]);
  });
});

class FakePointerEvent extends Event {
  constructor(
    type: string,
    readonly relatedTarget: EventTarget | null,
  ) {
    super(type);
  }
}

describe("bindPointer", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("holds while over the stage and leaves only when the pointer exits it", () => {
    const doc = new EventTarget();
    const child = new EventTarget();
    const outside = new EventTarget();
    const { calls, ctl } = setup({ expanded: true });
    const unbind = bindPointer(doc, (t) => t === doc || t === child, ctl);

    // Moving between elements inside the stage is not a leave.
    doc.dispatchEvent(new FakePointerEvent("pointerout", child));
    expect(ctl.pending).toBe(false);
    // Out of the window (no related target), then back over the stage in time.
    doc.dispatchEvent(new FakePointerEvent("pointerout", null));
    expect(ctl.pending).toBe(true);
    doc.dispatchEvent(new FakePointerEvent("pointerover", null));
    expect(ctl.pending).toBe(false);
    // Out to something outside the stage (the mock's page around #app).
    doc.dispatchEvent(new FakePointerEvent("pointerout", outside));
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS);
    expect(calls).toEqual([false]);

    unbind();
    const before = calls.length;
    doc.dispatchEvent(new FakePointerEvent("pointerout", null));
    vi.advanceTimersByTime(COLLAPSE_DELAY_MS);
    expect(calls).toHaveLength(before);
  });
});

describe("startDock", () => {
  it("returns one shared controller and reports failed requests without throwing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const store = { settings: { dock: "left" as const }, ui: { dock_expanded: false, click_through: false, view: "pill" as ViewMode } };
    const setExpanded = vi.fn(() => Promise.reject(new Error("no window")));
    const ctl = startDock(store, setExpanded);
    expect(startDock(store, vi.fn())).toBe(ctl);
    ctl.expand();
    expect(setExpanded).toHaveBeenCalledWith(true);
    await Promise.resolve();
    await Promise.resolve();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
