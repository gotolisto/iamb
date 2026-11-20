// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FieldView } from "../ipc";
import type { FormApi, FormEvent, FormOpen, ListenForm } from "./api";
import { FormView } from "./FormView";
import type { Os } from "./model";

afterEach(cleanup);

// Invented test data; nothing here is a user's text.
const field = (name: string, kind: FieldView["kind"], options: string[] = [], d = ""): FieldView => ({
  name,
  kind,
  options,
  default: d,
});

const EVERY_KIND: FieldView[] = [
  field("Customer", "text", [], "you"),
  field("Bike", "choice", ["Road", "Gravel", "City", "Cargo"], "Gravel"),
  field("Rim", "choice", ["26", "27.5", "29", "650b", "700c", "700c, 32 holes", "Other"], "700c"),
  field("Work needed", "area"),
  field("Parts in stock", "toggle", [], "no"),
];

function formOf(over: Partial<FormOpen> = {}): FormOpen {
  return {
    session: 7,
    snippet: "01010101-0101-0101-0101-010101010101",
    label: "Repair quote",
    abbreviation: ";rquote",
    app: null,
    fields: EVERY_KIND,
    lastValues: null,
    ...over,
  };
}

/** A fake Rust side: the commands, recorded, and the panel's event. */
function harness(form: FormOpen | null, os: Os = "win") {
  let current = form;
  let handler: ((e: FormEvent) => void) | undefined;
  const api = {
    ready: vi.fn(() => Promise.resolve()),
    current: vi.fn(() => Promise.resolve(current)),
    laidOut: vi.fn(() => Promise.resolve()),
    painted: vi.fn(() => Promise.resolve()),
    submit: vi.fn((_s: number, _v: Record<string, string>) => Promise.resolve()),
    cancel: vi.fn(() => Promise.resolve()),
  } satisfies FormApi;
  const listen: ListenForm = (h) => {
    handler = h;
    return Promise.resolve(() => {
      handler = undefined;
    });
  };
  const view = render(
    <FormView api={api} listen={listen} os={os} onNextPaint={(report) => report(4242)} />,
  );
  return {
    api,
    view,
    setCurrent: (f: FormOpen | null) => {
      current = f;
    },
    send: async (e: FormEvent) => {
      await act(async () => {
        handler?.(e);
        await Promise.resolve();
      });
    },
  };
}

/** Renders `form` as opened and shown by the Rust side. */
async function opened(form: FormOpen, os: Os = "win") {
  const h = harness(form, os);
  await screen.findByRole("dialog");
  await h.send({ type: "shown", session: form.session });
  return h;
}

