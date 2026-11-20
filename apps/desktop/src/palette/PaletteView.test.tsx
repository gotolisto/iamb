// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ListenPalette, PaletteApi, PaletteEvent, PaletteOpen, PaletteResults } from "./api";
import { DOWN_MS, PaletteView, SLOW_MS } from "./PaletteView";
import type { Os } from "./model";
import { hit, openOf } from "./testkit";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

/** A fake Rust side: the commands, recorded, and the panel's event. */
function harness(open: PaletteOpen | null, os: Os = "win") {
  let current = open;
  let handler: ((e: PaletteEvent) => void) | undefined;
  const answers = new Map<string, PaletteResults>();
  const api = {
    ready: vi.fn(() => Promise.resolve()),
    current: vi.fn(() => Promise.resolve(current)),
    painted: vi.fn(() => Promise.resolve()),
    search: vi.fn((q: string) =>
      Promise.resolve(answers.get(q) ?? { total: 76, hits: [] }),
    ),
    preview: vi.fn((_id: string) => Promise.resolve(null)),
    pick: vi.fn((_s: number, _a: unknown) => Promise.resolve()),
  } satisfies PaletteApi;
  const listen: ListenPalette = (h) => {
    handler = h;
    return Promise.resolve(() => {
      handler = undefined;
    });
  };
  const view = render(
    <PaletteView api={api} listen={listen} os={os} onNextPaint={(report) => report(4242)} />,
  );
  return {
    api,
    view,
    answers,
    setCurrent: (o: PaletteOpen | null) => {
      current = o;
    },
    send: async (e: PaletteEvent) => {
      await act(async () => {
        handler?.(e);
        await Promise.resolve();
      });
    },
  };
}

async function opened(open: PaletteOpen = openOf(), os: Os = "win") {
  const h = harness(open, os);
  await screen.findByRole("dialog", { name: "Search snippets" });
  await h.send({ type: "shown", session: open.session });
  return h;
}

const RET_RESULTS = (): PaletteResults => ({
  total: 76,
  hits: [
    hit(
      { label: "Return label", abbreviation: ";rret" },
      { field: "abbreviation", ranges: [[2, 5]] },
    ),
    hit({ label: "Return refused", abbreviation: ";rno", firstLine: "I am sorry, {field name=Name}" }),
    hit({ label: "Wheel retensioning", abbreviation: ";wten", enabled: false }),
    hit({ label: "Out of stock", abbreviation: ";roos", tags: ["returns"] }, { field: "tag", ranges: [[0, 3]] }),
  ],
});

