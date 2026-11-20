// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";

import { blockReload, guardUnload, isReloadKey } from "./reload";

const key = (init: KeyboardEventInit) => new KeyboardEvent("keydown", { cancelable: true, ...init });

let stops: Array<() => void> = [];
afterEach(() => {
  for (const stop of stops) stop();
  stops = [];
});

describe("reloading the page (the Phase 3 UI review's finding 5)", () => {
  it("names the keys a web view reloads with", () => {
    expect(isReloadKey(key({ key: "F5" }))).toBe(true);
    expect(isReloadKey(key({ key: "F5", ctrlKey: true }))).toBe(true);
    expect(isReloadKey(key({ key: "r", ctrlKey: true }))).toBe(true);
    expect(isReloadKey(key({ key: "R", ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isReloadKey(key({ key: "r", metaKey: true }))).toBe(true);
    expect(isReloadKey(key({ key: "r" }))).toBe(false);
    expect(isReloadKey(key({ key: "F6" }))).toBe(false);
    expect(isReloadKey(key({ key: "s", ctrlKey: true }))).toBe(false);
  });

  it("keeps F5 and Ctrl+R from reloading any window", () => {
    stops.push(blockReload(window));
    const f5 = key({ key: "F5", bubbles: true });
    document.body.dispatchEvent(f5);
    expect(f5.defaultPrevented).toBe(true);
    const ctrlR = key({ key: "r", ctrlKey: true, bubbles: true });
    document.body.dispatchEvent(ctrlR);
    expect(ctrlR.defaultPrevented).toBe(true);
    const typed = key({ key: "r", bubbles: true });
    document.body.dispatchEvent(typed);
    expect(typed.defaultPrevented).toBe(false);
  });

  it("asks before the page unloads while there are unsaved edits", () => {
    let dirty = true;
    stops.push(guardUnload(window, () => dirty));
    const first = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(first);
    expect(first.defaultPrevented).toBe(true);
    dirty = false;
    const second = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(second);
    expect(second.defaultPrevented).toBe(false);
  });
});
