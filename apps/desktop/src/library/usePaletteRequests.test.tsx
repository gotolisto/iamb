// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { LibraryRequest, PaletteLink } from "../palette/api";
import { usePaletteRequests } from "./usePaletteRequests";

afterEach(cleanup);

function link(requests: Array<LibraryRequest | null>) {
  let hear: () => void = () => undefined;
  const queue = [...requests];
  const stop = vi.fn();
  const fake: PaletteLink = {
    take: vi.fn(() => Promise.resolve(queue.shift() ?? null)),
    listen: (h) => {
      hear = h;
      return stop;
    },
    open: vi.fn(() => Promise.resolve()),
  };
  return { fake, stop, hear: () => hear(), push: (r: LibraryRequest) => queue.push(r) };
}

const flush = () => act(async () => void (await Promise.resolve()));

describe("the library's side of the search palette", () => {
  it("opens a draft for a new snippet, labelled as asked, once the folders are loaded", async () => {
    const l = link([{ kind: "new", label: "warranty" }]);
    const onNew = vi.fn();
    const onShow = vi.fn();
    const { rerender } = renderHook(
      ({ ready }) => usePaletteRequests(l.fake, ready, { onNew, onShow }),
      { initialProps: { ready: false } },
    );
    await flush();
    expect(onNew).not.toHaveBeenCalled();
    rerender({ ready: true });
    expect(onNew).toHaveBeenCalledWith("warranty");
    expect(onShow).not.toHaveBeenCalled();
  });

  it("shows the snippet at once when the window is ready, and again when told another waits", async () => {
    const l = link([{ kind: "show", id: "a" }]);
    const onNew = vi.fn();
    const onShow = vi.fn();
    renderHook(() => usePaletteRequests(l.fake, true, { onNew, onShow }));
    await flush();
    expect(onShow).toHaveBeenCalledWith("a");
    l.push({ kind: "show", id: "b" });
    l.hear();
    await flush();
    expect(onShow).toHaveBeenLastCalledWith("b");
    // Nothing waits: nothing happens.
    l.hear();
    await flush();
    expect(onShow).toHaveBeenCalledTimes(2);
  });

  it("stops listening when the window goes", async () => {
    const l = link([]);
    const { unmount } = renderHook(() =>
      usePaletteRequests(l.fake, true, { onNew: vi.fn(), onShow: vi.fn() }),
    );
    await flush();
    unmount();
    expect(l.stop).toHaveBeenCalled();
  });

  it("does nothing without a palette", () => {
    renderHook(() => usePaletteRequests(undefined, true, { onNew: vi.fn(), onShow: vi.fn() }));
  });

  it("shows the view the menu asked for", async () => {
    const l = link([{ kind: "view", view: "settings" }]);
    const onView = vi.fn();
    const onNew = vi.fn();
    const onShow = vi.fn();
    renderHook(() => usePaletteRequests(l.fake, true, { onNew, onShow, onView }));
    await flush();
    expect(onView).toHaveBeenCalledWith("settings");
    l.push({ kind: "view", view: "diagnostics" });
    l.hear();
    await flush();
    expect(onView).toHaveBeenLastCalledWith("diagnostics");
    expect(onNew).not.toHaveBeenCalled();
    expect(onShow).not.toHaveBeenCalled();
  });
});
