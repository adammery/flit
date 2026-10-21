// Keyboard shortcuts on the frontend: turn a KeyboardEvent into the canonical
// combo string the backend stores ("Ctrl+Alt+Shift+Meta+<Key>"), show it as
// Mac glyphs, and decide whether it may fire where focus is. Which combos are
// allowed and who owns them is decided in Rust (storage/settings.rs).

import type { ShortcutAction, ShortcutBinding } from "./types";

/** Named keys a combo can end in — `KeyboardEvent.code` names, which for
 * these also match `KeyboardEvent.key` except Space (" "). */
const NAMED_KEYS = new Set([
  "Enter",
  "Backspace",
  "Delete",
  "Space",
  "Escape",
  "Tab",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Comma",
  "Period",
  "Slash",
  "Semicolon",
  "Quote",
  "BracketLeft",
  "BracketRight",
  "Backslash",
  "Minus",
  "Equal",
  "Backquote",
  "F1",
  "F2",
  "F3",
  "F4",
  "F5",
  "F6",
  "F7",
  "F8",
  "F9",
  "F10",
  "F11",
  "F12",
]);

function keyOf(event: KeyboardEvent): string | null {
  // Letters come from the layout (event.key), so ⌘Z is the key labelled Z on
  // a QWERTZ keyboard too.
  if (/^[a-z]$/i.test(event.key)) return event.key.toUpperCase();
  // Otherwise the physical key: Option turns letters into special characters
  // (⌥R types ®) and the Slovak number row types ľščť… instead of digits.
  const letterOrDigit = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(event.code);
  if (letterOrDigit) return letterOrDigit[1] ?? letterOrDigit[2];
  if (event.code === "NumpadEnter") return "Enter";
  if (NAMED_KEYS.has(event.code)) return event.code;
  // Some synthetic events carry no code; the named keys still say who they are.
  if (event.key === " ") return "Space";
  if (NAMED_KEYS.has(event.key)) return event.key;
  return null;
}

/** The canonical combo for a key press, or null for a lone modifier or a key
 * that cannot be bound. */
export function comboFromEvent(event: KeyboardEvent): string | null {
  const key = keyOf(event);
  if (key === null) return null;
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Meta");
  parts.push(key);
  return parts.join("+");
}

const GLYPHS: Record<string, string> = {
  Ctrl: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Meta: "⌘",
  Enter: "↩",
  Backspace: "⌫",
  Delete: "⌦",
  Escape: "Esc",
  Tab: "⇥",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
};

/** A combo as macOS menus write it: "Shift+Meta+R" → "⇧⌘R". */
export function formatCombo(combo: string): string {
  return combo
    .split("+")
    .map((part) => GLYPHS[part] ?? part)
    .join("");
}

/** A combo as a native menu item's accelerator: "Shift+Meta+R" →
 * "Shift+Cmd+R". Tauri's menus take every other modifier and key name the
 * combo uses as they are. */
export function acceleratorFor(combo: string): string {
  return combo
    .split("+")
    .map((part) => (part === "Meta" ? "Cmd" : part))
    .join("+");
}

/** The action currently bound to `combo`, if any. */
export function actionFor(
  bindings: ShortcutBinding[],
  combo: string,
): ShortcutAction | null {
  return bindings.find((binding) => binding.combo === combo)?.action ?? null;
}

function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.isContentEditable ||
    // why closest as well: jsdom does not implement isContentEditable.
    target.closest('[contenteditable]:not([contenteditable="false"])') !== null
  );
}

/** May `combo` fire with focus on `target`? Combos with ⌘ or ⌃ work
 * everywhere — typing never produces them. Anything else (⌫, a bare letter,
 * ⇧/⌥ + key) would steal keystrokes, so it stays out of text fields. */
export function firesIn(target: EventTarget | null, combo: string): boolean {
  const parts = combo.split("+");
  if (parts.includes("Meta") || parts.includes("Ctrl")) return true;
  return !isTextField(target);
}
