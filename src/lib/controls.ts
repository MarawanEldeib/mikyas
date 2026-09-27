// Window controls: the hover group in the widget's top-right corner (WindowControls.svelte) and
// which right-clicks keep WebView2's own menu. Pure, so the rules are tested without a DOM.

import type { CloseAction, DockEdge, ViewMode } from "./types";

export type WindowControl = "minimize" | "expand" | "close";

export interface ControlsState {
  view: ViewMode;
  dock: DockEdge;
  dockExpanded: boolean;
  clickThrough: boolean;
}

/** The controls shown, left to right. */
export function windowControls(s: ControlsState): WindowControl[] {
  // Ghost mode lets the pointer through; the collapsed dock strip has no room.
  if (s.clickThrough || (s.dock !== "off" && !s.dockExpanded)) return [];
  switch (s.view) {
    case "pill":
      return ["expand", "close"];
    case "card":
      return ["minimize", "close"];
    default:
      // Settings, Sessions and History go back to the card with their own Back button.
      return ["close"];
  }
}

/** Height (CSS px at UI scale 1) of the header strip whose hover shows the controls. */
export const CONTROL_ZONE_PX = 48;

/**
 * Whether the pointer at `clientY` should show the controls: anywhere on the 72px pill, but only
 * over the header strip elsewhere, so pointing at the numbers never fades the context % out.
 */
export function inControlZone(view: ViewMode, clientY: number, uiScale: number): boolean {
  return view === "pill" || clientY <= CONTROL_ZONE_PX * uiScale;
}

/** Where a right-click menu opened from the keyboard (Menu key, Shift+F10) should appear. */
export interface MenuAnchor {
  x: number;
  y: number;
}

/**
 * `null` for a pointer right-click (the menu opens at the cursor). A keyboard-opened menu has no
 * pointer (`pointerType` "" in Chromium, or no coordinates), so it opens below the focused element
 * instead of wherever the mouse happens to be.
 */
export function menuAnchor(
  e: { clientX: number; clientY: number; pointerType?: string },
  focused: { getBoundingClientRect(): { left: number; bottom: number } } | null,
): MenuAnchor | null {
  const fromKeyboard = e.pointerType === "" || (e.clientX === 0 && e.clientY === 0);
  if (!fromKeyboard) return null;
  if (!focused) return { x: 8, y: 8 };
  const r = focused.getBoundingClientRect();
  return { x: Math.max(0, r.left), y: Math.max(0, r.bottom) };
}

export type ControlAction =
  | { type: "view"; view: ViewMode }
  /** Slide the docked widget back into its strip. */
  | { type: "dock-collapse" }
  | { type: "hide" }
  | { type: "quit" };

export function controlAction(control: WindowControl, s: ControlsState, close: CloseAction): ControlAction {
  switch (control) {
    case "expand":
      return { type: "view", view: "card" };
    case "minimize":
      return s.dock === "off" ? { type: "view", view: "pill" } : { type: "dock-collapse" };
    case "close":
      return close === "quit" ? { type: "quit" } : { type: "hide" };
  }
}

/** Accessible name and tooltip. */
export function controlLabel(control: WindowControl, s: ControlsState, close: CloseAction): string {
  switch (control) {
    case "expand":
      return "Expand";
    case "minimize":
      return s.dock === "off" ? "Minimize to pill" : "Minimize to the screen edge";
    case "close":
      return close === "quit" ? "Quit Mikyas" : "Hide to tray";
  }
}

/** Input types that take text (HTMLInputElement.type is lower-case, "text" by default). */
const TEXT_INPUTS = new Set(["text", "search", "url", "email", "tel", "password", "number"]);

/** The slice of an HTMLElement this reads (a plain object in tests). */
interface FieldLike {
  tagName?: string;
  type?: string;
  isContentEditable?: boolean;
}

/** The slice of a Selection this reads (a plain object in tests). */
interface SelectionLike {
  isCollapsed: boolean;
  rangeCount: number;
  getRangeAt(index: number): { intersectsNode(node: Node): boolean };
}

/**
 * Whether a right-click on `target` keeps WebView2's own menu: text fields do (cut, copy,
 * paste), and so does selected text under the pointer (the Connect panel's command and preview
 * are selectable, for copying); everywhere else the app's menu replaces it.
 */
export function nativeMenuAllowed(target: unknown, selection: SelectionLike | null = null): boolean {
  if (typeof target !== "object" || target === null) return false;
  if (selection && !selection.isCollapsed && selection.rangeCount > 0 && selection.getRangeAt(0).intersectsNode(target as Node)) {
    return true;
  }
  const el = target as FieldLike;
  if (el.isContentEditable) return true;
  const tag = el.tagName?.toLowerCase();
  return tag === "textarea" || (tag === "input" && TEXT_INPUTS.has(el.type ?? "text"));
}
