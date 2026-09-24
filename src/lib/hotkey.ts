// Converts keyboard events into Tauri global-shortcut accelerator strings ("Ctrl+Alt+U").
// Keys are read from `code` (physical position) so the result does not depend on layout.

export interface KeyLike {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

export type HotkeyInput =
  /** Only modifiers are held so far. */
  | { type: "wait" }
  /** Escape: stop recording without changes. */
  | { type: "cancel" }
  /** Backspace / Delete: remove the shortcut. */
  | { type: "clear" }
  | { type: "invalid"; reason: string }
  | { type: "set"; accelerator: string };

const MODIFIER_CODES = /^(Control|Shift|Alt|Meta|OS)(Left|Right)?$/;

const NAMED: Record<string, string> = {
  Space: "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Insert: "Insert",
  Minus: "Minus",
  Equal: "Equal",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Backslash: "Backslash",
  Backquote: "Backquote",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Semicolon: "Semicolon",
  Quote: "Quote",
};

/** Accelerator key name for a `KeyboardEvent.code`, or null if unsupported. */
export function keyName(code: string): string | null {
  let m = /^Key([A-Z])$/.exec(code);
  if (m) return m[1];
  m = /^Digit([0-9])$/.exec(code);
  if (m) return m[1];
  m = /^F([1-9]|1[0-9]|2[0-4])$/.exec(code);
  if (m) return code;
  m = /^Numpad([0-9])$/.exec(code);
  if (m) return code;
  return NAMED[code] ?? null;
}

/** Interprets one keydown while the hotkey field is recording. */
export function readHotkey(e: KeyLike): HotkeyInput {
  const plain = !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey;
  if (plain && e.key === "Escape") return { type: "cancel" };
  if (plain && (e.key === "Backspace" || e.key === "Delete")) return { type: "clear" };
  if (MODIFIER_CODES.test(e.code) || ["Control", "Shift", "Alt", "Meta", "OS"].includes(e.key)) {
    return { type: "wait" };
  }
  const name = keyName(e.code);
  if (!name) return { type: "invalid", reason: "That key can't be used in a shortcut" };
  const isF = /^F\d+$/.test(name);
  if (!e.ctrlKey && !e.altKey && !e.metaKey && !isF) {
    return { type: "invalid", reason: "Add Ctrl, Alt or Win" };
  }
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");
  parts.push(name);
  return { type: "set", accelerator: parts.join("+") };
}

/** Human-readable form of an accelerator ("Super" is shown as "Win"). */
export function displayHotkey(acc: string): string {
  if (!acc) return "None";
  return acc
    .split("+")
    .map((p) => (p === "Super" || p === "Meta" ? "Win" : p === "CommandOrControl" || p === "CmdOrCtrl" ? "Ctrl" : p))
    .join(" + ");
}