describe("the fill-in form panel", () => {
  it("draws every kind of field from the request, labelled (design system §4.13)", async () => {
    await opened(formOf());
    const dialog = screen.getByRole("dialog", { name: "Repair quote" });
    const description = document.getElementById(dialog.getAttribute("aria-describedby") ?? "");
    expect(description?.textContent).toBe("Fill-in form, 5 fields");
    expect(within(dialog).getByText(";rquote")).toBeTruthy();
    expect(screen.getByRole("textbox", { name: "Customer" })).toHaveProperty("value", "you");
    const bike = screen.getByRole("radiogroup", { name: "Bike" });
    expect(within(bike).getAllByRole("radio")).toHaveLength(4);
    expect(within(bike).getByRole("radio", { name: "Gravel" })).toHaveProperty("checked", true);
    const rim = screen.getByRole("combobox", { name: "Rim" });
    expect(rim).toHaveProperty("value", "700c");
    expect(within(rim).getAllByRole("option")).toHaveLength(7);
    const area = screen.getByRole("textbox", { name: "Work needed" });
    expect(area.tagName).toBe("TEXTAREA");
    expect(screen.getByRole("checkbox", { name: "Parts in stock" })).toHaveProperty(
      "checked",
      false,
    );
    // Nothing is remembered by the web view.
    for (const box of screen.getAllByRole("textbox")) {
      expect(box.getAttribute("autocomplete")).toBe("off");
      expect(box.getAttribute("spellcheck")).toBe("false");
    }
  });

  it("starts in the first field with its content selected, and reports its height and paint", async () => {
    const h = await opened(formOf());
    const first = screen.getByRole<HTMLInputElement>("textbox", { name: "Customer" });
    expect(document.activeElement).toBe(first);
    expect([first.selectionStart, first.selectionEnd]).toEqual([0, 3]);
    expect(h.api.laidOut).toHaveBeenCalledWith(7, expect.any(Number));
    expect(h.api.painted).toHaveBeenCalledWith(7, 4242);
  });

  it("says once that its page listens, so that the hidden panel can paint once", async () => {
    const h = harness(null);
    await waitFor(() => expect(h.api.ready).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("handles a shown event that comes before the form is rendered", async () => {
    const h = harness(null);
    h.setCurrent(formOf({ session: 9 }));
    await h.send({ type: "shown", session: 9 });
    await h.send({ type: "open", session: 9 });
    await screen.findByRole("dialog");
    await waitFor(() => expect(h.api.painted).toHaveBeenCalledWith(9, 4242));
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Customer" }));
  });

  it("inserts on Enter from a one-line field, with every value made to fit", async () => {
    const h = await opened(formOf());
    const user = userEvent.setup();
    await user.keyboard("Jun");
    await user.click(screen.getByRole("radio", { name: "City" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Rim" }), "29");
    await user.click(screen.getByRole("checkbox", { name: "Parts in stock" }));
    await user.click(screen.getByRole("textbox", { name: "Customer" }));
    await user.keyboard("{Enter}");
    expect(h.api.submit).toHaveBeenCalledTimes(1);
    expect(h.api.submit).toHaveBeenCalledWith(7, {
      // The default was selected, so typing replaced it.
      Customer: "Jun",
      Bike: "City",
      Rim: "29",
      "Work needed": "",
      "Parts in stock": "yes",
    });
    // The answers are gone once the Rust side took them.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("makes a new line on Enter in a several-line field, and inserts on Ctrl+Enter there", async () => {
    const h = await opened(formOf());
    const user = userEvent.setup();
    await user.click(screen.getByRole("textbox", { name: "Work needed" }));
    await user.keyboard("True the wheel{Enter}Two spokes");
    expect(h.api.submit).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "Work needed" })).toHaveProperty(
      "value",
      "True the wheel\nTwo spokes",
    );
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(h.api.submit).toHaveBeenCalledWith(
      7,
      expect.objectContaining({ "Work needed": "True the wheel\nTwo spokes" }),
    );
  });

  it("uses Command on macOS: ⌘⏎ inserts from a several-line field", async () => {
    const h = await opened(formOf(), "mac");
    const user = userEvent.setup();
    await user.click(screen.getByRole("textbox", { name: "Work needed" }));
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(h.api.submit).not.toHaveBeenCalled();
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    expect(h.api.submit).toHaveBeenCalledTimes(1);
  });

  it("flips a toggle with Space and inserts on Enter from it", async () => {
    const h = await opened(formOf());
    const user = userEvent.setup();
    const box = screen.getByRole("checkbox", { name: "Parts in stock" });
    box.focus();
    await user.keyboard(" ");
    expect(box).toHaveProperty("checked", true);
    await user.keyboard("{Enter}");
    expect(h.api.submit).toHaveBeenCalledWith(7, expect.objectContaining({ "Parts in stock": "yes" }));
  });

  it("jumps to an option by its first letters, and inserts on Enter from a list", async () => {
    const h = await opened(formOf());
    const user = userEvent.setup();
    screen.getByRole("radio", { name: "Gravel" }).focus();
    await user.keyboard("c");
    expect(screen.getByRole("radio", { name: "City" })).toHaveProperty("checked", true);
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "City" }));
    await user.keyboard("c");
    expect(screen.getByRole("radio", { name: "Cargo" })).toHaveProperty("checked", true);
    await user.keyboard("{Enter}");
    expect(h.api.submit).toHaveBeenCalledWith(7, expect.objectContaining({ Bike: "Cargo" }));
  });

  it("cancels on Esc, and forgets the answers", async () => {
    const h = await opened(formOf());
    const user = userEvent.setup();
    await user.keyboard("secret");
    await user.keyboard("{Escape}");
    expect(h.api.cancel).toHaveBeenCalledWith(7);
    expect(h.api.submit).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(h.view.container.innerHTML).not.toContain("secret");
  });

  it("cancels and inserts from its buttons, in each system's order", async () => {
    const win = await opened(formOf(), "win");
    const winButtons = screen.getAllByRole("button").map((b) => b.textContent);
    expect(winButtons).toEqual(["Insert Enter", "Cancel Esc"]);
    expect(screen.getByRole("button", { name: /Insert/ }).getAttribute("aria-keyshortcuts")).toBe(
      "Enter Control+Enter",
    );
    fireEvent.click(screen.getByRole("button", { name: /Cancel/ }));
    expect(win.api.cancel).toHaveBeenCalledWith(7);
    cleanup();
    const mac = await opened(formOf(), "mac");
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual([
      "Cancel esc",
      "Insert ⏎",
    ]);
    fireEvent.click(screen.getByRole("button", { name: /Insert/ }));
    expect(mac.api.submit).toHaveBeenCalledTimes(1);
  });

  it("opens with the last answers, says so, and starts empty on Ctrl+Backspace", async () => {
    const last = { Customer: "Jun", Bike: "City", "Parts in stock": "yes" };
    await opened(formOf({ lastValues: last, app: "Mail" }));
    expect(screen.getByRole("textbox", { name: "Customer" })).toHaveProperty("value", "Jun");
    expect(screen.getByRole("radio", { name: "City" })).toHaveProperty("checked", true);
    expect(screen.getByText("Your last answers.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Start empty" }).getAttribute("aria-keyshortcuts")).toBe(
      "Control+Backspace",
    );
    expect(screen.queryByText("Goes into Mail")).toBeNull();
    const user = userEvent.setup();
    await user.keyboard("{Control>}{Backspace}{/Control}");
    expect(screen.getByRole("textbox", { name: "Customer" })).toHaveProperty("value", "you");
    expect(screen.getByRole("radio", { name: "Gravel" })).toHaveProperty("checked", true);
    expect(screen.getByRole("checkbox", { name: "Parts in stock" })).toHaveProperty("checked", false);
    expect(screen.queryByText("Your last answers.")).toBeNull();
    expect(screen.getByText("Goes into Mail")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Customer" }));
  });

  it("starts empty from the footer's link", async () => {
    await opened(formOf({ lastValues: { Customer: "Jun" } }), "mac");
    fireEvent.click(screen.getByRole("button", { name: "Start empty" }));
    expect(screen.getByRole("textbox", { name: "Customer" })).toHaveProperty("value", "you");
  });

  it("leaves Ctrl+Backspace to the field when nothing was remembered", async () => {
    await opened(formOf());
    const user = userEvent.setup();
    await user.keyboard("{Control>}{Backspace}{/Control}");
    // Not reset to anything: no remembered answers to leave.
    expect(screen.queryByRole("button", { name: "Start empty" })).toBeNull();
  });

  it("keeps every answer and says so when the focus could not be given back", async () => {
    const h = await opened(formOf({ app: "Mail" }));
    h.api.submit.mockImplementationOnce(() =>
      Promise.reject({ code: "focus_lost", message: "gone" }),
    );
    const user = userEvent.setup();
    await user.keyboard("Jun{Enter}");
    const banner = await screen.findByRole("alert");
    expect(banner.textContent).toBe(
      "Iamb could not get back to Mail. Nothing was typed. Click where the text should go and press Insert again, or cancel.",
    );
    expect(screen.getByText("Your answers are still here.")).toBeTruthy();
    expect(screen.getByRole("textbox", { name: "Customer" })).toHaveProperty("value", "Jun");
    // Insert again.
    fireEvent.click(screen.getByRole("button", { name: /Insert/ }));
    expect(h.api.submit).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("names no app it cannot name", async () => {
    const h = await opened(formOf());
    h.api.submit.mockImplementationOnce(() =>
      Promise.reject({ code: "focus_lost", message: "gone" }),
    );
    fireEvent.click(screen.getByRole("button", { name: /Insert/ }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Iamb could not get back to the app.",
    );
  });

  it("forgets the answers when the Rust side closes the form", async () => {
    const h = await opened(formOf());
    const user = userEvent.setup();
    await user.keyboard("secret");
    await h.send({ type: "closed", session: 7 });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(h.view.container.innerHTML).not.toContain("secret");
  });

  it("ignores the events of another session", async () => {
    const h = await opened(formOf());
    await h.send({ type: "closed", session: 8 });
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("replaces the fields of one form with the next one's", async () => {
    const h = await opened(formOf());
    h.setCurrent(formOf({ session: 8, label: "Greeting", fields: [field("Name", "text")] }));
    await h.send({ type: "open", session: 8 });
    await screen.findByRole("dialog", { name: "Greeting" });
    expect(screen.queryByRole("textbox", { name: "Customer" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveProperty("value", "");
  });

  it("uses the abbreviation as its title when the snippet has no label", async () => {
    await opened(formOf({ label: "" }));
    expect(screen.getByRole("dialog", { name: ";rquote" })).toBeTruthy();
  });
});