describe("the search palette", () => {
  it("is a dialog with a combobox that controls a listbox, and an empty hint", async () => {
    await opened();
    const dialog = screen.getByRole("dialog", { name: "Search snippets" });
    const box = within(dialog).getByRole("combobox", { name: "Search snippets" });
    expect(box.getAttribute("aria-controls")).toBe("pl-list");
    expect(within(dialog).getByRole("listbox")).toBeTruthy();
    expect(screen.getByText("Type to search by abbreviation, label, text or tag.")).toBeTruthy();
    expect(screen.getByPlaceholderText("Search snippets")).toBe(box);
  });

  it("is empty before a request, and says once that its page listens", async () => {
    const h = harness(null);
    await waitFor(() => expect(h.api.ready).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("puts the keyboard in the query box and reports the paint once shown", async () => {
    const h = await opened();
    expect(document.activeElement).toBe(screen.getByRole("combobox"));
    expect(h.api.painted).toHaveBeenCalledWith(4, 4242);
  });

  it("handles a shown event that comes before the palette is rendered", async () => {
    const h = harness(null);
    h.setCurrent(openOf({ session: 9 }));
    await h.send({ type: "shown", session: 9 });
    await h.send({ type: "open", session: 9 });
    await screen.findByRole("dialog");
    await waitFor(() => expect(h.api.painted).toHaveBeenCalledWith(9, 4242));
    expect(document.activeElement).toBe(screen.getByRole("combobox"));
  });

  it("searches on every change and shows the count, the rows and the abbreviation on each", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    expect(h.api.search).toHaveBeenLastCalledWith("ret");
    const rows = screen.getAllByRole("option");
    expect(rows).toHaveLength(4);
    expect(rows[0]?.getAttribute("aria-selected")).toBe("true");
    expect(rows[0]?.getAttribute("aria-label")).toBe(
      "Return label, abbreviation semicolon rret, 1 of 4",
    );
    expect(rows[1]?.getAttribute("aria-label")).toContain("asks for values");
    expect(rows[2]?.getAttribute("aria-label")).toContain("off");
    expect(within(rows[2] as HTMLElement).getByText("Off")).toBeTruthy();
    // The fill-in row carries the icon and its footer says what Enter does.
    expect(within(rows[1] as HTMLElement).getByTitle("Asks for values")).toBeTruthy();
  });

  it("marks the matched characters, heavier and tinted, not by colour alone", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    await userEvent.setup().type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    const first = screen.getAllByRole("option")[0] as HTMLElement;
    expect(within(first).getByText("ret", { selector: "mark" })).toBeTruthy();
    // A tag hit marks the tag's name.
    const tagRow = screen.getAllByRole("option")[3] as HTMLElement;
    expect(within(tagRow).getByText("ret", { selector: "mark" })).toBeTruthy();
  });

  it("moves with the arrows (they wrap) and the page keys", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    const selected = () =>
      screen.getAllByRole("option").findIndex((o) => o.getAttribute("aria-selected") === "true");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(selected()).toBe(2);
    await user.keyboard("{ArrowUp}{ArrowUp}{ArrowUp}");
    expect(selected()).toBe(3);
    await user.keyboard("{ArrowDown}");
    expect(selected()).toBe(0);
    await user.keyboard("{PageDown}");
    expect(selected()).toBe(3);
    await user.keyboard("{PageUp}");
    expect(selected()).toBe(0);
    expect(screen.getByRole("combobox").getAttribute("aria-activedescendant")).toBe("pl-opt-0");
  });

  it("types the selected snippet on Enter, after asking Rust to hide the panel and hand the focus back", async () => {
    const h = await opened();
    const results = RET_RESULTS();
    h.answers.set("ret", results);
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    await user.keyboard("{Enter}");
    expect(h.api.pick).toHaveBeenCalledWith(4, { kind: "insert", id: results.hits[0]?.row.id });
    // The palette is over: the query is gone.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("inserts a row with Ctrl+1 to Ctrl+9 (⌘ on a Mac) and edits with Ctrl+E", async () => {
    const h = await opened();
    const results = RET_RESULTS();
    h.answers.set("ret", results);
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    await user.keyboard("{Control>}2{/Control}");
    expect(h.api.pick).toHaveBeenLastCalledWith(4, { kind: "insert", id: results.hits[1]?.row.id });
    cleanup();
    const mac = await opened(openOf({ session: 5 }), "mac");
    mac.answers.set("ret", results);
    await userEvent.setup().type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "e", metaKey: true });
    expect(mac.api.pick).toHaveBeenLastCalledWith(5, { kind: "show", id: results.hits[0]?.row.id });
    // Ctrl does not do it on a Mac.
    mac.api.pick.mockClear();
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "e", ctrlKey: true });
    expect(mac.api.pick).not.toHaveBeenCalled();
  });

  it("closes on Esc and changes nothing else", async () => {
    const h = await opened();
    await userEvent.setup().keyboard("{Escape}");
    expect(h.api.pick).toHaveBeenCalledWith(4, { kind: "close" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(h.api.search).not.toHaveBeenCalled();
  });

  it("footer says what Enter does for the selected row", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    const footer = () => document.querySelector(".pl-ft")?.textContent ?? "";
    expect(footer()).toBe("↑↓ moveEnter insertCtrl+E editEsc close");
    await user.keyboard("{ArrowDown}");
    expect(footer()).toContain("Enter fill in and insert");
  });

  it("has the empty and no-match states of the spec, with New snippet", async () => {
    const h = await opened();
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "warranty");
    expect(await screen.findByText("No snippet matches “warranty”.")).toBeTruthy();
    expect(screen.getByText("0 of 76")).toBeTruthy();
    const only = screen.getByRole("option");
    expect(only.textContent).toContain("New snippet labelled “warranty”");
    expect(only.textContent).toContain("opens the library");
    expect(document.querySelector(".pl-ft")?.textContent).toContain("Enter create");
    await user.keyboard("{Enter}");
    expect(h.api.pick).toHaveBeenCalledWith(4, { kind: "new", label: "warranty" });
  });

  it("offers the last form first when there is one, and opens it pre-filled on Enter", async () => {
    const h = await opened(openOf({ lastForm: "Late delivery" }));
    const row = screen.getByRole("option");
    expect(row.textContent).toContain("Fill in “Late delivery” again");
    expect(row.textContent).toContain("your last answers");
    expect(document.querySelector(".pl-ft")?.textContent).toContain("Enter open the form");
    await userEvent.setup().keyboard("{Enter}");
    expect(h.api.pick).toHaveBeenCalledWith(4, { kind: "fill_again" });
  });

  it("opened from Iamb's own window, Enter selects the snippet in the library", async () => {
    const h = await opened(openOf({ ownWindow: true }));
    const results = RET_RESULTS();
    h.answers.set("ret", results);
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    expect(document.querySelector(".pl-ft")?.textContent).toContain("Enter select");
    await user.keyboard("{Enter}");
    expect(h.api.pick).toHaveBeenCalledWith(4, { kind: "show", id: results.hits[0]?.row.id });
  });

  it("says the focus could not be given back and keeps the palette open", async () => {
    const h = await opened(openOf({ app: "Mail" }));
    h.answers.set("ret", RET_RESULTS());
    h.api.pick.mockRejectedValueOnce({ code: "focus_lost", message: "gone" });
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    await user.keyboard("{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Iamb could not get back to Mail. Click where the text should go and press Enter again.",
    );
    // Enter again.
    await user.keyboard("{Enter}");
    expect(h.api.pick).toHaveBeenCalledTimes(2);
  });

  it("drops an answer to an older query", async () => {
    const h = await opened();
    let late: (r: PaletteResults) => void = () => undefined;
    h.api.search.mockImplementationOnce(
      () =>
        new Promise<PaletteResults>((resolve) => {
          late = resolve;
        }),
    );
    h.answers.set("re", { total: 76, hits: [hit({ label: "Current answer" })] });
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox"), "r");
    await user.type(screen.getByRole("combobox"), "e");
    await screen.findByText("Current answer");
    await act(async () => {
      late({ total: 76, hits: [hit({ label: "Stale answer" })] });
      await Promise.resolve();
    });
    expect(screen.queryByText("Stale answer")).toBeNull();
    expect(screen.getByText("Current answer")).toBeTruthy();
  });

  it("forgets the query when the palette closes, and opens empty the next time", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    await userEvent.setup().type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    await h.send({ type: "closed", session: 4 });
    expect(screen.queryByRole("dialog")).toBeNull();
    h.setCurrent(openOf({ session: 5 }));
    await h.send({ type: "open", session: 5 });
    const box = await screen.findByRole<HTMLInputElement>("combobox");
    expect(box.value).toBe("");
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(document.body.textContent).not.toContain("Return label");
  });

  it("keeps no copy of the query in the web view's storage", async () => {
    const h = await opened();
    h.answers.set("zqxcanary", { total: 76, hits: [] });
    await userEvent.setup().type(screen.getByRole("combobox"), "zqxcanary");
    await screen.findByText("0 of 76");
    expect(JSON.stringify({ ...window.localStorage })).not.toContain("zqxcanary");
    expect(JSON.stringify({ ...window.sessionStorage })).not.toContain("zqxcanary");
    expect(document.cookie).not.toContain("zqxcanary");
    const box = screen.getByRole("combobox");
    expect(box.getAttribute("autocomplete")).toBe("off");
    expect(box.getAttribute("spellcheck")).toBe("false");
  });

  it("announces the number of results and the selected row", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    await userEvent.setup().type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    expect(screen.getByRole("status").textContent).toBe(
      "4 results, Return label, abbreviation semicolon rret",
    );
  });
});

