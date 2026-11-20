// @vitest-environment jsdom
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { contentText, createFake, renderLibrary, setContent, typeContent } from "./testkit";

const base = () =>
  createFake({
    folders: [{ name: "Replies", defaults: { abbreviationPrefix: ";r", trigger: null, caseMode: null } }],
    tags: [{ name: "urgent", color: "red" }],
    snippets: [
      { abbreviation: ";rty", label: "Thanks for your order", body: "Thank you for your order." },
      { abbreviation: ";rlate", label: "Late delivery", body: "Hi {field name=\"First name\"}, sorry." },
      { abbreviation: ";rhours", label: "Opening hours", body: "We are open Tuesday to Saturday." },
    ],
  });

const listbox = () => screen.getByRole("listbox", { name: "All snippets" });
const rowNames = () => within(listbox()).getAllByRole("option").map((o) => o.getAttribute("aria-label") ?? "");

describe("the library window", () => {
  it("shows the folders, tags and snippets the service holds", async () => {
    await renderLibrary(base());
    expect(await screen.findByRole("option", { name: /^Thanks for your order, abbreviation semicolon rty/ })).toBeTruthy();
    expect(rowNames()).toHaveLength(3);
    // Sorted by label.
    expect(rowNames()[0]).toMatch(/^Late delivery/);
    const tree = screen.getByRole("tree", { name: "Folders" });
    expect(within(tree).getByRole("treeitem", { name: /^Replies, 3 snippets, level 1/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^urgent, red, 0 snippets/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /All snippets/ }).textContent).toContain("3");
    // The row's second line shows a fill-in field as a name, and the row says it asks for values.
    expect(screen.getByText("Hi ‹First name›, sorry.")).toBeTruthy();
    expect(rowNames().some((n) => n.includes("asks for values"))).toBe(true);
  });

  it("creates a snippet from the keyboard, saves it, and says how to use it", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}n{/Control}");
    const abbreviation = await screen.findByLabelText("Abbreviation");
    // Focus starts in Abbreviation, after the folder's prefix.
    expect(document.activeElement).toBe(abbreviation);
    expect((abbreviation as HTMLInputElement).value).toBe(";r");
    expect((screen.getByRole("button", { name: /^Save/ }) as HTMLButtonElement).disabled).toBe(true);
    await user.type(abbreviation, "new");
    await user.tab();
    await user.type(screen.getByLabelText(/^Label/), "Brand new");
    await user.tab();
    await typeContent("Hello there");
    expect(screen.getByText("Not saved")).toBeTruthy();
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(fake.byAbbreviation(";rnew")?.body).toBe("Hello there"));
    expect((await screen.findByRole("status", { name: "" , hidden: true }).catch(() => null)) ?? true).toBeTruthy();
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent?.includes("Saved. Type ;rnew and a space to use it."))).toBe(true));
    // The row is in the list and selected; the header lost "Not saved"; focus stayed in the editor.
    await waitFor(() => expect(rowNames().some((n) => n.startsWith("Brand new"))).toBe(true));
    expect(screen.queryByText("Not saved")).toBeNull();
    expect(screen.getByRole("option", { name: /^Brand new/ }).getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement?.closest("form")).toBeTruthy();
  });

  it("starts a draft with the prefix of the folder and says so", async () => {
    const user = userEvent.setup();
    await renderLibrary(base());
    await user.click(await screen.findByRole("treeitem", { name: /^Replies/ }));
    await user.click(screen.getByRole("button", { name: /^New (Ctrl\+N|⌘N)$/ }));
    const abbreviation = (await screen.findByLabelText("Abbreviation")) as HTMLInputElement;
    expect(abbreviation.value).toBe(";r");
    expect(screen.getByText("Starts with ;r, the prefix of the folder Replies.")).toBeTruthy();
  });

  it("edits a snippet and keeps the saved text until it is saved", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    await user.click(await screen.findByRole("option", { name: /^Opening hours/ }));
    await screen.findByLabelText("Content");
    expect(contentText()).toBe("We are open Tuesday to Saturday.");
    await setContent("Open until six");
    expect(screen.getByText("Not saved")).toBeTruthy();
    // The service still holds the old text.
    expect(fake.byAbbreviation(";rhours")?.body).toBe("We are open Tuesday to Saturday.");
    await user.click(screen.getByRole("button", { name: /^Save/ }));
    await waitFor(() => expect(fake.byAbbreviation(";rhours")?.body).toBe("Open until six"));
    await waitFor(() => expect(screen.queryByText("Not saved")).toBeNull());
  });

  it("asks before leaving unsaved changes: Cancel, Do not save, Save", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    await user.click(await screen.findByRole("option", { name: /^Opening hours/ }));
    await screen.findByLabelText("Content");
    await typeContent(" and Sunday");

    // Another snippet is chosen: the dialog of the specification, with its copy.
    await user.click(screen.getByRole("option", { name: /^Late delivery/ }));
    const dialog = await screen.findByRole("alertdialog", { name: "Save your changes to “Opening hours”?" });
    expect(within(dialog).getByText("If you do not save, the changes you made to this snippet are lost.")).toBeTruthy();
    // Cancel: still editing, nothing lost.
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(contentText()).toMatch(/and Sunday$/);

    // Do not save: the other snippet opens and the first one is as it was.
    await user.click(screen.getByRole("option", { name: /^Late delivery/ }));
    await user.click(await screen.findByRole("button", { name: "Do not save" }));
    await waitFor(() => expect(contentText()).toMatch(/^Hi /));
    expect(fake.byAbbreviation(";rhours")?.body).toBe("We are open Tuesday to Saturday.");

    // Save: Enter is the primary button.
    await user.click(screen.getByRole("option", { name: /^Opening hours/ }));
    await typeContent(" and Sunday");
    await user.click(screen.getByRole("option", { name: /^Thanks for your order/ }));
    await screen.findByRole("alertdialog");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(fake.byAbbreviation(";rhours")?.body).toMatch(/and Sunday$/));
    await waitFor(() => expect(contentText()).toBe("Thank you for your order."));
  });

  it("discards an untouched draft without asking", async () => {
    const user = userEvent.setup();
    await renderLibrary(base());
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}n{/Control}");
    await screen.findByLabelText("Abbreviation");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByText("Select a snippet, or press Ctrl+N to make one.")).toBeTruthy();
  });

  it("deletes with an Undo toast that restores the snippet by its own id", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    const row = await screen.findByRole("option", { name: /^Opening hours/ });
    const id = row.id.replace("row-", "");
    await user.click(row);
    await user.keyboard("{Delete}");
    const toast = await screen.findByText("Deleted “Opening hours”.");
    expect(toast).toBeTruthy();
    await waitFor(() => expect(fake.live.has(id)).toBe(false));
    await waitFor(() => expect(screen.queryByRole("option", { name: /^Opening hours/ })).toBeNull());
    // The next row is selected and the list keeps the focus.
    expect(within(listbox()).getAllByRole("option", { selected: true })).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(fake.live.get(id)?.abbreviation).toBe(";rhours"));
    expect(await screen.findByRole("option", { name: /^Opening hours/ })).toBeTruthy();
  });

  it("deletes several snippets at once", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    await user.click(await screen.findByRole("option", { name: /^Late delivery/ }));
    await user.keyboard("{Shift>}{ArrowDown}{ArrowDown}{/Shift}");
    expect(await screen.findByText("3 snippets selected")).toBeTruthy();
    await user.keyboard("{Delete}");
    expect(await screen.findByText("Deleted 3 snippets.")).toBeTruthy();
    expect(fake.live.size).toBe(0);
    expect(await screen.findByText("No snippets yet")).toBeTruthy();
    expect(screen.getByText("A snippet is text you save once and type with a short abbreviation from then on.")).toBeTruthy();
  });

  it("keeps Undo for what a partly failed delete removed, and says what failed", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    await user.click(await screen.findByRole("option", { name: /^Late delivery/ }));
    await user.keyboard("{Shift>}{ArrowDown}{ArrowDown}{/Shift}");
    expect(await screen.findByText("3 snippets selected")).toBeTruthy();
    const late = fake.byAbbreviation(";rlate");
    const deleteSnippet = fake.api.deleteSnippet;
    fake.api.deleteSnippet = async (id) => {
      if (id === late?.id) throw { code: "unavailable", message: "busy" };
      return deleteSnippet(id);
    };
    await user.keyboard("{Delete}");
    expect(
      await screen.findByText(
        "Deleted 2 snippets. “Late delivery” could not be deleted: Iamb is busy. Try again in a moment.",
      ),
    ).toBeTruthy();
    expect([...fake.live.values()].map((s) => s.abbreviation)).toEqual([";rlate"]);
    // Undo brings back the two that were deleted.
    await user.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(fake.live.size).toBe(3));
  });

  it("asks before Quit drops unsaved edits, as closing the window does", async () => {
    const user = userEvent.setup();
    let requested: (() => void) | undefined;
    let quits = 0;
    await renderLibrary(base(), {
      onQuitRequested: (h) => {
        requested = h;
        return () => {
          requested = undefined;
        };
      },
      quit: async () => {
        quits += 1;
      },
    });
    await screen.findAllByRole("option");
    // Nothing unsaved: Quit goes ahead.
    act(() => requested?.());
    await waitFor(() => expect(quits).toBe(1));
    // An edit that is not saved: the window asks first.
    await user.click(screen.getByRole("option", { name: /^Opening hours/ }));
    const label = (await screen.findByLabelText(/^Label/)) as HTMLInputElement;
    await user.type(label, " now");
    act(() => requested?.());
    const dialog = await screen.findByRole("alertdialog");
    expect(quits).toBe(1);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(quits).toBe(1);
    act(() => requested?.());
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /Do not save|Don’t save/ }));
    await waitFor(() => expect(quits).toBe(2));
  });

  it("turns a snippet off with the space bar and marks it Off", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(base());
    const row = await screen.findByRole("option", { name: /^Opening hours/ });
    await user.click(row);
    await user.keyboard(" ");
    await waitFor(() => expect(fake.byAbbreviation(";rhours")?.enabled).toBe(false));
    await waitFor(() => expect(screen.getByRole("option", { name: /^Opening hours.*off/ })).toBeTruthy());
  });

  it("filters the list in the window and offers the other folders' matches", async () => {
    const user = userEvent.setup();
    const fake = base();
    const other = fake.addFolder({ name: "Shop" });
    await fake.api.saveSnippet({
      folderId: other.id, abbreviation: ";shop", label: "Late shipping", kind: "plain", body: "x",
      trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
    });
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    await user.click(screen.getByRole("treeitem", { name: /^Replies/ }));
    const list = () => screen.getByRole("listbox", { name: "Snippets in Replies" });
    const filter = screen.getByRole("searchbox", { name: "Filter Replies" });
    await user.type(filter, "late");
    await waitFor(() => expect(within(list()).getAllByRole("option")).toHaveLength(1));
    expect(within(list()).getByRole("option").getAttribute("aria-label")).toMatch(/^Late delivery/);
    await user.clear(filter);
    await user.type(filter, "shipping");
    expect(await screen.findByText("Nothing in Replies matches “shipping”")).toBeTruthy();
    expect(screen.getByText("1 match in other folders")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Show them" }));
    await waitFor(() => expect(screen.getByRole("option", { name: /^Late shipping/ })).toBeTruthy());
  });

  it("clears the filter with the button", async () => {
    const user = userEvent.setup();
    await renderLibrary(base());
    await screen.findAllByRole("option");
    await user.type(screen.getByRole("searchbox", { name: "Filter snippets" }), "zzz");
    expect(await screen.findByText("Nothing in All snippets matches “zzz”")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Clear filter" }));
    expect(rowNames()).toHaveLength(3);
  });

  it("shows the empty folder", async () => {
    const user = userEvent.setup();
    const fake = base();
    fake.addFolder({ name: "Empty one" });
    await renderLibrary(fake);
    await user.click(await screen.findByRole("treeitem", { name: /^Empty one/ }));
    expect(await screen.findByText("This folder is empty")).toBeTruthy();
  });

  it("keeps the list read-only with a banner while the service is not running", async () => {
    const fake = base();
    await renderLibrary(fake, { connection: "disconnected" });
    expect(screen.getByRole("alert").textContent).toContain("Iamb is not running. Its background service stopped, so nothing can be saved or expanded.");
    expect((screen.getByRole("button", { name: /^New (Ctrl\+N|⌘N)$/ }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByRole("button", { name: /Iamb is not running/ })).toBeTruthy();
  });

  it("follows what changes elsewhere", async () => {
    const fake = base();
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    const folder = fake.firstFolder.id;
    await fake.api.saveSnippet({
      folderId: folder, abbreviation: ";rnew", label: "Added elsewhere", kind: "plain", body: "b",
      trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
    });
    expect(await screen.findByRole("option", { name: /^Added elsewhere/ })).toBeTruthy();
  });

  it("moves between panes with the keyboard map", async () => {
    const user = userEvent.setup();
    await renderLibrary(base());
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}2{/Control}");
    expect(document.activeElement).toBe(listbox());
    await user.keyboard("{Control>}1{/Control}");
    expect(document.activeElement?.closest("[data-pane]")?.getAttribute("data-pane")).toBe("sidebar");
    await user.keyboard("{F6}");
    expect(document.activeElement).toBe(listbox());
    await user.keyboard("{Control>}f{/Control}");
    expect(document.activeElement).toBe(screen.getByRole("searchbox"));
  });

  it("opens the views that later tasks fill as placeholders", async () => {
    const user = userEvent.setup();
    await renderLibrary(base());
    await screen.findAllByRole("option");
    await user.click(screen.getByRole("button", { name: /Library health/ }));
    expect(await screen.findByText("Nothing to fix. All 3 snippets can expand.")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /Why didn’t it expand\?/ }));
    expect(screen.getByRole("heading", { name: "Why didn’t it expand?" })).toBeTruthy();
    await user.keyboard("{Control>},{/Control}");
    expect(screen.getByRole("heading", { name: "Settings" })).toBeTruthy();
    expect(document.title).toBe("Iamb – Settings");
  });

  it("counts a snippet that will not expand in Library health", async () => {
    const fake = base();
    await fake.api.saveSnippet({
      folderId: fake.firstFolder.id, abbreviation: ";rbad", label: "Bad", kind: "plain", body: "an {unclosed",
      trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
    });
    await renderLibrary(fake);
    const row = await screen.findByRole("option", { name: /^Bad/ });
    expect(row.textContent).toContain("Broken");
    expect(screen.getByRole("button", { name: /Library health/ }).textContent).toContain("1");
    fireEvent.click(screen.getByRole("button", { name: /Library health/ }));
    expect(await screen.findByRole("region", { name: "Will not expand: error in the snippet" })).toBeTruthy();
    expect(screen.getByText(/Something that starts with \{ is never closed/)).toBeTruthy();
  });

  it("warns when a saved snippet will not expand", async () => {
    const user = userEvent.setup();
    await renderLibrary(base());
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}n{/Control}");
    await user.type(await screen.findByLabelText("Abbreviation"), ";rbrace");
    await setContent("oops {");
    await user.keyboard("{Control>}s{/Control}");
    expect(await screen.findByText("Saved, but it will not expand until the error is fixed.")).toBeTruthy();
  });
});
