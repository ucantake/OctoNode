// Cross-platform keyboard shortcuts.
//
// Shortcuts are declared once with the abstract `Mod` modifier:
//   "Mod+K"        → ⌘K on macOS, Ctrl+K on Linux/Windows
//   "Mod+Shift+P"  → ⇧⌘P / Ctrl+Shift+P
//   "Alt+ArrowUp"  → ⌥↑ / Alt+↑
//
// The OS comes from the Rust backend (`PlatformInfo`, authoritative) and falls
// back to the user agent before bootstrap completes.

import type { PlatformInfo } from "../types/models";

let platformOs: string | null = null;

export function setPlatform(info: PlatformInfo): void {
  platformOs = info.os;
}

export function isMac(): boolean {
  if (platformOs) return platformOs === "macos";
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
}

export interface Shortcut {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  /** Raw Ctrl on macOS (distinct from Mod there). */
  ctrl: boolean;
  /** Normalized key: lowercase letter, digit, or a KeyboardEvent.key name. */
  key: string;
}

export function parseShortcut(spec: string): Shortcut {
  const parts = spec.split("+").map((p) => p.trim());
  const key = parts.pop();
  if (!key) throw new Error(`invalid shortcut: ${spec}`);
  const mods = new Set(parts.map((p) => p.toLowerCase()));
  for (const m of mods) {
    if (!["mod", "shift", "alt", "ctrl"].includes(m)) {
      throw new Error(`unknown modifier "${m}" in shortcut ${spec}`);
    }
  }
  return {
    mod: mods.has("mod"),
    shift: mods.has("shift"),
    alt: mods.has("alt"),
    ctrl: mods.has("ctrl"),
    key: key.length === 1 ? key.toLowerCase() : key,
  };
}

/** Physical-key fallback: with Alt/Option held, macOS reports `event.key` as
 * a composed character (⌥K → "˚"), so letters/digits are matched by `code`. */
function keyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  return null;
}

export interface KeyEventLike {
  key: string;
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export function matchesShortcut(e: KeyEventLike, s: Shortcut, mac = isMac()): boolean {
  const wantMeta = mac && s.mod;
  const wantCtrl = mac ? s.ctrl : s.mod || s.ctrl;
  if (e.metaKey !== wantMeta || e.ctrlKey !== wantCtrl) return false;
  if (e.altKey !== s.alt) return false;

  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  const shiftSensitive = s.key.length !== 1 || /[a-z0-9]/.test(s.key);
  if (shiftSensitive && e.shiftKey !== s.shift) return false;

  if (key === s.key) return true;
  return e.altKey || e.shiftKey ? keyFromCode(e.code) === s.key : false;
}

const MAC_SYMBOLS: Record<string, string> = {
  mod: "⌘",
  ctrl: "⌃",
  alt: "⌥",
  shift: "⇧",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Enter: "↩",
  Backspace: "⌫",
  Escape: "⎋",
};

/** Human label: "⇧⌘P" on macOS, "Ctrl+Shift+P" elsewhere. */
export function formatShortcut(spec: string, mac = isMac()): string {
  const s = parseShortcut(spec);
  const keyLabel = s.key.length === 1 ? s.key.toUpperCase() : s.key;
  if (mac) {
    // Apple HIG order: ⌃ ⌥ ⇧ ⌘
    return [
      s.ctrl && MAC_SYMBOLS.ctrl,
      s.alt && MAC_SYMBOLS.alt,
      s.shift && MAC_SYMBOLS.shift,
      s.mod && MAC_SYMBOLS.mod,
      MAC_SYMBOLS[s.key] ?? keyLabel,
    ]
      .filter(Boolean)
      .join("");
  }
  return [s.mod || s.ctrl ? "Ctrl" : null, s.alt && "Alt", s.shift && "Shift", keyLabel]
    .filter(Boolean)
    .join("+");
}

/** Last path segment for either separator (paths from Windows use `\`). */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
