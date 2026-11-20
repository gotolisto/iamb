import { describe, expect, it } from "vitest";

import type { Permissions } from "./api";
import {
  anyMissing,
  asksForPermissions,
  conflictText,
  didExpand,
  endsWithAbbreviation,
  firstCause,
  firstMissing,
  holdsUnexpanded,
  iconName,
  isMac,
  labelOf,
  progressLine,
  snippetProblem,
  stepsFor,
  whereItLives,
} from "./model";

const p = (accessibility: Permissions["accessibility"], inputMonitoring: Permissions["inputMonitoring"]): Permissions => ({
  accessibility,
  inputMonitoring,
});

describe("the steps", () => {
  it("are five on macOS with a missing permission and four without any", () => {
    expect(stepsFor({ permissions: true, snippet: true })).toEqual(["welcome", "permissions", "snippet", "try", "done"]);
    expect(stepsFor({ permissions: false, snippet: true })).toEqual(["welcome", "snippet", "try", "done"]);
    expect(stepsFor({ permissions: false, snippet: false })).toEqual(["welcome", "try", "done"]);
    expect(progressLine(1, 5)).toBe("Step 2 of 5");
  });

  it("read the permissions the way the service reports them", () => {
    expect(asksForPermissions(p("not_required", "not_required"))).toBe(false);
    expect(asksForPermissions(p("denied", "granted"))).toBe(true);
    expect(anyMissing(p("granted", "denied"))).toBe(true);
    expect(anyMissing(p("granted", "granted"))).toBe(false);
    expect(firstMissing(p("denied", "denied"))).toBe("input_monitoring");
    expect(firstMissing(p("denied", "granted"))).toBe("accessibility");
    expect(firstMissing(p("granted", "granted"))).toBeNull();
  });
});

describe("the first snippet", () => {
  it("is refused with the sentences of the flow", () => {
    expect(snippetProblem({ abbreviation: ";e m", content: "x" })?.text).toBe("An abbreviation cannot contain spaces.");
    expect(snippetProblem({ abbreviation: ";em", content: "  " })?.text).toBe("Type the text Iamb should type for you.");
    expect(snippetProblem({ abbreviation: ";em", content: "ines@brindlewood.example" })).toBeNull();
    expect(conflictText(";em")).toBe(";em is already used by another snippet. Pick a different abbreviation.");
  });

  it("is labelled by its text, cut short", () => {
    expect(labelOf("ines@brindlewood.example")).toBe("ines@brindlewood.example");
    expect(labelOf("x".repeat(60))).toHaveLength(40);
  });
});

describe("the test field", () => {
  it("holds an unexpanded abbreviation once a space or punctuation finishes it", () => {
    expect(holdsUnexpanded(";em ", ";em", false)).toBe(true);
    expect(holdsUnexpanded("hello ;em,", ";em", false)).toBe(true);
    expect(holdsUnexpanded(";em", ";em", false)).toBe(false);
    expect(holdsUnexpanded(";em\n", ";em", false)).toBe(false);
    expect(holdsUnexpanded(";e ", ";em", false)).toBe(false);
    expect(holdsUnexpanded(";em", ";em", true)).toBe(true);
    expect(holdsUnexpanded("", "", true)).toBe(false);
  });

  it("expanded when the abbreviation gave way to the snippet's text", () => {
    expect(didExpand("ines@x.example ", ";em", "ines@x.example")).toBe(true);
    expect(didExpand(";em ", ";em", "ines@x.example")).toBe(false);
    expect(didExpand("", ";em", "ines@x.example")).toBe(false);
    expect(didExpand("ines@x.example ;em", ";em", "ines@x.example")).toBe(false);
  });

  it("knows an abbreviation finished with Enter or Tab", () => {
    expect(endsWithAbbreviation("hello ;em", ";em")).toBe(true);
    expect(endsWithAbbreviation(";em ", ";em")).toBe(false);
  });
});

describe("the first cause", () => {
  const ok = { connected: true, service: { listener: true, paused: false }, permissions: p("granted", "granted"), composing: false, mac: true };

  it("is the keyboard first, in each system's words", () => {
    const mac = firstCause({ ...ok, service: { listener: false, paused: true } });
    expect(mac).toMatchObject({ kind: "no_keyboard", action: "Restart the helper" });
    expect(mac.rest).toContain("macOS sometimes needs");
    const win = firstCause({ ...ok, mac: false, connected: false, service: null });
    expect(win).toMatchObject({ kind: "no_keyboard", action: "Start it again" });
    expect(win.rest).toContain("background service is not running");
  });

  it("then paused, a permission, an input method, and anything else, in that order", () => {
    const all = { ...ok, service: { listener: true, paused: true }, permissions: p("denied", "granted"), composing: true };
    expect(firstCause(all).kind).toBe("paused");
    expect(firstCause({ ...all, service: { listener: true, paused: false } }).kind).toBe("permission");
    expect(firstCause({ ...all, service: { listener: true, paused: false }, permissions: p("granted", "granted") }).kind).toBe("composing");
    expect(firstCause({ ...ok }).kind).toBe("other");
    expect(firstCause({ ...ok, permissions: null }).kind).toBe("other");
  });
});

describe("the system", () => {
  it("is told from the platform's name", () => {
    expect(isMac("MacIntel")).toBe(true);
    expect(isMac("Win32")).toBe(false);
    expect(whereItLives(true)).toContain("menu bar");
    expect(whereItLives(false)).toContain("notification area");
    expect(iconName(true)).toBe("menu-bar icon");
    expect(iconName(false)).toBe("tray icon");
  });
});
