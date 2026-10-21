import { describe, expect, it } from "vitest";
import {
  acceleratorFor,
  actionFor,
  comboFromEvent,
  firesIn,
  formatCombo,
} from "./shortcuts";
import type { ShortcutBinding } from "./types";

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", init);
}

describe("comboFromEvent", () => {
  it("spells modifiers in canonical order before the key", () => {
    expect(comboFromEvent(key({ key: "n", code: "KeyN", metaKey: true }))).toBe(
      "Meta+N",
    );
    expect(
      comboFromEvent(
        key({
          key: "A",
          code: "KeyA",
          metaKey: true,
          shiftKey: true,
          altKey: true,
          ctrlKey: true,
        }),
      ),
    ).toBe("Ctrl+Alt+Shift+Meta+A");
  });

  it("takes letters from the layout, so QWERTZ Z is Z", () => {
    // On a Slovak/German layout the key labelled Z sits where QWERTY has Y.
    expect(comboFromEvent(key({ key: "z", code: "KeyY", metaKey: true }))).toBe(
      "Meta+Z",
    );
  });

  it("falls back to the physical key when Option makes a special character", () => {
    expect(comboFromEvent(key({ key: "®", code: "KeyR", altKey: true }))).toBe(
      "Alt+R",
    );
  });

  it("reads the number row by position, not the accented character", () => {
    // Slovak layout: the 2 key types ľ without Shift.
    expect(comboFromEvent(key({ key: "ľ", code: "Digit2", metaKey: true }))).toBe(
      "Meta+2",
    );
  });

  it("names punctuation and special keys by code", () => {
    expect(comboFromEvent(key({ key: ",", code: "Comma", metaKey: true }))).toBe(
      "Meta+Comma",
    );
    expect(comboFromEvent(key({ key: "Enter", code: "Enter", metaKey: true }))).toBe(
      "Meta+Enter",
    );
    expect(comboFromEvent(key({ key: "F5", code: "F5" }))).toBe("F5");
    expect(comboFromEvent(key({ key: " ", code: "Space", altKey: true }))).toBe(
      "Alt+Space",
    );
  });

  it("understands named keys from key alone when code is missing", () => {
    expect(comboFromEvent(key({ key: "Backspace" }))).toBe("Backspace");
    expect(comboFromEvent(key({ key: "Enter", metaKey: true }))).toBe(
      "Meta+Enter",
    );
  });

  it("ignores lone modifiers and keys it cannot bind", () => {
    expect(comboFromEvent(key({ key: "Meta", code: "MetaLeft", metaKey: true }))).toBe(
      null,
    );
    expect(comboFromEvent(key({ key: "Shift", code: "ShiftLeft", shiftKey: true }))).toBe(
      null,
    );
    expect(comboFromEvent(key({ key: "Dead", code: "IntlRo" }))).toBe(null);
  });
});

describe("formatCombo", () => {
  it("renders the Mac glyphs in Apple's modifier order", () => {
    expect(formatCombo("Meta+N")).toBe("⌘N");
    expect(formatCombo("Ctrl+Alt+Shift+Meta+A")).toBe("⌃⌥⇧⌘A");
    expect(formatCombo("Backspace")).toBe("⌫");
    expect(formatCombo("Meta+Enter")).toBe("⌘↩");
    expect(formatCombo("Meta+Comma")).toBe("⌘,");
    expect(formatCombo("Shift+ArrowDown")).toBe("⇧↓");
    expect(formatCombo("Alt+Space")).toBe("⌥Space");
  });
});

describe("acceleratorFor", () => {
  // Tauri's native menus read "Cmd" where the stored combo says "Meta";
  // every other modifier and key name they take as is.
  it("writes a combo the way a native menu item expects it", () => {
    expect(acceleratorFor("Meta+R")).toBe("Cmd+R");
    expect(acceleratorFor("Ctrl+Alt+Shift+Meta+A")).toBe("Ctrl+Alt+Shift+Cmd+A");
    expect(acceleratorFor("Backspace")).toBe("Backspace");
    expect(acceleratorFor("Meta+Enter")).toBe("Cmd+Enter");
  });
});

describe("actionFor", () => {
  const bindings: ShortcutBinding[] = [
    { action: "new-message", combo: null, defaultCombo: "Meta+N" },
    { action: "reply", combo: "Meta+N", defaultCombo: "Meta+R" },
  ];

  it("finds the action bound to a combo, ignoring unbound ones", () => {
    expect(actionFor(bindings, "Meta+N")).toBe("reply");
    expect(actionFor(bindings, "Meta+R")).toBe(null);
  });
});

describe("firesIn", () => {
  const input = document.createElement("input");
  const textarea = document.createElement("textarea");
  const editor = document.createElement("div");
  editor.setAttribute("contenteditable", "true");
  const inEditor = document.createElement("p");
  editor.append(inEditor);

  it("fires combos with Cmd or Ctrl everywhere, even while typing", () => {
    for (const target of [document.body, input, textarea, inEditor]) {
      expect(firesIn(target, "Meta+N")).toBe(true);
      expect(firesIn(target, "Ctrl+Meta+S")).toBe(true);
    }
  });

  it("keeps bare and Shift/Option keys out of text fields", () => {
    expect(firesIn(document.body, "Backspace")).toBe(true);
    expect(firesIn(null, "Backspace")).toBe(true);
    for (const target of [input, textarea, editor, inEditor]) {
      expect(firesIn(target, "Backspace")).toBe(false);
      expect(firesIn(target, "Shift+R")).toBe(false);
      expect(firesIn(target, "Alt+R")).toBe(false);
    }
  });
});
