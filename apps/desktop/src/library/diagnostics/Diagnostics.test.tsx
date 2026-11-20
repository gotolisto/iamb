// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Diagnostic } from "../settings/api";
import type { SectionId } from "../settings/section";
import { type Rig, diagnostic, emitEvent, rig } from "../settings/testkit";
import { createFake } from "../testkit";
import { Diagnostics, MAX_ENTRIES, mergeEntries } from "./Diagnostics";

const T = (h: number, m: number) => Date.UTC(2026, 10, 20, h, m);

function show(r: Rig, handlers: { onOpenSnippet?: (id: string) => void; onOpenSettings?: (s: SectionId) => void } = {}) {
  const onOpenSnippet = handlers.onOpenSnippet ?? (() => {});
  const onOpenSettings = handlers.onOpenSettings ?? (() => {});
  return render(<Diagnostics env={r.env} onOpenSnippet={onOpenSnippet} onOpenSettings={onOpenSettings} />);
}

const entries = () => screen.getByRole("list", { name: "Entries, newest first" });
const items = () => within(entries()).getAllByRole("listitem");

describe("the log", () => {
  it("says nothing was held back, and that nothing is recorded but codes", async () => {
    show(rig());
    expect(await screen.findByText("Nothing has been held back since Iamb started.")).toBeTruthy();
    expect(screen.getByText("Kept in memory only. Iamb never records what you typed.")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Entries, newest first" })).toBeNull();
  });

  it("lists the entries newest first, each with its time and sentence", async () => {
    const r = rig(
      {},
      {},
      {
        diagnostics: [
          diagnostic({ code: "suspended_secure_field", at: T(9, 5) }),
          diagnostic({ code: "suspended_composing", at: T(9, 20) }),
          diagnostic({ code: "no_last_form", at: T(9, 10) }),
        ],
      },
    );
    show(r);
    await screen.findByRole("list", { name: "Entries, newest first" });
    const titles = items().map((li) => within(li).getByText(/.+/, { selector: "b" }).textContent);
    expect(titles).toEqual([
      "Not expanding while your keyboard composes text",
      "There is no earlier form to fill in again",
      "Not expanding in a password field",
    ]);
    const first = items()[0] as HTMLElement;
    expect(within(first).getByText("An input method is on.")).toBeTruthy();
    expect(first.querySelector("time")?.getAttribute("datetime")).toBe(new Date(T(9, 20)).toISOString());
    expect(first.querySelector("time")?.textContent).toMatch(/\d/);
  });

  it("names the app where the service named one", async () => {
    const r = rig({}, {}, { diagnostics: [diagnostic({ code: "suspended_excluded_app", app: "mybank.exe" })] });
    show(r);
    const li = (await screen.findAllByRole("listitem"))[0] as HTMLElement;
    expect(within(li).getByText("Not expanding in mybank.exe")).toBeTruthy();
    // Said once: in the sentence.
    expect(li.textContent?.match(/mybank\.exe/g)).toHaveLength(1);
    const r2 = rig({}, {}, { diagnostics: [diagnostic({ code: "form_open", app: "notes" })] });
    document.body.innerHTML = "";
    show(r2);
    expect((await screen.findAllByRole("listitem"))[0]?.textContent).toContain("notes");
  });

  it("names the snippet by its label, or by its abbreviation when it has none", async () => {
    const library = createFake({
      snippets: [
        { abbreviation: ";rty", label: "Thanks for your order", body: "Thank you." },
        { abbreviation: ";sig", label: "", body: "Best wishes" },
      ],
    });
    const rty = library.byAbbreviation(";rty")?.id ?? "";
    const sig = library.byAbbreviation(";sig")?.id ?? "";
    const r = rig();
    r.env.api = library.api;
    r.settings.diagnostics.push(
      diagnostic({ code: "not_a_delimiter", snippet: rty, at: T(10, 2) }),
      diagnostic({ code: "injection_cancelled", snippet: sig, at: T(10, 1) }),
      diagnostic({ code: "form_open", snippet: "00000000-0000-7000-8000-00000000dead", at: T(10, 0) }),
    );
    show(r);
    expect(await screen.findByText("“Thanks for your order” did not expand: Enter and Tab do not count as the end of a word")).toBeTruthy();
    expect(await screen.findByText("“Best wishes” was stopped halfway")).toBeTruthy();
    // The snippet is gone: no name is made up.
    expect(await screen.findByText("A snippet was not started: another fill-in form was still open")).toBeTruthy();
  });

  it("adds the diagnostics that arrive while it is open, once each", async () => {
    const r = rig({}, {}, { diagnostics: [diagnostic({ code: "no_last_form", at: T(8, 0) })] });
    show(r);
    await screen.findByRole("list", { name: "Entries, newest first" });
    const added = { type: "diagnostic_added", code: "suspended_composing", snippet: null, app: null, at: T(8, 5) } as const;
    act(() => emitEvent(r.library, added));
    act(() => emitEvent(r.library, added));
    expect(items()).toHaveLength(2);
    expect(within(items()[0] as HTMLElement).getByText("Not expanding while your keyboard composes text")).toBeTruthy();
  });

  it("keeps an entry heard while the list was on its way", async () => {
    const r = rig({}, {}, { diagnostics: [diagnostic({ code: "no_last_form", at: T(8, 0) })] });
    let release: () => void = () => {};
    const real = r.settings.api.recentDiagnostics;
    r.settings.api.recentDiagnostics = () => new Promise((resolve) => { release = () => resolve(real()); });
    show(r);
    await waitFor(() => expect(r.library.listeners.size).toBeGreaterThan(0));
    act(() => emitEvent(r.library, { type: "diagnostic_added", code: "form_open", snippet: null, app: null, at: T(8, 9) }));
    await act(async () => release());
    await waitFor(() => expect(items()).toHaveLength(2));
  });

  it("keeps at most 200 entries", async () => {
    const many: Diagnostic[] = Array.from({ length: 250 }, (_, i) => diagnostic({ code: "form_open", at: T(0, 0) + i * 1000 }));
    show(rig({}, {}, { diagnostics: many }));
    await screen.findByRole("list", { name: "Entries, newest first" });
    expect(items()).toHaveLength(MAX_ENTRIES);
    expect(MAX_ENTRIES).toBe(200);
    expect(mergeEntries(many, many)).toHaveLength(200);
    expect(mergeEntries([many[0] as Diagnostic], [many[0] as Diagnostic])).toHaveLength(1);
  });

  it("hides the technical entries until asked", async () => {
    const r = rig(
      {},
      {},
      {
        diagnostics: [
          diagnostic({ code: "keys_for_unreported_window", at: T(7, 2) }),
          diagnostic({ code: "a_code_of_a_newer_iamb", at: T(7, 1) }),
          diagnostic({ code: "no_last_form", at: T(7, 0) }),
        ],
      },
    );
    const user = userEvent.setup();
    show(r);
    await screen.findByRole("list", { name: "Entries, newest first" });
    expect(items()).toHaveLength(1);
    await user.click(screen.getByRole("checkbox", { name: "Show technical entries" }));
    expect(items()).toHaveLength(3);
    expect(screen.getByText("Technical: keys_for_unreported_window")).toBeTruthy();
    expect(screen.getByText("Technical: a_code_of_a_newer_iamb")).toBeTruthy();
    await user.click(screen.getByRole("checkbox", { name: "Show technical entries" }));
    expect(items()).toHaveLength(1);
  });

  it("says Nothing has been held back when only technical entries exist", async () => {
    show(rig({}, {}, { diagnostics: [diagnostic({ code: "panel_focus_expired" })] }));
    expect(await screen.findByText("Nothing has been held back since Iamb started.")).toBeTruthy();
  });

  it("adds the administrator hint on Windows only", async () => {
    const d = { diagnostics: [diagnostic({ code: "suspended_unknown_focus", app: "Terminal" })] };
    show(rig({}, { mac: false }, d));
    expect(await screen.findByText(/It may be running as administrator\./)).toBeTruthy();
    document.body.innerHTML = "";
    show(rig({}, { mac: true }, d));
    await screen.findByText(/Iamb cannot see into this window, so it stays out\./);
    expect(screen.queryByText(/administrator/)).toBeNull();
  });
});

describe("the buttons", () => {
  it("open the snippet of an error, where it is", async () => {
    const library = createFake({ snippets: [{ abbreviation: ";out", label: "Out of office", body: "{" }] });
    const id = library.byAbbreviation(";out")?.id ?? "";
    const r = rig();
    r.env.api = library.api;
    r.settings.diagnostics.push(diagnostic({ code: "E_UNCLOSED_MACRO", snippet: id }));
    const onOpenSnippet = vi.fn();
    const user = userEvent.setup();
    show(r, { onOpenSnippet });
    expect(await screen.findByText("“Out of office” has an error")).toBeTruthy();
    expect(screen.getByText("E_UNCLOSED_MACRO")).toBeTruthy();
    expect(screen.getByText(/Nothing was typed\./)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Open snippet" }));
    expect(onOpenSnippet).toHaveBeenCalledWith(id);
  });

  it("offer no Open snippet for an error that names no snippet", async () => {
    show(rig({}, {}, { diagnostics: [diagnostic({ code: "E_BAD_SYNTAX" })] }));
    await screen.findByText("A snippet has an error");
    expect(screen.queryByRole("button", { name: "Open snippet" })).toBeNull();
  });

  it("open the Apps settings for an excluded app and the Typing settings for Enter and Tab", async () => {
    const r = rig(
      {},
      {},
      {
        diagnostics: [
          diagnostic({ code: "suspended_excluded_app", app: "mybank.exe", at: T(6, 2) }),
          diagnostic({ code: "not_a_delimiter", at: T(6, 1) }),
        ],
      },
    );
    const onOpenSettings = vi.fn();
    const user = userEvent.setup();
    show(r, { onOpenSettings });
    await user.click(await screen.findByRole("button", { name: "Manage apps" }));
    expect(onOpenSettings).toHaveBeenLastCalledWith("apps");
    // The entry's own "Why?" and the foot's.
    const whys = screen.getAllByRole("button", { name: "Why?" });
    expect(whys).toHaveLength(2);
    await user.click(whys[0] as HTMLElement);
    expect(onOpenSettings).toHaveBeenLastCalledWith("typing");
  });

  it("end with the line about Enter and Tab and a way to the Typing settings", async () => {
    const onOpenSettings = vi.fn();
    const user = userEvent.setup();
    show(rig(), { onOpenSettings });
    expect(await screen.findByText(/Typed an abbreviation and pressed Enter or Tab\? Those keys do not expand a snippet\./)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Why?" }));
    expect(onOpenSettings).toHaveBeenCalledWith("typing");
  });
});

describe("Test an abbreviation", () => {
  const typed = async (r: Rig, text: string) => {
    const user = userEvent.setup();
    show(r);
    const field = await screen.findByRole("textbox", { name: "Test an abbreviation" });
    await user.type(field, text);
    return { user, field };
  };
  const base = { snippet: "id-1", label: "Thanks for your order", abbreviation: ";rty", error: null };

  it("says what would replace what", async () => {
    const r = rig();
    r.settings.answer(";rty ", { ...base, outcome: "expands" });
    await typed(r, ";rty ");
    const status = await screen.findByText(/^Expands\./);
    expect(status.parentElement?.textContent).toBe("Expands. “Thanks for your order” would replace ;rty and the space.");
  });

  it("says Not yet, and that Enter and Tab do not count", async () => {
    const r = rig();
    r.settings.answer(";rty", { ...base, outcome: "not_yet" });
    await typed(r, ";rty");
    const status = await screen.findByText(/^Not yet\./);
    expect(status.parentElement?.textContent).toBe(
      "Not yet. “Thanks for your order” waits for a space or punctuation after ;rty. Enter and Tab do not count.",
    );
  });

  it("does not say they do not count when they do", async () => {
    const r = rig({ delimiters: " .\n" });
    r.settings.answer(";rty", { ...base, outcome: "not_yet" });
    await typed(r, ";rty");
    const status = await screen.findByText(/^Not yet\./);
    expect(status.parentElement?.textContent).not.toContain("Enter and Tab do not count");
  });

  it("says No snippet", async () => {
    const r = rig();
    await typed(r, ";rtx");
    const status = await screen.findByText(/^No snippet\./);
    expect(status.parentElement?.textContent).toBe("No snippet. Nothing has the abbreviation ;rtx.");
  });

  it("says Broken, with the error, and opens the snippet", async () => {
    const r = rig();
    r.settings.answer(";out", { snippet: "id-9", label: "Out of office", abbreviation: ";out", error: "E_UNCLOSED_MACRO", outcome: "broken" });
    const onOpenSnippet = vi.fn();
    const user = userEvent.setup();
    show(r, { onOpenSnippet });
    await user.type(await screen.findByRole("textbox", { name: "Test an abbreviation" }), ";out");
    const status = await screen.findByText(/^Broken\./);
    expect(status.parentElement?.textContent).toContain("“Out of office” has an error: Something that starts with { is never closed.");
    await user.click(screen.getByRole("button", { name: "Open snippet" }));
    expect(onOpenSnippet).toHaveBeenCalledWith("id-9");
  });

  it("answers for what was typed last, not for an older question", async () => {
    const r = rig();
    const answers: Record<string, (v: Awaited<ReturnType<typeof r.settings.api.testAbbreviation>>) => void> = {};
    r.settings.api.testAbbreviation = (text) =>
      new Promise((resolve) => {
        answers[text] = resolve;
      });
    const user = userEvent.setup();
    show(r);
    const field = await screen.findByRole("textbox", { name: "Test an abbreviation" });
    await user.type(field, ";a");
    await waitFor(() => expect(answers[";a"]).toBeDefined());
    await user.type(field, "b");
    await waitFor(() => expect(answers[";ab"]).toBeDefined());
    await act(async () => answers[";ab"]?.({ ...base, outcome: "expands", label: "New" }));
    await act(async () => answers[";a"]?.({ ...base, outcome: "expands", label: "Old" }));
    expect(screen.getByText(/“New”/)).toBeTruthy();
    expect(screen.queryByText(/“Old”/)).toBeNull();
  });

  it("clears the answer when the field is cleared, and never keeps what was typed", async () => {
    const r = rig();
    const { user, field } = await typed(r, ";zz");
    await screen.findByText(/^No snippet\./);
    await user.clear(field);
    await waitFor(() => expect(screen.queryByText(/^No snippet\./)).toBeNull());
    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);
  });

  it("says when the service is busy", async () => {
    const r = rig();
    r.settings.api.testAbbreviation = async () => {
      throw Object.assign(new Error("x"), { code: "unavailable", message: "busy" });
    };
    await typed(r, ";zz");
    expect((await screen.findByText("Iamb is busy. Try again in a moment.")).className).toContain("error");
  });

  it("cannot be used while Iamb is not running", async () => {
    show(rig({}, { connection: "disconnected" }));
    expect(await screen.findByRole("textbox", { name: "Test an abbreviation" })).toHaveProperty("disabled", true);
    fireEvent.change(screen.getByRole("textbox", { name: "Test an abbreviation" }), { target: { value: "x" } });
  });
});

describe("without commands", () => {
  it("says it is not available yet", () => {
    show(rig({}, { settings: undefined }));
    expect(screen.getByText("This view is not available yet.")).toBeTruthy();
  });
});