describe("a service that does not answer", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  it("dims the previous results after 300 ms and says it is not running after 2 s", async () => {
    const h = await opened();
    h.answers.set("ret", RET_RESULTS());
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    await user.type(screen.getByRole("combobox"), "ret");
    await screen.findByText("4 of 76");
    h.api.search.mockImplementation(() => new Promise(() => undefined));
    await user.type(screen.getByRole("combobox"), "u");
    await act(async () => {
      vi.advanceTimersByTime(SLOW_MS + 10);
    });
    expect(document.querySelector(".pl-list")?.getAttribute("data-dim")).toBe("true");
    expect(screen.queryByText("Iamb is not running.")).toBeNull();
    await act(async () => {
      vi.advanceTimersByTime(DOWN_MS);
    });
    expect(screen.getByText("Iamb is not running.")).toBeTruthy();
    // "Start it again" asks again.
    h.api.search.mockClear();
    await user.click(screen.getByRole("button", { name: "Start it again" }));
    expect(h.api.search).toHaveBeenCalledWith("retu");
  });

  it("says so at once when the search fails", async () => {
    const h = await opened();
    h.api.search.mockRejectedValue({ code: "disconnected", message: "gone" });
    await userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) }).type(
      screen.getByRole("combobox"),
      "x",
    );
    expect(await screen.findByText("Iamb is not running.")).toBeTruthy();
  });
});
