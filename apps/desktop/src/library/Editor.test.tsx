// @vitest-environment jsdom
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { contentText, createFake, renderLibrary, typeContent } from "./testkit";

const fakeWith = () =>
  createFake({
    folders: [{ name: "Replies", defaults: { abbreviationPrefix: ";r", trigger: null, caseMode: null } }],
    tags: [
      { name: "urgent", color: "red" },
      { name: "español", color: "orange" },
    ],
    snippets: [
      { abbreviation: ";rty", label: "Thanks for your order", body: "Thank you." },
      { abbreviation: ";rlate", label: "Late delivery", body: "Sorry." },
      { abbreviation: ";rl", label: "Reply later", body: "Later.", trigger: "immediate", wordStart: false },
    ],
  });

async function openNew() {
  const user = userEvent.setup();
  const ctx = await renderLibrary(fakeWith());
  await screen.findAllByRole("option");
  await user.keyboard("{Control>}n{/Control}");
  const abbreviation = (await screen.findByLabelText("Abbreviation")) as HTMLInputElement;
  return { user, abbreviation, ...ctx };
}

const saveButton = () => screen.getByRole("button", { name: /^Save/ }) as HTMLButtonElement;

describe("the abbreviation's messages", () => {
  it("blocks an abbreviation that is in use and shows that snippet", async () => {
    const { user, abbreviation } = await openNew();
    await user.type(abbreviation, "ty");
    const alert = await screen.findByRole("alert", { name: /already the abbreviation/ });
    expect(alert.textContent).toContain(
      ";rty is already the abbreviation of “Thanks for your order” in Replies. Each abbreviation can belong to one snippet.",
    );
    expect(abbreviation.getAttribute("aria-invalid")).toBe("true");
    await typeContent("text");
    expect(saveButton().disabled).toBe(true);
    // [Show that snippet] opens it.
    // The draft has changes, so leaving it asks first.
    await user.click(within(alert).getByRole("button", { name: "Show that snippet" }));
    await user.click(await screen.findByRole("button", { name: "Do not save" }));
    await waitFor(() => expect((screen.getByLabelText("Abbreviation") as HTMLInputElement).value).toBe(";rty"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("says what an empty abbreviation or one with a space needs", async () => {
    const { user, abbreviation } = await openNew();
    await user.type(abbreviation, "x");
    await user.clear(abbreviation);
    expect((await screen.findByRole("alert")).textContent).toContain("Type an abbreviation.");
    await user.type(abbreviation, "a b");
    expect((await screen.findByRole("alert")).textContent).toContain("An abbreviation cannot contain spaces.");
    expect(saveButton().disabled).toBe(true);
    fireEvent.change(abbreviation, { target: { value: ";" + "x".repeat(64) } });
    expect((await screen.findByRole("alert")).textContent).toContain("An abbreviation can be at most 64 characters.");
  });

  it("warns, without blocking, that an immediate abbreviation hides a longer one", async () => {
    const { user, abbreviation } = await openNew();
    await user.keyboard("{Control>}{Shift>}o{/Shift}{/Control}");
    await user.click(screen.getByRole("radio", { name: /^Immediately/ }));
    await user.clear(abbreviation);
    await user.type(abbreviation, ";rla");
    // `;rlate` starts with `;rla`.
    const warning = await screen.findByText(/will fire before you can finish typing/);
    expect(warning.closest("p")?.textContent).toContain(
      "This snippet expands immediately, so ;rla will fire before you can finish typing ;rlate. Use a different abbreviation, or let it wait for a space.",
    );
    await typeContent("x");
    expect(saveButton().disabled).toBe(false);
  });

  it("warns when another immediate snippet would hide this one", async () => {
    const { user, abbreviation } = await openNew();
    // `;rl` is immediate and `;rlater` starts with it.
    await user.type(abbreviation, "later");
    const warning = await screen.findByText(/expands immediately and will fire before/);
    expect(warning.closest("p")?.textContent).toBe(
      ";rl expands immediately and will fire before ;rlater can be typed. Change one of the two.",
    );
  });

  it("warns that short immediate abbreviations made of letters fire in ordinary typing", async () => {
    const { user, abbreviation } = await openNew();
    await user.keyboard("{Control>}{Shift>}o{/Shift}{/Control}");
    await user.click(screen.getByRole("radio", { name: /^Immediately/ }));
    await user.clear(abbreviation);
    await user.type(abbreviation, "ty");
    expect(
      await screen.findByText(
        "Short abbreviations made only of letters can fire in the middle of ordinary typing. Starting with a character such as ; avoids that.",
      ),
    ).toBeTruthy();
  });

  it("answers a conflict from the service with the same message", async () => {
    const { user, abbreviation, fake } = await openNew();
    await user.type(abbreviation, "unique");
    await typeContent("x");
    // Someone else takes it between the check and the save.
    await fake.api.saveSnippet({
      folderId: fake.firstFolder.id, abbreviation: ";runique", label: "Taken", kind: "plain", body: "t",
      trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
    });
    await user.click(saveButton());
    const alert = await screen.findByRole("alert", { name: /already the abbreviation of “Taken”/ });
    expect(alert).toBeTruthy();
    expect(screen.getByText("Not saved")).toBeTruthy();
  });
});

describe("the editor's fields", () => {
  it("tags a snippet with an existing tag and a new one", async () => {
    const { user, abbreviation, fake } = await openNew();
    await user.type(abbreviation, "tagged");
    await typeContent("x");
    const tags = screen.getByLabelText(/^Tags/, { selector: "input" });
    await user.type(tags, "urg");
    await user.click(await screen.findByRole("option", { name: "urgent" }));
    await user.type(tags, "billing");
    await user.click(await screen.findByRole("option", { name: "Create tag “billing”" }));
    expect(await screen.findByRole("button", { name: "Remove tag billing" })).toBeTruthy();
    await user.click(saveButton());
    await waitFor(() => expect(fake.byAbbreviation(";rtagged")?.tags).toHaveLength(2));
    // The new tag got one of the eight colours and shows in the sidebar with its count.
    expect(await screen.findByRole("button", { name: /^billing, \w+, 1 snippet/ })).toBeTruthy();
    // Backspace in the empty field removes the last tag.
    await user.click(screen.getByLabelText(/^Tags/, { selector: "input" }));
    await user.keyboard("{Backspace}");
    expect(screen.queryByRole("button", { name: "Remove tag billing" })).toBeNull();
    expect(screen.getByText("Not saved")).toBeTruthy();
  });

  it("follows the folder's prefix when the folder changes before anything is typed", async () => {
    const fake = fakeWith();
    fake.addFolder({ name: "Shop", defaults: { abbreviationPrefix: ";s", trigger: null, caseMode: null } });
    const user = userEvent.setup();
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}n{/Control}");
    const abbreviation = (await screen.findByLabelText("Abbreviation")) as HTMLInputElement;
    expect(abbreviation.value).toBe(";r");
    await user.selectOptions(screen.getByLabelText("Folder"), "Shop");
    expect(abbreviation.value).toBe(";s");
    expect(screen.getByText("Starts with ;s, the prefix of the folder Shop.")).toBeTruthy();
  });

  it("shows the options in words and changes them with the Options group", async () => {
    const { user, abbreviation, fake } = await openNew();
    expect(screen.getByText(/Expands after a space or punctuation · Case: ignore/)).toBeTruthy();
    const toggle = screen.getByRole("button", { name: "Options" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(toggle);
    const group = screen.getByRole("group", { name: "Options" });
    expect(within(group).getByText("When the word is finished. Enter and Tab do not count.")).toBeTruthy();
    await user.click(within(group).getByRole("radio", { name: /^Ignore, and follow mine/ }));
    await user.click(within(group).getByRole("radio", { name: /^Immediately/ }));
    // "Only at the start of a word" follows the trigger: off for Immediately.
    expect((within(group).getByRole("checkbox") as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText(/Expands immediately · Case: ignore, and follow mine/)).toBeTruthy();
    await user.type(abbreviation, ";opts");
    await typeContent("x");
    await user.click(saveButton());
    await waitFor(() => expect(fake.byAbbreviation(";r;opts")).toMatchObject({ trigger: "immediate", caseMode: "adaptive", wordStart: false }));
    await waitFor(() =>
      expect(screen.getAllByRole("status").some((s) => s.textContent === "Saved. Type ;r;opts to use it.")).toBe(true),
    );
  });

  it("keeps the On switch of a saved snippet immediate", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(fakeWith());
    await user.click(await screen.findByRole("option", { name: /^Late delivery/ }));
    const on = await screen.findByRole("switch", { name: "On" });
    expect(on.getAttribute("aria-checked")).toBe("true");
    await user.click(on);
    await waitFor(() => expect(fake.byAbbreviation(";rlate")?.enabled).toBe(false));
    // It was not an unsaved edit.
    expect(screen.queryByText("Not saved")).toBeNull();
    expect(on.getAttribute("aria-checked")).toBe("false");
  });

  it("names the template error under the content of a saved snippet that is broken", async () => {
    const fake = fakeWith();
    await fake.api.saveSnippet({
      folderId: fake.firstFolder.id, abbreviation: ";rbad", label: "Broken one", kind: "plain", body: "a {b",
      trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
    });
    const user = userEvent.setup();
    await renderLibrary(fake);
    await user.click(await screen.findByRole("option", { name: /^Broken one/ }));
    expect(
      await screen.findByText("Something that starts with { is never closed. Add the closing } or delete it."),
    ).toBeTruthy();
    expect(screen.getByText("E_UNCLOSED_MACRO")).toBeTruthy();
  });

  it("offers Duplicate, Copy content and Delete in More actions", async () => {
    const copied: string[] = [];
    const user = userEvent.setup();
    const fake = fakeWith();
    await renderLibrary(fake, { copyText: async (t) => void copied.push(t) });
    await user.click(await screen.findByRole("option", { name: /^Late delivery/ }));
    await user.click(await screen.findByRole("button", { name: "More actions" }));
    await user.click(screen.getByRole("menuitem", { name: /Copy content/ }));
    expect(copied).toEqual(["Sorry."]);
    await user.click(screen.getByRole("button", { name: "More actions" }));
    await user.click(screen.getByRole("menuitem", { name: /^Duplicate/ }));
    const abbreviation = (await screen.findByLabelText("Abbreviation")) as HTMLInputElement;
    expect(abbreviation.value).toBe(";rlate2");
    expect(contentText()).toBe("Sorry.");
    expect(screen.getByRole("heading", { name: "Late delivery" })).toBeTruthy();
  });
});

describe("changes made elsewhere", () => {
  it("shows a changed snippet's new text when nothing is being edited", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(fakeWith());
    const row = await screen.findByRole("option", { name: /^Late delivery/ });
    await user.click(row);
    const id = row.id.replace("row-", "");
    await screen.findByLabelText("Content");
    const s = fake.live.get(id)!;
    await fake.api.saveSnippet({ ...s, body: "Changed on another device" });
    await waitFor(() => expect(contentText()).toBe("Changed on another device"));
  });

  it("asks what to do when the open snippet changed while it had unsaved edits", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(fakeWith());
    const row = await screen.findByRole("option", { name: /^Late delivery/ });
    await user.click(row);
    const id = row.id.replace("row-", "");
    await waitFor(() => expect(contentText()).toBe("Sorry."));
    await typeContent(" mine");
    const s = fake.live.get(id)!;
    await fake.api.saveSnippet({ ...s, body: "Theirs" });
    const banner = await screen.findByText("This snippet was changed on another device.");
    expect(banner).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Keep editing mine" }));
    expect(contentText()).toBe("Sorry. mine");
    expect(screen.queryByText("This snippet was changed on another device.")).toBeNull();
    // Saving keeps the user's text.
    await user.click(saveButton());
    await waitFor(() => expect(fake.live.get(id)?.body).toBe("Sorry. mine"));
  });

  it("says so when the open snippet was deleted elsewhere", async () => {
    const user = userEvent.setup();
    const { fake } = await renderLibrary(fakeWith());
    const row = await screen.findByRole("option", { name: /^Late delivery/ });
    await user.click(row);
    await screen.findByLabelText("Content");
    await fake.api.deleteSnippet(row.id.replace("row-", ""));
    expect(
      await screen.findByText("This snippet no longer exists. It may have been deleted on another device."),
    ).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(await screen.findByText(/Select a snippet, or press/)).toBeTruthy();
  });

  it("keeps the draft when the service stops, and saves after it is back", async () => {
    const user = userEvent.setup();
    const fake = fakeWith();
    const { env, rerender } = await renderLibrary(fake);
    void env;
    void rerender;
    await screen.findAllByRole("option");
    await user.keyboard("{Control>}n{/Control}");
    await user.type(await screen.findByLabelText("Abbreviation"), "draft");
    await typeContent("kept");
    fake.setDown("disconnected");
    await user.click(saveButton());
    expect(await screen.findByText("Iamb is not running. Your changes stay here until it is back.")).toBeTruthy();
    expect(contentText()).toBe("kept");
    fake.setDown(null);
    await user.click(saveButton());
    await waitFor(() => expect(fake.byAbbreviation(";rdraft")?.body).toBe("kept"));
  });
});
