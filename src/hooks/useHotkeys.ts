import { useEffect, useRef } from "react";
import { matchesShortcut, parseShortcut, type Shortcut } from "../lib/platform";

export type HotkeyMap = Record<string, (e: KeyboardEvent) => void>;

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/**
 * Registers global shortcuts written with the abstract `Mod` key.
 * Shortcuts without Mod/Alt/Ctrl are ignored while typing in a field.
 */
export function useHotkeys(map: HotkeyMap, enabled = true): void {
  const handlers = useRef(map);
  handlers.current = map;

  const specs = Object.keys(map).sort().join("|");

  useEffect(() => {
    if (!enabled) return;
    const parsed: Array<[Shortcut, string]> = Object.keys(handlers.current).map((spec) => [
      parseShortcut(spec),
      spec,
    ]);

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.isComposing) return; // IME input (CJK) in progress
      for (const [shortcut, spec] of parsed) {
        const plain = !shortcut.mod && !shortcut.alt && !shortcut.ctrl;
        if (plain && isEditable(e.target)) continue;
        if (matchesShortcut(e, shortcut)) {
          e.preventDefault();
          handlers.current[spec]?.(e);
          return;
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [specs, enabled]);
}
