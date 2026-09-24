import { describe, expect, it } from "vitest";
import { displayHotkey, keyName, readHotkey, type KeyLike } from "./hotkey";

const key = (code: string, mods: Partial<KeyLike> = {}, k = ""): KeyLike => ({
  key: k || code,
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe("keyName", () => {
  it("maps physical codes", () => {
    expect(keyName("KeyU")).toBe("U");
    expect(keyName("Digit7")).toBe("7");
    expect(keyName("F13")).toBe("F13");
    expect(keyName("ArrowUp")).toBe("Up");
    expect(keyName("Numpad4")).toBe("Numpad4");
    expect(keyName("IntlBackslash")).toBeNull();
  });
});

describe("readHotkey", () => {
  it("builds accelerators in a fixed modifier order", () => {
    expect(readHotkey(key("KeyU", { ctrlKey: true, altKey: true }, "u"))).toEqual({ type: "set", accelerator: "Ctrl+Alt+U" });
    expect(readHotkey(key("KeyK", { metaKey: true, shiftKey: true, ctrlKey: true }))).toEqual({
      type: "set",
      accelerator: "Ctrl+Shift+Super+K",
    });
  });
  it("allows bare function keys", () => {
    expect(readHotkey(key("F9"))).toEqual({ type: "set", accelerator: "F9" });
  });
  it("waits while only modifiers are held", () => {
    expect(readHotkey(key("ControlLeft", { ctrlKey: true }, "Control"))).toEqual({ type: "wait" });
    expect(readHotkey(key("AltRight", { altKey: true }, "Alt"))).toEqual({ type: "wait" });
  });
  it("requires a non-shift modifier for normal keys", () => {
    expect(readHotkey(key("KeyA", {}, "a")).type).toBe("invalid");
    expect(readHotkey(key("KeyA", { shiftKey: true }, "A")).type).toBe("invalid");
  });
  it("handles cancel and clear", () => {
    expect(readHotkey(key("Escape", {}, "Escape"))).toEqual({ type: "cancel" });
    expect(readHotkey(key("Backspace", {}, "Backspace"))).toEqual({ type: "clear" });
    expect(readHotkey(key("Delete", {}, "Delete"))).toEqual({ type: "clear" });
  });
  it("rejects unsupported keys", () => {
    expect(readHotkey(key("IntlRo", { ctrlKey: true })).type).toBe("invalid");
  });
});

describe("displayHotkey", () => {
  it("prettifies accelerators", () => {
    expect(displayHotkey("Ctrl+Alt+U")).toBe("Ctrl + Alt + U");
    expect(displayHotkey("Super+Shift+K")).toBe("Win + Shift + K");
    expect(displayHotkey("")).toBe("None");
  });
});
