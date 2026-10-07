import { describe, expect, it } from "vitest";
import { basename, formatShortcut, matchesShortcut, parseShortcut, type KeyEventLike } from "./platform";

const ev = (over: Partial<KeyEventLike>): KeyEventLike => ({
  key: "",
  code: "",
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...over,
});

describe("shortcuts", () => {
  it("maps Mod to Cmd on macOS and Ctrl elsewhere", () => {
    const s = parseShortcut("Mod+K");
    expect(matchesShortcut(ev({ key: "k", code: "KeyK", metaKey: true }), s, true)).toBe(true);
    expect(matchesShortcut(ev({ key: "k", code: "KeyK", ctrlKey: true }), s, true)).toBe(false);
    expect(matchesShortcut(ev({ key: "k", code: "KeyK", ctrlKey: true }), s, false)).toBe(true);
    expect(matchesShortcut(ev({ key: "k", code: "KeyK", metaKey: true }), s, false)).toBe(false);
  });

  it("requires exact shift state for letters", () => {
    const s = parseShortcut("Mod+Shift+P");
    expect(matchesShortcut(ev({ key: "P", code: "KeyP", ctrlKey: true, shiftKey: true }), s, false)).toBe(true);
    expect(matchesShortcut(ev({ key: "p", code: "KeyP", ctrlKey: true }), s, false)).toBe(false);
  });

  it("matches Option-modified letters on macOS by physical key", () => {
    const s = parseShortcut("Mod+Alt+K");
    expect(matchesShortcut(ev({ key: "˚", code: "KeyK", metaKey: true, altKey: true }), s, true)).toBe(true);
  });

  it("formats per platform", () => {
    expect(formatShortcut("Mod+Shift+P", true)).toBe("⇧⌘P");
    expect(formatShortcut("Mod+Shift+P", false)).toBe("Ctrl+Shift+P");
    expect(formatShortcut("Alt+ArrowUp", true)).toBe("⌥↑");
  });

  it("rejects unknown modifiers", () => {
    expect(() => parseShortcut("Hyper+K")).toThrow();
  });
});

describe("basename", () => {
  it("handles both separators", () => {
    expect(basename("C:\\repos\\octonode")).toBe("octonode");
    expect(basename("/home/me/octonode/")).toBe("octonode");
  });
});
