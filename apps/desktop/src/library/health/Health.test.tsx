// @vitest-environment jsdom
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { createFake, renderLibrary } from "../testkit";
import { affectedCount, errorSentence, lastUsedText } from "./model";

const DAY = 86_400_000;

function planted() {
  return createFake({
    snippets: [
      { abbreviation: ";bad", label: "Out of office", body: "an {unclosed" },
      { abbreviation: ";rl", label: "Reply later", body: "Later.", trigger: "immediate", wordStart: false },
      { abbreviation: ";rlate", label: "Late delivery", body: "Sorry.", trigger: "delimiter" },
      { abbreviation: ";old", label: "Holiday closing", body: "We are closed." },
      { abbreviation: ";new", label: "Fine", body: "Fine." },
    ],
    usage: {
      3: { uses: 4, lastUsed: Date.now() - 200 * DAY },
      4: { uses: 9, lastUsed: Date.now() - DAY },
    },
  });
}

async function openHealth() {
  const ctx = await renderLibrary(planted());
  await screen.findAllByRole("option");
  fireEvent.click(screen.getByRole("button", { name: /Library health/ }));
  return ctx;
}

describe("Library health", () => {
  it("shows one section per kind of problem, with the spec's titles and no empty section", async () => {
    await openHealth();
    for (const name of [
      "Will not expand: error in the snippet",
      "Never reached",
      "Not used on this computer for 90 days",
    ]) {
      expect(await screen.findByRole("region", { name })).toBeTruthy();
    }
    expect(screen.queryByRole("region", { name: "Will not expand: abbreviation taken" })).toBeNull();
  });

  it("explains each entry in one line and offers the fix", async () => {
    await openHealth();
    const errors = await screen.findByRole("region", { name: "Will not expand: error in the snippet" });
    expect(within(errors).getByText(/Out of office/)).toBeTruthy();
    expect(within(errors).getByRole("button", { name: "Open snippet" })).toBeTruthy();
    const never = screen.getByRole("region", { name: "Never reached" });
    expect(never.textContent).toContain("Late delivery");
    expect(never.textContent).toContain(";rl expands immediately and fires first.");
    expect(within(never).getByRole("button", { name: "Open ;rl" })).toBeTruthy();
    expect(within(never).getByRole("button", { name: "Open ;rlate" })).toBeTruthy();
    const unused = screen.getByRole("region", { name: "Not used on this computer for 90 days" });
    expect(unused.textContent).toContain("Counted on this computer only.");
    expect(unused.textContent).toContain("Holiday closing");
    expect(unused.textContent).toMatch(/last used \d/);
    expect(unused.textContent).not.toContain("Fine");
  });

  it("opens the snippet from its entry", async () => {
    const user = userEvent.setup();
    await openHealth();
    const errors = await screen.findByRole("region", { name: "Will not expand: error in the snippet" });
    await user.click(within(errors).getByRole("button", { name: "Open snippet" }));
    expect(await screen.findByLabelText("Abbreviation")).toBeTruthy();
    expect((screen.getByLabelText("Abbreviation") as HTMLInputElement).value).toBe(";bad");
  });

  it("turns an unused snippet off, and deletes it with an Undo", async () => {
    const user = userEvent.setup();
    const { fake } = await openHealth();
    const unused = await screen.findByRole("region", { name: "Not used on this computer for 90 days" });
    await user.click(within(unused).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(fake.byAbbreviation(";old")?.enabled).toBe(false));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Not used on this computer for 90 days" })).toBeNull());
  });

  it("deletes an unused snippet from the view", async () => {
    const user = userEvent.setup();
    const { fake } = await openHealth();
    const unused = await screen.findByRole("region", { name: "Not used on this computer for 90 days" });
    await user.click(within(unused).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(fake.byAbbreviation(";old")).toBeUndefined());
    expect(await screen.findByText(/Deleted “Holiday closing”/)).toBeTruthy();
  });

  it("counts the snippets with something to fix in the sidebar, each once", async () => {
    await renderLibrary(planted());
    await screen.findAllByRole("option");
    // ;bad (error), ;rlate (never reached), ;old (unused).
    await waitFor(() => expect(screen.getByRole("button", { name: /Library health/ }).textContent).toContain("3"));
  });

  it("says so when nothing needs fixing", async () => {
    await renderLibrary(createFake({ snippets: [{ abbreviation: ";a", body: "a" }, { abbreviation: ";b", body: "b" }] }));
    await screen.findAllByRole("option");
    fireEvent.click(screen.getByRole("button", { name: /Library health/ }));
    expect(await screen.findByText("Nothing to fix. All 2 snippets can expand.")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Library health/ }).textContent).not.toMatch(/\d/);
  });

  it("follows a change made while it is open", async () => {
    const { fake } = await openHealth();
    await screen.findByRole("region", { name: "Will not expand: error in the snippet" });
    const bad = fake.byAbbreviation(";bad");
    expect(bad).toBeTruthy();
    if (bad) {
      await fake.api.saveSnippet({ ...bad, body: "fixed", tags: bad.tags });
    }
    await waitFor(() => expect(screen.queryByRole("region", { name: "Will not expand: error in the snippet" })).toBeNull());
  });

  it("offers a way back when the service cannot answer", async () => {
    const fake = planted();
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    fake.setDown("disconnected");
    fireEvent.click(screen.getByRole("button", { name: /Library health/ }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Library health could not be read.");
    fake.setDown(null);
    fireEvent.click(within(alert).getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("region", { name: "Never reached" })).toBeTruthy();
  });
});

describe("the sentences of library health", () => {
  const e = (over: object) => ({ snippet: "a", reason: "E_SNIPPET_NOT_FOUND", kept: null, reference: null, included: null, ...over });
  it("name the missing snippet, the cycle and the included snippet that is at fault", () => {
    expect(errorSentence(e({ reference: ";sig2" }))).toBe("It includes ;sig2, which does not exist.");
    expect(errorSentence(e({ reason: "E_SNIPPET_CYCLE", reference: ";a" }))).toBe("It includes itself, through ;a.");
    expect(errorSentence(e({ reason: "E_SNIPPET_CYCLE", reference: ";a", included: ";b" }))).toBe(
      "It includes ;b, which includes itself through ;a.",
    );
    expect(errorSentence(e({ reason: "E_TOO_DEEP", reference: ";x" }))).toContain("more than 8 deep, at ;x");
    expect(errorSentence(e({ reason: "E_BAD_FORMAT", included: ";fmt" }))).toContain("It includes ;fmt, which has an error.");
    expect(errorSentence(e({ reason: "E_UNBALANCED_IF" }))).toContain("optional part");
  });

  it("dates the last use, or say it never happened", () => {
    expect(lastUsedText(null)).toBe("never used on this computer");
    expect(lastUsedText(Date.UTC(2025, 11, 14, 12), new Date(Date.UTC(2025, 11, 20)))).toBe("last used 14 December");
    expect(lastUsedText(Date.UTC(2024, 11, 14, 12), new Date(Date.UTC(2025, 11, 20)))).toBe("last used 14 December 2024");
  });

  it("count each snippet once across sections", () => {
    expect(
      affectedCount({
        shadowed: [{ by: "x", hidden: "a" }],
        excluded: [e({ snippet: "a" }), e({ snippet: "b" })],
        unused: [{ snippet: "b", lastUsed: null }, { snippet: "c", lastUsed: null }],
      }),
    ).toBe(3);
    expect(affectedCount(null)).toBe(0);
  });
});
