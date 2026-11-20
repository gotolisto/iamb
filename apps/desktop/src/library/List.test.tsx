// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { committed } from "../../scripts/tokens.mjs";
import css from "./library.css?raw";
import type { Summary } from "./api";
import { fetchAllSummaries } from "./useLibraryData";
import { VirtualList } from "./VirtualList";
import { createFake, renderLibrary, snippetOf } from "./testkit";

function many(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    abbreviation: `;s${i}`,
    label: `Snippet ${String(i).padStart(5, "0")}`,
    body: `Body of number ${i}`,
  }));
}

describe("the virtual list", () => {
  const rows = Array.from({ length: 10_000 }, (_, i) => i);
  const view = (first = 0) => (
    <VirtualList
      count={rows.length}
      rowHeight={48}
      label="Rows"
      activeIndex={first}
      rowId={(i) => `r${i}`}
      renderRow={(i) => (
        <div key={i} id={`r${i}`} role="option" aria-selected={false} style={{ height: 48 }}>
          Row {i}
        </div>
      )}
      onKeyDown={() => {}}
    />
  );

  it("draws only the rows in view for 10,000 snippets", () => {
    render(view());
    const options = screen.getAllByRole("option");
    // 600 px of rows, and a few above and below.
    expect(options.length).toBeLessThan(40);
    expect(options[0]?.textContent).toBe("Row 0");
    const space = screen.getByRole("listbox").firstElementChild as HTMLElement;
    expect(space.style.height).toBe(`${10_000 * 48}px`);
  });

  it("moves its window when it scrolls, and keeps the rows it draws in step", () => {
    render(view());
    const list = screen.getByRole("listbox");
    list.scrollTop = 48 * 5000;
    fireEvent.scroll(list);
    const options = screen.getAllByRole("option");
    expect(options.length).toBeLessThan(40);
    expect(options.map((o) => o.textContent)).toContain("Row 5000");
    expect(options[0]?.textContent).toBe("Row 4994");
    const window = list.querySelector(".vlist-window") as HTMLElement;
    expect(window.style.transform).toBe(`translateY(${4994 * 48}px)`);
  });

  it("does not render again while a scroll stays inside the same row", () => {
    let renders = 0;
    render(
      <VirtualList
        count={rows.length}
        rowHeight={48}
        label="Rows"
        activeIndex={-1}
        rowId={(i) => `r${i}`}
        renderRow={(i) => {
          renders += 1;
          return <div key={i} id={`r${i}`} role="option" aria-selected={false} />;
        }}
        onKeyDown={() => {}}
      />,
    );
    const list = screen.getByRole("listbox");
    list.scrollTop = 48 * 100;
    fireEvent.scroll(list);
    const after = renders;
    for (let y = 1; y < 40; y++) {
      list.scrollTop = 48 * 100 + y;
      fireEvent.scroll(list);
    }
    expect(renders).toBe(after);
  });

  it("names the active row for assistive technology without moving the focus", () => {
    render(view(42));
    const list = screen.getByRole("listbox");
    expect(list.getAttribute("aria-activedescendant")).toBe("r42");
    expect(list.tabIndex).toBe(0);
  });
});

