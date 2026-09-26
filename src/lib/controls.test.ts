import { describe, expect, it } from "vitest";
import {
  CONTROL_ZONE_PX,
  controlAction,
  controlLabel,
  inControlZone,
  menuAnchor,
  nativeMenuAllowed,
  windowControls,
  type ControlsState,
} from "./controls";
import type { ViewMode } from "./types";

const base: ControlsState = { view: "card", dock: "off", dockExpanded: false, clickThrough: false };

describe("windowControls", () => {
  it("shows minimize and close on the card, expand and close on the pill", () => {
    expect(windowControls(base)).toEqual(["minimize", "close"]);
    expect(windowControls({ ...base, view: "pill" })).toEqual(["expand", "close"]);
  });

  it("shows only close on the panels (they have a Back button)", () => {
    for (const view of ["settings", "sessions", "history"] as ViewMode[]) {
      expect(windowControls({ ...base, view })).toEqual(["close"]);
    }
  });

  it("shows nothing in click-through mode, for any view", () => {
    for (const view of ["pill", "card", "settings", "sessions", "history"] as ViewMode[]) {
      expect(windowControls({ ...base, view, clickThrough: true })).toEqual([]);
      expect(windowControls({ ...base, view, dock: "left", dockExpanded: true, clickThrough: true })).toEqual([]);
    }
  });

  it("shows nothing on the collapsed dock strip, and the usual set once slid out", () => {
    for (const dock of ["left", "right", "top"] as const) {
      expect(windowControls({ ...base, dock })).toEqual([]);
      expect(windowControls({ ...base, dock, view: "pill" })).toEqual([]);
      expect(windowControls({ ...base, dock, dockExpanded: true })).toEqual(["minimize", "close"]);
      expect(windowControls({ ...base, dock, dockExpanded: true, view: "settings" })).toEqual(["close"]);
    }
  });
});

describe("controlAction", () => {
  it("minimizes the card to the pill, or a slid-out dock back into its strip", () => {
    expect(controlAction("minimize", base, "hide")).toEqual({ type: "view", view: "pill" });
    expect(controlAction("minimize", { ...base, dock: "right", dockExpanded: true }, "hide")).toEqual({ type: "dock-collapse" });
    expect(controlAction("expand", { ...base, view: "pill" }, "hide")).toEqual({ type: "view", view: "card" });
  });

  it("closes by hiding or quitting, as set", () => {
    expect(controlAction("close", base, "hide")).toEqual({ type: "hide" });
    expect(controlAction("close", base, "quit")).toEqual({ type: "quit" });
    expect(controlAction("close", { ...base, dock: "top", dockExpanded: true }, "hide")).toEqual({ type: "hide" });
  });

  it("names every control for its action", () => {
    expect(controlLabel("close", base, "hide")).toBe("Hide to tray");
    expect(controlLabel("close", base, "quit")).toBe("Quit Claude Usage");
    expect(controlLabel("minimize", base, "hide")).toBe("Minimize to pill");
    expect(controlLabel("minimize", { ...base, dock: "left", dockExpanded: true }, "hide")).toBe("Minimize to the screen edge");
    expect(controlLabel("expand", { ...base, view: "pill" }, "hide")).toBe("Expand");
  });
});

describe("inControlZone", () => {
  it("shows the controls over the header strip only, scaled with the UI", () => {
    expect(inControlZone("card", 10, 1)).toBe(true);
    expect(inControlZone("card", CONTROL_ZONE_PX, 1)).toBe(true);
    expect(inControlZone("card", CONTROL_ZONE_PX + 1, 1)).toBe(false);
    expect(inControlZone("card", 150, 1)).toBe(false);
    expect(inControlZone("settings", 30, 1)).toBe(true);
    expect(inControlZone("history", 200, 1)).toBe(false);
    expect(inControlZone("card", 60, 1.3)).toBe(true);
    expect(inControlZone("card", 60, 0.85)).toBe(false);
  });

  it("covers the whole pill", () => {
    expect(inControlZone("pill", 70, 1)).toBe(true);
    expect(inControlZone("pill", 90, 1.3)).toBe(true);
  });
});

describe("menuAnchor", () => {
  const focused = { getBoundingClientRect: () => ({ left: 12.5, bottom: 40 }) };

  it("leaves pointer right-clicks at the cursor", () => {
    expect(menuAnchor({ clientX: 100, clientY: 50, pointerType: "mouse" }, focused)).toBeNull();
    expect(menuAnchor({ clientX: 100, clientY: 50 }, focused)).toBeNull();
  });

  it("anchors keyboard-opened menus below the focused element", () => {
    expect(menuAnchor({ clientX: 0, clientY: 0, pointerType: "" }, focused)).toEqual({ x: 12.5, y: 40 });
    expect(menuAnchor({ clientX: 0, clientY: 0 }, focused)).toEqual({ x: 12.5, y: 40 });
    expect(menuAnchor({ clientX: 0, clientY: 0, pointerType: "" }, null)).toEqual({ x: 8, y: 8 });
    const offLeft = { getBoundingClientRect: () => ({ left: -4, bottom: -2 }) };
    expect(menuAnchor({ clientX: 0, clientY: 0, pointerType: "" }, offLeft)).toEqual({ x: 0, y: 0 });
  });
});

describe("nativeMenuAllowed", () => {
  const input = (type?: string) => ({ tagName: "INPUT", type, isContentEditable: false });

  it("keeps the browser menu in text fields", () => {
    for (const type of ["text", "search", "url", "email", "tel", "password", "number", undefined]) {
      expect(nativeMenuAllowed(input(type)), String(type)).toBe(true);
    }
    expect(nativeMenuAllowed({ tagName: "TEXTAREA", isContentEditable: false })).toBe(true);
    expect(nativeMenuAllowed({ tagName: "DIV", isContentEditable: true })).toBe(true);
  });

  it("replaces it everywhere else", () => {
    for (const type of ["range", "radio", "checkbox", "button", "submit"]) {
      expect(nativeMenuAllowed(input(type)), type).toBe(false);
    }
    for (const tagName of ["BUTTON", "DIV", "SPAN", "svg", "path", "SELECT", "CODE"]) {
      expect(nativeMenuAllowed({ tagName, isContentEditable: false }), tagName).toBe(false);
    }
    expect(nativeMenuAllowed(null)).toBe(false);
    expect(nativeMenuAllowed(undefined)).toBe(false);
    expect(nativeMenuAllowed("input")).toBe(false);
  });

  it("keeps it on selected text, so the Connect panel's command and preview can be copied", () => {
    const pre = { tagName: "PRE", isContentEditable: false };
    const other = { tagName: "SPAN", isContentEditable: false };
    // A fake Selection whose one range covers `pre` only.
    const selection = (text: boolean) => ({
      isCollapsed: !text,
      rangeCount: 1,
      getRangeAt: () => ({ intersectsNode: (n: unknown) => n === pre }),
    });
    expect(nativeMenuAllowed(pre, selection(true))).toBe(true);
    // No selection, an empty one, or one elsewhere: the app's menu.
    expect(nativeMenuAllowed(pre, null)).toBe(false);
    expect(nativeMenuAllowed(pre, selection(false))).toBe(false);
    expect(nativeMenuAllowed(pre, { isCollapsed: false, rangeCount: 0, getRangeAt: () => { throw new Error("no range"); } })).toBe(false);
    expect(nativeMenuAllowed(other, selection(true))).toBe(false);
  });
});
