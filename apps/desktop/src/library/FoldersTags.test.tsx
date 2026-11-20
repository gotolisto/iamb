// @vitest-environment jsdom
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { createFake, renderLibrary } from "./testkit";

const tree = () => screen.getByRole("tree", { name: "Folders" });

function fakeWith() {
  const fake = createFake({
    folders: [{ name: "Replies" }],
    tags: [{ name: "urgent", color: "red" }, { name: "español", color: "blue" }],
    snippets: [{ abbreviation: ";a", label: "Alpha", body: "a" }],
  });
  return fake;
}

describe("folders", () => {
  it("creates a folder from the keyboard and selects it", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}{Shift>}n{/Shift}{/Control}");
    const dialog = await screen.findByRole("dialog", { name: "New folder" });
    const name = within(dialog).getByLabelText("Name");
    expect(document.activeElement).toBe(name);
    await user.type(name, "Shop replies");
    await user.type(within(dialog).getByLabelText(/^Abbreviation prefix/), ";s");
    await user.keyboard("{Enter}");
    await waitFor(() => expect([...fake.folders.values()].some((f) => f.name === "Shop replies")).toBe(true));
    const created = [...fake.folders.values()].find((f) => f.name === "Shop replies")!;
    expect(created.defaults.abbreviationPrefix).toBe(";s");
    const item = await within(tree()).findByRole("treeitem", { name: /^Shop replies, 0 snippets/ });
    expect(item.getAttribute("aria-selected")).toBe("true");
    expect(await screen.findByText("This folder is empty")).toBeTruthy();
    expect(document.title).toBe("Iamb – Shop replies");
  });

  it("nests folders four levels deep and no deeper", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    const l1 = [...fake.folders.values()][0]!;
    const l2 = fake.addFolder({ name: "Two", parentId: l1.id });
    const l3 = fake.addFolder({ name: "Three", parentId: l2.id });
    fake.addFolder({ name: "Four", parentId: l3.id });
    await renderLibrary(fake);
    const items = await within(tree()).findAllByRole("treeitem");
    expect(items.map((i) => i.getAttribute("aria-level"))).toEqual(["1", "2", "3", "4"]);
    // A new folder cannot go inside the fourth level: it is not offered as a parent.
    await user.keyboard("{Control>}{Shift>}n{/Shift}{/Control}");
    const dialog = await screen.findByRole("dialog", { name: "New folder" });
    const parents = within(dialog).getByLabelText("Inside") as HTMLSelectElement;
    expect([...parents.options].map((o) => o.textContent?.trim())).toEqual(["Top level", "Replies", "Two", "Three"]);
  });

  it("explains a refusal of the service in the dialog", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    fake.api.saveFolder = async () => {
      throw Object.assign(new Error("folder x would be nested 5 levels deep (at most 4)"), { code: "invalid" });
    };
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    await user.click(screen.getByRole("button", { name: "New folder" }));
    await user.type(await screen.findByLabelText("Name"), "Deep");
    await user.keyboard("{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe("Folders can be nested at most 4 levels deep.");
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("navigates the tree with the arrow keys, expands, collapses and types to jump", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    const l1 = [...fake.folders.values()][0]!;
    fake.addFolder({ name: "Returns", parentId: l1.id });
    fake.addFolder({ name: "Zebra" });
    await renderLibrary(fake);
    await within(tree()).findAllByRole("treeitem");
    const [replies] = within(tree()).getAllByRole("treeitem");
    replies!.focus();
    expect(replies!.getAttribute("aria-expanded")).toBe("true");
    await user.keyboard("{ArrowLeft}");
    expect(within(tree()).getAllByRole("treeitem")).toHaveLength(2);
    expect(replies!.getAttribute("aria-expanded")).toBe("false");
    await user.keyboard("{ArrowRight}");
    expect(within(tree()).getAllByRole("treeitem")).toHaveLength(3);
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.getAttribute("aria-label")).toMatch(/^Returns/);
    await user.keyboard("{ArrowLeft}");
    expect(document.activeElement?.getAttribute("aria-label")).toMatch(/^Replies/);
    await user.keyboard("z");
    await waitFor(() => expect(document.activeElement?.getAttribute("aria-label")).toMatch(/^Zebra/));
    await user.keyboard("{Home}");
    await waitFor(() => expect(document.activeElement?.getAttribute("aria-label")).toMatch(/^Replies/));
    await user.keyboard(" ");
    expect(document.title).toBe("Iamb – Replies");
  });

  it("renames a folder with F2 and changes its defaults in Folder settings", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    await renderLibrary(fake);
    const item = await within(tree()).findByRole("treeitem", { name: /^Replies/ });
    item.focus();
    await user.keyboard("{F2}");
    const dialog = await screen.findByRole("dialog", { name: "Folder settings" });
    const name = within(dialog).getByLabelText("Name") as HTMLInputElement;
    expect(name.value).toBe("Replies");
    await user.clear(name);
    await user.type(name, "Answers");
    await user.selectOptions(within(dialog).getByLabelText("New snippets expand"), "Immediately");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect([...fake.folders.values()][0]).toMatchObject({ name: "Answers", defaults: { trigger: "immediate" } }));
    // Focus returns to the folder that was being edited.
    await waitFor(() => expect(within(tree()).getByRole("treeitem", { name: /^Answers/ })).toBeTruthy());
  });

  it("asks before deleting a folder with the specification's words, and deletes its snippets", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    const f = fake.addFolder({ name: "Returns" });
    for (const [i, a] of [";x1", ";x2"].entries()) {
      await fake.api.saveSnippet({
        folderId: f.id, abbreviation: a, label: `L${i}`, kind: "plain", body: "b",
        trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
      });
    }
    await renderLibrary(fake);
    const item = await within(tree()).findByRole("treeitem", { name: /^Returns, 2 snippets/ });
    item.focus();
    await user.keyboard("{Delete}");
    const dialog = await screen.findByRole("alertdialog", { name: "Delete “Returns” and its 2 snippets?" });
    expect(within(dialog).getByText("Other snippets that include one of them will stop working.")).toBeTruthy();
    // Cancel is focused: Enter does not delete.
    await user.keyboard("{Enter}");
    expect(fake.folders.size).toBe(2);
    item.focus();
    await user.keyboard("{Delete}");
    await user.click(await screen.findByRole("button", { name: "Delete" }));
    await waitFor(() => expect(fake.folders.size).toBe(1));
    expect(fake.live.size).toBe(1);
    await waitFor(() => expect(within(tree()).queryByRole("treeitem", { name: /^Returns/ })).toBeNull());
  });

  it("opens a folder's actions from its More button and from the context menu key", async () => {
    const user = userEvent.setup();
    await renderLibrary(fakeWith());
    await user.click(await within(tree()).findByRole("treeitem", { name: /^Replies/ }));
    expect(screen.getByText("Only you")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "More actions for Replies" }));
    const menu = screen.getByRole("menu", { name: "Replies" });
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
      "Folder settings…",
      "New folder inside…",
      "Delete…",
    ]);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("tags", () => {
  it("lists the tags with their colours and counts and filters the list across folders", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    const tag = [...fake.tags.values()].find((t) => t.name === "urgent")!;
    const row = [...fake.live.values()][0]!;
    await fake.api.saveSnippet({ ...row, tags: [tag.id] });
    await fake.api.saveSnippet({ folderId: fake.firstFolder.id, abbreviation: ";b", label: "Beta", kind: "plain", body: "b", trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [] });
    await renderLibrary(fake);
    const button = await screen.findByRole("button", { name: "urgent, red, 1 snippet" });
    expect(button.querySelector(".dot")?.getAttribute("style")).toContain("var(--color-tag-red)");
    await user.click(button);
    const list = await screen.findByRole("listbox", { name: "Snippets in urgent" });
    expect(within(list).getAllByRole("option")).toHaveLength(1);
    expect(document.title).toBe("Iamb – urgent");
  });

  it("creates a tag with one of the eight named colours", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    await renderLibrary(fake);
    await user.click(await screen.findByRole("button", { name: "New tag" }));
    const dialog = await screen.findByRole("dialog", { name: "New tag" });
    const colours = within(dialog).getAllByRole("radio");
    expect(colours.map((c) => c.getAttribute("aria-label"))).toEqual(["red", "orange", "yellow", "green", "teal", "blue", "purple", "grey"]);
    await user.type(within(dialog).getByLabelText("Name"), "billing");
    await user.click(within(dialog).getByRole("radio", { name: "purple" }));
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect([...fake.tags.values()].find((t) => t.name === "billing")?.color).toBe("purple"));
    expect(await screen.findByRole("button", { name: "billing, purple, 0 snippets" })).toBeTruthy();
  });

  it("refuses a name another tag has, without regard to case", async () => {
    const user = userEvent.setup();
    await renderLibrary(fakeWith());
    await user.click(await screen.findByRole("button", { name: "New tag" }));
    await user.type(await screen.findByLabelText("Name"), "URGENT");
    await user.keyboard("{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe("A tag with that name already exists.");
  });

  it("renames and recolours a tag, and deletes it after asking", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    await renderLibrary(fake);
    const button = await screen.findByRole("button", { name: /^urgent, red/ });
    await user.pointer({ keys: "[MouseRight]", target: button });
    await user.click(await screen.findByRole("menuitem", { name: "Rename or recolour…" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit tag" });
    const name = within(dialog).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "now");
    await user.click(within(dialog).getByRole("radio", { name: "green" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("button", { name: "now, green, 0 snippets" })).toBeTruthy();

    await user.pointer({ keys: "[MouseRight]", target: screen.getByRole("button", { name: /^now, green/ }) });
    await user.click(await screen.findByRole("menuitem", { name: "Delete…" }));
    expect(await screen.findByRole("alertdialog", { name: "Delete the tag “now”?" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: /^now, green/ })).toBeNull());
  });
});
