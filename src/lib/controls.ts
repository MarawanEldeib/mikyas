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
      return close === "quit" ? "Quit Claude Usage" : "Hide to tray";
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

/**
 * Whether a right-click on `target` keeps WebView2's own menu: text fields do (cut, copy,
 * paste); everywhere else the app's menu replaces it.
 */
export function nativeMenuAllowed(target: unknown): boolean {
  if (typeof target !== "object" || target === null) return false;
  const el = target as FieldLike;
  if (el.isContentEditable) return true;
  const tag = el.tagName?.toLowerCase();
  return tag === "textarea" || (tag === "input" && TEXT_INPUTS.has(el.type ?? "text"));
}
