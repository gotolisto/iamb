import { describe, expect, it, vi } from "vitest";

import {
  createPaletteApi,
  createPaletteLink,
  isLibraryRequest,
  isPaletteEvent,
  isPaletteOpen,
  isPaletteResults,
} from "./api";
import { hit, openOf } from "./testkit";

describe("the palette's commands", () => {
  it("call the Rust side's commands with the arguments it expects", async () => {
    const invoke = vi.fn((command: string) => {
      if (command === "palette_current") return Promise.resolve(openOf());
      if (command === "palette_search") return Promise.resolve({ total: 3, hits: [hit()] });
      return Promise.resolve(null);
    });
    const api = createPaletteApi(invoke as never);
    await api.ready();
    expect(await api.current()).toEqual(openOf());
    expect((await api.search("ret")).total).toBe(3);
    await api.painted(4, 12.5);
    await api.pick(4, { kind: "insert", id: "x" });
    expect(await api.preview("x")).toBeNull();
    expect(invoke.mock.calls).toEqual([
      ["palette_ready"],
      ["palette_current"],
      ["palette_search", { query: "ret" }],
      ["palette_painted", { session: 4, paintedAtMs: 12.5 }],
      ["palette_pick", { session: 4, action: { kind: "insert", id: "x" } }],
      ["palette_preview", { id: "x" }],
    ]);
  });

  it("refuses an answer of the wrong shape", async () => {
    const api = createPaletteApi((() => Promise.resolve({ total: "many" })) as never);
    await expect(api.search("a")).rejects.toMatchObject({ code: "unexpected_response" });
    expect(await api.current()).toBeNull();
  });

  it("checks what the Rust side sends", () => {
    expect(isPaletteEvent({ type: "open", session: 1 })).toBe(true);
    expect(isPaletteEvent({ type: "opened", session: 1 })).toBe(false);
    expect(isPaletteOpen(openOf({ lastForm: "x" }))).toBe(true);
    expect(isPaletteOpen({ session: 1 })).toBe(false);
    expect(isPaletteResults({ total: 1, hits: [hit({}, { ranges: [[0, 2]] })] })).toBe(true);
    expect(isPaletteResults({ total: 1, hits: [{ row: {}, field: null, ranges: [] }] })).toBe(false);
    expect(isPaletteResults({ total: 1, hits: [hit({}, { ranges: [[-1, 2]] })] })).toBe(false);
    expect(isLibraryRequest({ kind: "new", label: "a" })).toBe(true);
    expect(isLibraryRequest({ kind: "show", id: "a" })).toBe(true);
    expect(isLibraryRequest({ kind: "show" })).toBe(false);
  });

  it("takes a library request once and opens the palette from the library window", async () => {
    const invoke = vi.fn((command: string) =>
      Promise.resolve(command === "library_take_request" ? { kind: "new", label: "w" } : undefined),
    );
    const link = createPaletteLink(invoke as never, () => Promise.resolve(() => undefined));
    expect(await link.take()).toEqual({ kind: "new", label: "w" });
    await link.open();
    expect(invoke).toHaveBeenLastCalledWith("palette_open");
  });
});
