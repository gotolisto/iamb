// @vitest-environment jsdom
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { createFake } from "../testkit";
import { diagnostic, renderWindow, rig } from "./testkit";

describe("Settings and “Why didn’t it expand?” in the library window", () => {
  it("replace the placeholders", async () => {
    const user = userEvent.setup();
    await renderWindow(rig());
    await user.click(screen.getByRole("button", { name: /Why didn’t it expand\?/ }));
    expect(screen.getByRole("heading", { level: 1, name: "Why didn’t it expand?" })).toBeTruthy();
    expect(await screen.findByRole("textbox", { name: "Test an abbreviation" })).toBeTruthy();
    expect(document.title).toBe("Iamb – Why didn’t it expand?");
    expect(screen.queryByText("This view is not available yet.")).toBeNull();

    await user.click(screen.getByRole("button", { name: /^Settings/ }));
    expect(screen.getByRole("heading", { level: 1, name: "Settings" })).toBeTruthy();
    expect(await screen.findByRole("heading", { level: 2, name: "General" })).toBeTruthy();
    expect(document.title).toBe("Iamb – Settings");
    expect(screen.queryByText("This view is not available yet.")).toBeNull();
  });

  it("open with Ctrl+, from anywhere in the window", async () => {
    await renderWindow(rig());
    fireEvent.keyDown(document, { key: ",", ctrlKey: true });
    expect(await screen.findByRole("heading", { level: 2, name: "General" })).toBeTruthy();
  });

  it("show the placeholder only when the app gives them no commands", async () => {
    const user = userEvent.setup();
    await renderWindow(rig({}, { settings: undefined }));
    await user.click(screen.getByRole("button", { name: /^Settings/ }));
    expect(screen.getByText("This view is not available yet.")).toBeTruthy();
  });

  it("take Why? from the log to the Typing settings, and Manage apps to the Apps settings", async () => {
    const user = userEvent.setup();
    await renderWindow(rig({}, {}, { diagnostics: [diagnostic({ code: "suspended_excluded_app", app: "mybank.exe" })] }));
    await user.click(screen.getByRole("button", { name: /Why didn’t it expand\?/ }));
    await user.click(await screen.findByRole("button", { name: "Manage apps" }));
    expect(await screen.findByRole("heading", { level: 2, name: "Apps" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /Why didn’t it expand\?/ }));
    await user.click(await screen.findByRole("button", { name: "Why?" }));
    expect(await screen.findByRole("heading", { level: 2, name: "Typing" })).toBeTruthy();
  });

  it("open the snippet of an entry in the library", async () => {
    const user = userEvent.setup();
    const library = createFake({ snippets: [{ abbreviation: ";out", label: "Out of office", body: "Back soon {" }] });
    const id = library.byAbbreviation(";out")?.id ?? "";
    const r = rig();
    r.env.api = library.api;
    r.env.onEvent = (h) => {
      library.listeners.add(h);
      return () => library.listeners.delete(h);
    };
    r.settings.diagnostics.push(diagnostic({ code: "E_UNCLOSED_MACRO", snippet: id }));
    await renderWindow(r);
    await user.click(screen.getByRole("button", { name: /Why didn’t it expand\?/ }));
    await user.click(await screen.findByRole("button", { name: "Open snippet" }));
    // Back in the library, the snippet is in the editor.
    await waitFor(() => expect(screen.getByRole("listbox", { name: "All snippets" })).toBeTruthy());
    expect(await screen.findByDisplayValue(";out")).toBeTruthy();
    expect(within(screen.getByRole("listbox", { name: "All snippets" })).getAllByRole("option")).toHaveLength(1);
  });

  it("keep the setting of the appearance when the window changes view", async () => {
    const user = userEvent.setup();
    const r = rig({ "ui.theme": "dark" });
    await renderWindow(r);
    await user.click(screen.getByRole("button", { name: /^Settings/ }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
    await user.click(screen.getByRole("button", { name: /All snippets/ }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    delete document.documentElement.dataset.theme;
  });
});
