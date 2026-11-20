// @vitest-environment jsdom
// The editor's kinds (task 3.8; specification §4.3, "Kinds"): "Plain text" or "Formatted", the
// formatting toolbar of a formatted snippet, and the question before formatting would be lost.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { TO_PLAIN_BODY, TO_PLAIN_CONFIRM, TO_PLAIN_TITLE } from "./Editor";
import { createFake, renderLibrary, setContent } from "./testkit";

async function openNew() {
  const user = userEvent.setup();
  const ctx = await renderLibrary(createFake({ snippets: [{ abbreviation: ";a", label: "A", body: "a" }] }));
  await screen.findAllByRole("option");
  await user.keyboard("{Control>}n{/Control}");
  const abbreviation = await screen.findByLabelText("Abbreviation");
  await user.type(abbreviation, ";sigf");
  return { user, ...ctx };
}

const kind = () => screen.getByRole("group", { name: "Kind" });

describe("the kind of a snippet", () => {
  it("starts as plain text, and Formatted brings the formatting toolbar", async () => {
    const { user } = await openNew();
    const plain = within(kind()).getByRole("button", { name: "Plain text" });
    const formatted = within(kind()).getByRole("button", { name: "Formatted" });
    expect(plain.getAttribute("aria-pressed")).toBe("true");
    expect(screen.queryByRole("toolbar", { name: "Formatting" })).toBeNull();
    await user.click(formatted);
    expect(formatted.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("toolbar", { name: "Formatting" })).toBeTruthy();
  });

  it("saves the kind", async () => {
    const { user, fake } = await openNew();
    await user.click(within(kind()).getByRole("button", { name: "Formatted" }));
    await setContent("**Kim Lee**");
    await user.click(screen.getByRole("button", { name: /^Save/ }));
    await waitFor(() => expect(fake.byAbbreviation(";sigf")?.kind).toBe("rich"));
    expect(fake.byAbbreviation(";sigf")?.body).toBe("**Kim Lee**");
  });

  it("asks before formatting would be lost, and keeps it on Cancel", async () => {
    const { user } = await openNew();
    await user.click(within(kind()).getByRole("button", { name: "Formatted" }));
    await setContent("**Kim Lee**");
    await user.click(within(kind()).getByRole("button", { name: "Plain text" }));
    const dialog = await screen.findByRole("alertdialog", { name: TO_PLAIN_TITLE });
    expect(dialog.textContent).toContain(TO_PLAIN_BODY);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(within(kind()).getByRole("button", { name: "Formatted" }).getAttribute("aria-pressed")).toBe("true");

    await user.click(within(kind()).getByRole("button", { name: "Plain text" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: TO_PLAIN_CONFIRM }));
    expect(within(kind()).getByRole("button", { name: "Plain text" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.queryByRole("toolbar", { name: "Formatting" })).toBeNull();
  });

  it("switches back without asking when nothing would be lost", async () => {
    const { user } = await openNew();
    await user.click(within(kind()).getByRole("button", { name: "Formatted" }));
    await setContent("Kim Lee, 2 * 3");
    await user.click(within(kind()).getByRole("button", { name: "Plain text" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(within(kind()).getByRole("button", { name: "Plain text" }).getAttribute("aria-pressed")).toBe("true");
  });
});
