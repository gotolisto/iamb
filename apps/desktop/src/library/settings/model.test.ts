import { describe, expect, it } from "vitest";

import {
  DEFAULT_DELIMITERS,
  chordBadges,
  chordSpoken,
  delimiterChoices,
  delimitersToStore,
  formatExample,
  languageOf,
  readOverrides,
  recordChord,
  refusedText,
  sameApp,
  unknownFormatCode,
  withGroup,
  withOverride,
  withoutOverride,
} from "./model";

describe("delimiters", () => {
  it("start as the default set: no Enter, no Tab", () => {
    const c = delimiterChoices(undefined);
    expect(c).toEqual({ punctuation: "on", closers: "on", enter: false, tab: false });
  });

  it("show a partly chosen group as mixed", () => {
    expect(delimiterChoices(" .,").punctuation).toBe("mixed");
    expect(delimiterChoices(" ").closers).toBe("off");
  });

  it("turn groups on and off, always keeping the space and other characters", () => {
    expect(withGroup(undefined, "enter", true)).toBe(`${DEFAULT_DELIMITERS}\n`);
    expect(delimiterChoices(withGroup(undefined, "tab", true)).tab).toBe(true);
    const no = withGroup(undefined, "punctuation", false);
    expect(no).toBe(" )]}\"'");
    // A stored set that is not the default keeps what it has (a character the page has no box for).
    expect(withGroup("-.", "closers", true)).toBe("-. )]}\"'");
    expect(withGroup("-", "closers", false)).toBe("- ");
    expect(withGroup("", "punctuation", false)).toContain(" ");
  });

  it("are stored only when they differ from the default set", () => {
    expect(delimitersToStore(DEFAULT_DELIMITERS)).toBeNull();
    expect(delimitersToStore([...DEFAULT_DELIMITERS].reverse().join(""))).toBeNull();
    expect(delimitersToStore(`${DEFAULT_DELIMITERS}\n`)).toBe(`${DEFAULT_DELIMITERS}\n`);
  });
});

describe("date and time formats", () => {
  it("find the first code Iamb does not know", () => {
    expect(unknownFormatCode("%Y-%m-%d")).toBeNull();
    expect(unknownFormatCode("%-d %B %Y, %-I:%M %p %j %%")).toBeNull();
    expect(unknownFormatCode("%Y %Q")).toBe("%Q");
    expect(unknownFormatCode("100%")).toBe("%");
    expect(unknownFormatCode("%-Y")).toBe("%-");
  });

  it("write an example with the names of the chosen language", () => {
    const d = new Date(2026, 2, 8, 15, 4, 9); // Sunday 8 March 2026
    expect(formatExample("%Y-%m-%d", d, "en")).toBe("2026-03-08");
    expect(formatExample("%A %-d %B %Y", d, "en")).toBe("Sunday 8 March 2026");
    expect(formatExample("%A %-d %B %Y", d, "de")).toBe("Sonntag 8 März 2026");
    expect(formatExample("%a %b", d, "fr")).toBe("dim. mars");
    expect(formatExample("%-I:%M %p / %H:%M:%S", d, "es")).toBe("3:04 PM / 15:04:09");
    expect(formatExample("%j %y 100%%", d, "en")).toBe("067 26 100%");
  });

  it("read a locale tag as the service does", () => {
    expect(languageOf("de-AT")).toBe("de");
    expect(languageOf("FR")).toBe("fr");
    expect(languageOf("ja")).toBe("en");
    expect(languageOf(undefined)).toBe("en");
  });
});