describe("the snippet list", () => {
  it("reads 2,500 snippets in three pages of the service", async () => {
    const fake = createFake({ snippets: many(2500) });
    const all = await fetchAllSummaries(fake.api);
    expect(all).toHaveLength(2500);
    expect(fake.calls.filter((c) => c.startsWith("listSummaries"))).toEqual([
      "listSummaries 0",
      "listSummaries 1000",
      "listSummaries 2000",
    ]);
    expect(new Set(all.map((s: Summary) => s.id)).size).toBe(2500);
  });

  it("shows 10,000 snippets with only a screenful of rows in the page, and the true count", async () => {
    const fake = createFake();
    for (const s of many(10_000)) {
      const sn = snippetOf(s, fake.firstFolder.id);
      fake.live.set(sn.id, sn);
    }
    await renderLibrary(fake);
    const list = await screen.findByRole("listbox", { name: "All snippets" });
    await waitFor(() => expect(within(list).getAllByRole("option").length).toBeGreaterThan(5));
    expect(within(list).getAllByRole("option").length).toBeLessThan(40);
    const first = within(list).getAllByRole("option")[0]!;
    expect(first.getAttribute("aria-setsize")).toBe("10000");
    expect(first.getAttribute("aria-label")).toMatch(/1 of 10000$/);
    expect(screen.getByRole("button", { name: /All snippets/ }).textContent).toContain("10000");
  });

  it("filters 10,000 snippets as the person types", async () => {
    const user = userEvent.setup();
    const fake = createFake();
    for (const s of many(10_000)) {
      const sn = snippetOf(s, fake.firstFolder.id);
      fake.live.set(sn.id, sn);
    }
    await renderLibrary(fake);
    await screen.findByRole("listbox", { name: "All snippets" });
    await user.type(screen.getByRole("searchbox"), "09999");
    await waitFor(() => expect(screen.getAllByRole("option", { name: /^Snippet 09999/ })).toHaveLength(1));
    expect(within(screen.getByRole("listbox")).getAllByRole("option")).toHaveLength(1);
  });

  it("moves with the arrow keys, Home, End and Page keys, and scrolls to the row", async () => {
    const user = userEvent.setup();
    const fake = createFake({ snippets: many(300) });
    await renderLibrary(fake);
    const list = await screen.findByRole("listbox", { name: "All snippets" });
    await within(list).findAllByRole("option");
    list.focus();
    await user.keyboard("{ArrowDown}");
    expect(list.getAttribute("aria-activedescendant")).toMatch(/^row-/);
    const first = within(list).getAllByRole("option", { selected: true })[0]!;
    expect(first.getAttribute("aria-label")).toMatch(/^Snippet 00000/);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label")).toMatch(/^Snippet 00002/);
    await user.keyboard("{End}");
    await waitFor(() => expect(within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label")).toMatch(/^Snippet 00299/));
    expect(list.scrollTop).toBeGreaterThan(0);
    await user.keyboard("{Home}");
    await waitFor(() => expect(within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label")).toMatch(/^Snippet 00000/));
    await user.keyboard("{PageDown}");
    const selected = within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label") ?? "";
    expect(Number(/Snippet (\d+)/.exec(selected)?.[1])).toBeGreaterThan(5);
  });

  it("jumps to the label that starts with what is typed", async () => {
    const user = userEvent.setup();
    const fake = createFake({
      snippets: [
        { abbreviation: ";a", label: "Apple", body: "." },
        { abbreviation: ";b", label: "Banana", body: "." },
        { abbreviation: ";c", label: "Cherry", body: "." },
        { abbreviation: ";ch", label: "Chocolate", body: "." },
      ],
    });
    await renderLibrary(fake);
    const list = await screen.findByRole("listbox", { name: "All snippets" });
    await within(list).findAllByRole("option");
    list.focus();
    await user.keyboard("c");
    expect(within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label")).toMatch(/^Cherry/);
    await user.keyboard("h");
    expect(within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label")).toMatch(/^Cherry/);
    await user.keyboard("o");
    expect(within(list).getAllByRole("option", { selected: true })[0]?.getAttribute("aria-label")).toMatch(/^Chocolate/);
  });

  it("selects a range with Shift and adds with Ctrl", async () => {
    const user = userEvent.setup();
    const fake = createFake({ snippets: many(6) });
    await renderLibrary(fake);
    const list = await screen.findByRole("listbox", { name: "All snippets" });
    const options = await within(list).findAllByRole("option");
    await user.click(options[1]!);
    await user.keyboard("{Shift>}");
    await user.click(options[3]!);
    await user.keyboard("{/Shift}");
    expect(within(list).getAllByRole("option", { selected: true })).toHaveLength(3);
    await user.keyboard("{Control>}");
    await user.click(options[5]!);
    await user.keyboard("{/Control}");
    expect(within(list).getAllByRole("option", { selected: true })).toHaveLength(4);
    expect(await screen.findByText("4 snippets selected")).toBeTruthy();
  });

  it("opens the context menu of a row and runs its actions", async () => {
    const user = userEvent.setup();
    const fake = createFake({ snippets: [{ abbreviation: ";a", label: "Apple", body: "." }] });
    await renderLibrary(fake);
    const row = await screen.findByRole("option", { name: /^Apple/ });
    await user.pointer({ keys: "[MouseRight]", target: row });
    const menu = await screen.findByRole("menu", { name: "Apple" });
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Open", "Turn off", "DuplicateCtrl+D", "Delete"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Turn off" }));
    await waitFor(() => expect([...fake.live.values()][0]?.enabled).toBe(false));
    await act(async () => {});
  });

  it("sorts by recently changed on request", async () => {
    const user = userEvent.setup();
    const fake = createFake({
      snippets: [
        { abbreviation: ";a", label: "Apple", body: ".", updatedAt: 1 },
        { abbreviation: ";b", label: "Banana", body: ".", updatedAt: 9 },
      ],
    });
    await renderLibrary(fake);
    const list = await screen.findByRole("listbox", { name: "All snippets" });
    await within(list).findAllByRole("option");
    expect(within(list).getAllByRole("option")[0]?.getAttribute("aria-label")).toMatch(/^Apple/);
    await user.selectOptions(screen.getByLabelText("Sort by"), "Recently changed");
    expect(within(list).getAllByRole("option")[0]?.getAttribute("aria-label")).toMatch(/^Banana/);
  });

  it("remembers the sort and the selected snippet for the next time the window opens", async () => {
    const user = userEvent.setup();
    const fake = createFake({ snippets: [{ abbreviation: ";a", label: "Apple", body: "." }, { abbreviation: ";b", label: "Banana", body: "." }] });
    const first = await renderLibrary(fake);
    await user.selectOptions(screen.getByLabelText("Sort by"), "Recently changed");
    await user.click(await screen.findByRole("option", { name: /^Banana/ }));
    first.unmount();
    await renderLibrary(fake);
    expect((screen.getByLabelText("Sort by") as HTMLSelectElement).value).toBe("changed");
    await waitFor(() => expect(screen.getByRole("option", { name: /^Banana/ }).getAttribute("aria-selected")).toBe("true"));
  });
});

describe("the stylesheet", () => {
  it("uses only tokens that exist", () => {
    const tokens = new Set([...committed().matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]));
    const used = new Set([...css.matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]));
    expect([...used].filter((t) => !tokens.has(t as string))).toEqual([]);
    // The component code names tag colours through tokens too.
    for (const color of ["red", "orange", "yellow", "green", "teal", "blue", "purple", "grey"]) {
      expect(tokens.has(`--color-tag-${color}`)).toBe(true);
    }
  });
});
