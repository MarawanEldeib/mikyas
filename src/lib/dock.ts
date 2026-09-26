// Edge-dock hover behaviour. Pointing at the collapsed strip slides the widget out at once;
// leaving the slid-out widget slides it back in after a short grace period unless the pointer
// returns, and the card's "–" slides it in at once. Rust owns the geometry (dock.rs) and ignores
// requests that don't apply (click-through, Settings open, pointer still over the window unless
// forced); the guards here only avoid pointless calls.

import type { Settings, UiState, ViewMode } from "./types";

/** Grace period before a left widget slides back in. */
export const COLLAPSE_DELAY_MS = 450;
/** After the card's "–" slid the widget in, the strip may appear right under the pointer:
 *  pointing at it doesn't slide it out again until the pointer has left, or this long. */
export const REEXPAND_GUARD_MS = 1000;

export interface DockState {
  docked: boolean;
  expanded: boolean;
  clickThrough: boolean;
  view: ViewMode;
}

/** The slice of the app store the dock reads (`app` from stores.svelte.ts satisfies it). */
export interface DockApp {
  readonly settings: Pick<Settings, "dock"> | null;
  readonly ui: Pick<UiState, "dock_expanded" | "click_through" | "view">;
}

export function dockState(a: DockApp): DockState {
  return {
    docked: (a.settings?.dock ?? "off") !== "off",
    expanded: a.ui.dock_expanded,
    clickThrough: a.ui.click_through,
    view: a.ui.view,
  };
}

/** Click-through never slides out: the widget could not be left again. */
export function canExpand(s: DockState): boolean {
  return s.docked && !s.expanded && !s.clickThrough;
}

/** Only the pill and the card slide back in (Settings, Sessions and History stay out). */
export function canCollapse(s: DockState): boolean {
  return s.docked && s.expanded && (s.view === "pill" || s.view === "card");
}

export interface DockDeps {
  state(): DockState;
  /** `force`: the user's own slide-in, with the pointer still over the widget. */
  setExpanded(expanded: boolean, force?: boolean): void;
}

export interface DockController {
  /** The strip was clicked (or activated from the keyboard): slide out now. */
  expand(): void;
  /** The pointer entered the strip: slide out now, unless the "–" just slid it in under it. */
  hover(): void;
  /** The card's "–": slide in now. */
  collapse(): void;
  /** The pointer is over the widget: keep it out. */
  hold(): void;
  /** The pointer left the widget: slide in after COLLAPSE_DELAY_MS unless it comes back. */
  leave(): void;
  /** A slide-in is scheduled. */
  readonly pending: boolean;
  dispose(): void;
}

export function createDockController(deps: DockDeps): DockController {
  let timer: ReturnType<typeof setTimeout> | null = null;
  // A slide-out was requested; the store only reports it once Rust's ui-state arrives, and the
  // flag is dropped when it does.
  let requested = false;
  let guardUntil = 0;
  const cancel = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const expand = () => {
    cancel();
    if (!canExpand(deps.state())) return;
    requested = true;
    deps.setExpanded(true);
  };
  return {
    expand,
    hover() {
      if (Date.now() >= guardUntil) expand();
    },
    collapse() {
      cancel();
      requested = false;
      if (!canCollapse(deps.state())) return;
      guardUntil = Date.now() + REEXPAND_GUARD_MS;
      deps.setExpanded(false, true);
    },
    hold: cancel,
    leave() {
      cancel();
      guardUntil = 0;
      const s = deps.state();
      if (s.expanded) requested = false;
      // A pointer that brushed past the strip leaves before the slide-out lands; the widget
      // must still slide back in, so the timer is armed and re-checks when it fires.
      if (!canCollapse(s) && !(requested && s.docked)) return;
      timer = setTimeout(() => {
        timer = null;
        requested = false;
        if (canCollapse(deps.state())) deps.setExpanded(false);
      }, COLLAPSE_DELAY_MS);
    },
    get pending() {
      return timer !== null;
    },
    dispose: cancel,
  };
}

/** The event-listener surface of a Document (a plain EventTarget in tests). */
export type Listenable = Pick<EventTarget, "addEventListener" | "removeEventListener">;

/**
 * Document-level pointer tracking: a pointer over the stage holds the widget out, and one that
 * leaves it (out of the window in the app; out of the #app box in the browser mock) starts the
 * slide-in timer. Moves between elements inside the stage are ignored. Returns the unbinder.
 */
export function bindPointer(doc: Listenable, inside: (t: EventTarget | null) => boolean, ctl: DockController): () => void {
  const over = (e: Event) => {
    if (inside(e.target)) ctl.hold();
  };
  const out = (e: Event) => {
    if (!inside((e as PointerEvent).relatedTarget)) ctl.leave();
  };
  doc.addEventListener("pointerover", over);
  doc.addEventListener("pointerout", out);
  return () => {
    doc.removeEventListener("pointerover", over);
    doc.removeEventListener("pointerout", out);
  };
}

let controller: DockController | null = null;

/**
 * The app-wide dock controller, bound to the document on first use (DockBar, Pill, Card and the
 * window controls call this; it lives for the page's lifetime because the widget swaps between
 * those views while docked).
 */
export function startDock(a: DockApp, setExpanded: (expanded: boolean, force?: boolean) => Promise<unknown>): DockController {
  if (controller) return controller;
  const ctl = createDockController({
    state: () => dockState(a),
    setExpanded: (v, force) => {
      setExpanded(v, force).catch((e: unknown) => console.warn("set_dock_expanded failed", e));
    },
  });
  if (typeof document !== "undefined") {
    const stage = document.getElementById("app") ?? document.documentElement;
    bindPointer(document, (t) => t instanceof Node && stage.contains(t), ctl);
  }
  controller = ctl;
  return ctl;
}