const key = (code: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}, k = code) => ({
  key: k,
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe("the shortcut recorder", () => {
  it("waits while only modifiers are down", () => {
    expect(recordChord(key("ShiftLeft", { shiftKey: true }, "Shift"))).toEqual({ kind: "waiting" });
    expect(recordChord(key("ControlRight", { ctrlKey: true }, "Control"))).toEqual({ kind: "waiting" });
  });

  it("writes the chord the way the service stores it", () => {
    expect(recordChord(key("Space", { ctrlKey: true, shiftKey: true }, " "))).toEqual({
      kind: "chord",
      chord: "Ctrl+Shift+Space",
    });
    expect(recordChord(key("KeyK", { metaKey: true, altKey: true }, "k"))).toEqual({ kind: "chord", chord: "Alt+Meta+K" });
    expect(recordChord(key("Digit7", { ctrlKey: true }, "7"))).toEqual({ kind: "chord", chord: "Ctrl+7" });
    expect(recordChord(key("ArrowUp", { altKey: true }, "ArrowUp"))).toEqual({ kind: "chord", chord: "Alt+Up" });
    expect(recordChord(key("PageDown", { ctrlKey: true }, "PageDown"))).toEqual({ kind: "chord", chord: "Ctrl+PageDown" });
  });

  it("lets a function key stand alone and refuses a letter without Ctrl, Alt or the Windows key", () => {
    expect(recordChord(key("F9", {}, "F9"))).toEqual({ kind: "chord", chord: "F9" });
    expect(recordChord(key("KeyK", { shiftKey: true }, "K"))).toEqual({ kind: "refused", reason: "modifier" });
    expect(recordChord(key("Space", {}, " "))).toEqual({ kind: "refused", reason: "modifier" });
  });

  it("refuses the editing chords every app uses, as the service does", () => {
    for (const [code, mods] of [
      ["KeyV", { ctrlKey: true }],
      ["KeyV", { metaKey: true }],
      ["KeyC", { ctrlKey: true }],
      ["KeyX", { metaKey: true }],
      ["KeyZ", { ctrlKey: true }],
      ["KeyA", { ctrlKey: true }],
      ["KeyY", { ctrlKey: true }],
      ["Delete", { ctrlKey: true }],
    ] as const) {
      expect(recordChord(key(code, mods, "x")), code).toEqual({ kind: "refused", reason: "reserved" });
      expect(recordChord(key(code, mods, "x"), true), code).toEqual({ kind: "refused", reason: "reserved" });
    }
    // With another modifier they are ordinary chords.
    expect(recordChord(key("KeyV", { ctrlKey: true, shiftKey: true }, "V"))).toEqual({ kind: "chord", chord: "Ctrl+Shift+V" });
    expect(recordChord(key("KeyZ", { metaKey: true, shiftKey: true }, "Z"), true)).toEqual({ kind: "chord", chord: "Shift+Meta+Z" });
  });

  it("refuses a chord that types a character: AltGr, Option and a letter, Ctrl+Alt on Windows", () => {
    const altGr = { getModifierState: (k: string) => k === "AltGraph" };
    expect(recordChord({ ...key("KeyQ", { ctrlKey: true, altKey: true }, "@"), ...altGr })).toEqual({ kind: "refused", reason: "types" });
    expect(recordChord(key("KeyE", { altKey: true }, "é"), true)).toEqual({ kind: "refused", reason: "types" });
    expect(recordChord(key("Digit2", { altKey: true, shiftKey: true }, "€"), true)).toEqual({ kind: "refused", reason: "types" });
    expect(recordChord(key("KeyQ", { altKey: true }, "q"))).toEqual({ kind: "refused", reason: "types" });
    // Windows: Ctrl+Alt is AltGr, also with Shift; the Windows key makes it a shortcut.
    expect(recordChord(key("KeyA", { ctrlKey: true, altKey: true }, "ą"))).toEqual({ kind: "refused", reason: "types" });
    expect(recordChord(key("KeyA", { ctrlKey: true, altKey: true, shiftKey: true }, "Ą"))).toEqual({ kind: "refused", reason: "types" });
    expect(recordChord(key("KeyK", { ctrlKey: true, altKey: true, metaKey: true }, "k"))).toEqual({ kind: "chord", chord: "Ctrl+Alt+Meta+K" });
    // macOS: Control+Option and a letter types nothing.
    expect(recordChord(key("KeyK", { ctrlKey: true, altKey: true }, "k"), true)).toEqual({ kind: "chord", chord: "Ctrl+Alt+K" });
    // Option with a key that is not a letter or digit is fine.
    expect(recordChord(key("ArrowUp", { altKey: true }, "ArrowUp"), true)).toEqual({ kind: "chord", chord: "Alt+Up" });
    expect(refusedText("types", true)).toMatch(/types a character/);
    expect(refusedText("reserved", false)).toMatch(/editing/);
  });

  it("refuses a key the service has no name for", () => {
    expect(recordChord(key("Comma", { ctrlKey: true }, ","))).toEqual({ kind: "refused", reason: "key" });
    expect(recordChord(key("F25", { ctrlKey: true }, "F25"))).toEqual({ kind: "refused", reason: "key" });
  });

  it("shows a chord as key badges for each system", () => {
    expect(chordBadges("Ctrl+Shift+Space", true)).toEqual(["⌃", "⇧", "Space"]);
    expect(chordBadges("Ctrl+Shift+Space", false)).toEqual(["Ctrl", "Shift", "Space"]);
    expect(chordBadges("cmd+k", true)).toEqual(["⌘", "K"]);
    expect(chordBadges("Meta+K", false)).toEqual(["Win", "K"]);
    expect(chordSpoken("Ctrl+Alt+K", true)).toBe("Control, Option, K");
    expect(chordSpoken("Meta+K", false)).toBe("Windows, K");
  });
});

describe("injection overrides", () => {
  it("read the apps of a document and ignore what they do not know", () => {
    expect(readOverrides(undefined)).toEqual([]);
    expect(readOverrides("")).toEqual([]);
    expect(
      readOverrides('{"defaults":{"paste_threshold":9},"apps":{"chat.exe":{"method":"paste","line_break":"shift+enter","key_delay_ms":5,"paste_keys":"ctrl+v"}}}'),
    ).toEqual([{ app: "chat.exe", method: "paste", lineBreak: "shift+enter", keyDelayMs: 5 }]);
    expect(readOverrides("{not json")).toBeNull();
    expect(readOverrides("[]")).toBeNull();
  });

  it("change one field and leave the rest of the document alone", () => {
    const text = '{"defaults":{"paste_threshold":9},"apps":{"chat.exe":{"method":"paste","paste_keys":"ctrl+v"}}}';
    const next = withOverride(text, "chat.exe", { lineBreak: "enter" });
    expect(JSON.parse(next?.text ?? "")).toEqual({
      defaults: { paste_threshold: 9 },
      apps: { "chat.exe": { method: "paste", paste_keys: "ctrl+v", line_break: "enter" } },
    });
    const cleared = withOverride(text, "Chat", { method: null });
    // Another spelling of the same app is the same entry.
    expect(JSON.parse(cleared?.text ?? "").apps).toEqual({ "chat.exe": { paste_keys: "ctrl+v" } });
  });

  it("add an app, remove it, and leave no document behind when nothing is left", () => {
    const added = withOverride(undefined, "chat.exe", { method: "keys" });
    expect(added?.text).toBe('{"apps":{"chat.exe":{"method":"keys"}}}');
    expect(withoutOverride(added?.text, "CHAT.EXE")).toEqual({ text: null });
    const two = withOverride(added?.text, "other.exe", { keyDelayMs: 20 });
    expect(withoutOverride(two?.text, "chat.exe")?.text).toBe('{"apps":{"other.exe":{"key_delay_ms":20}}}');
  });

  it("do not touch a document they cannot read", () => {
    expect(withOverride("{oops", "a.exe", { method: "keys" })).toBeNull();
    expect(withoutOverride("{oops", "a.exe")).toBeNull();
  });

  it("compare apps without a folder, a case or .exe", () => {
    expect(sameApp("C:\\Apps\\MyBank.EXE", "mybank")).toBe(true);
    expect(sameApp("com.example.A", "com.example.a")).toBe(true);
    expect(sameApp("a.exe", "b.exe")).toBe(false);
  });
});
