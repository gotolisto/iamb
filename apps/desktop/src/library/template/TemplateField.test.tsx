// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ParsedTemplate, TemplatePreview, TemplateService } from "./api";
import { select } from "./dom";
import { docFromTokens, type Doc } from "./model";
import { referenceTokens } from "./reference-tokenizer";
import { TemplateField } from "./TemplateField";

afterEach(cleanup);

/** A service that answers as the real one would for tokens (through the test oracle), with an
 * error when the test gives one, and an empty preview. */
function fakeService(errors: Record<string, ParsedTemplate["error"]> = {}) {
  const parsed: string[] = [];
  const service: TemplateService = {
    parse: vi.fn((source: string) => {
      parsed.push(source);
      return Promise.resolve({ tokens: referenceTokens(source), error: errors[source] ?? null });
    }),
    preview: vi.fn(
      (): Promise<TemplatePreview> => Promise.resolve({ parts: [], cursor: null, fields: [], error: null }),
    ),
  };
  return { service, parsed };
}

async function setup(value: string, errors: Record<string, ParsedTemplate["error"]> = {}) {
  const { service } = fakeService(errors);
  const changes: string[] = [];
  let current = value;
  const onChange = vi.fn((v: string) => {
    changes.push(v);
    current = v;
    view.rerender(<Field />);
  });
  const onEscape = vi.fn();
  function Field() {
    return (
      <>
        <label htmlFor="content">Content</label>
        <TemplateField id="content" value={current} onChange={onChange} onEscape={onEscape} service={service} />
      </>
    );
  }
  const view = render(<Field />);
  await flush();
  const box = screen.getByRole("textbox", { name: "Content" });
  return { box, changes, onChange, onEscape, service, value: () => current };
}

/** Lets the fake service's promises settle and React render. */
async function flush() {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
}

/** The document the editor shows for `source` once its tokens arrived. */
const docOf = (source: string): Doc => docFromTokens(source, referenceTokens(source));

/** Focuses the text box and puts the selection at display positions. */
function place(box: HTMLElement, source: string, anchor: number, focus = anchor) {
  act(() => {
    box.focus();
    select(box, docOf(source), anchor, focus);
  });
}

async function type(box: HTMLElement, text: string) {
  for (const ch of text) {
    act(() => {
      box.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: ch, bubbles: true, cancelable: true }));
    });
    await flush();
  }
}

async function key(box: HTMLElement, k: string, opts: Partial<KeyboardEventInit> = {}) {
  fireEvent.keyDown(box, { key: k, ...opts });
  await flush();
}

function chips(box: HTMLElement) {
  return Array.from(box.querySelectorAll<HTMLElement>(".tpl-chip"));
}

describe("chips", () => {
  it("are drawn from the service's tokens, with their labels and names", async () => {
    const source = 'Hi {field name="First name"}, {date shift=+3bd}{key name=tab times=2}{lbrace}x';
    const { box, onChange } = await setup(source);
    expect(chips(box).map((c) => c.textContent)).toEqual(["First name", "Date +3 working days", "Tab ×2"]);
    expect(chips(box).map((c) => c.getAttribute("aria-label"))).toEqual([
      "Fill-in field, First name, text",
      "Date, plus 3 working days",
      "Key press, Tab, 2 times",
    ]);
    expect(chips(box).map((c) => c.className)).toEqual([
      "tpl-chip ask",
      "tpl-chip insert",
      "tpl-chip act",
    ]);
    // {lbrace} is a plain brace, not a chip; loading changes nothing.
    expect(box.textContent).toContain("{x");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("are one character for the arrow keys, and selected with Shift", async () => {
    const source = "a{cursor}b";
    const { box } = await setup(source);
    place(box, source, 1);
    await key(box, "ArrowRight", { shiftKey: true });
    expect(chips(box)[0]?.classList.contains("is-selected")).toBe(true);
    await key(box, "ArrowRight");
    expect(chips(box)[0]?.classList.contains("is-selected")).toBe(false);
    // The caret is after the chip, before "b": typing goes there.
    await type(box, "!");
    expect(box.textContent).toBe("aCursor!b");
  });

  it("are deleted as one character by Backspace and Delete", async () => {
    const source = "a{clipboard}b{cursor}c";
    const { box, value } = await setup(source);
    place(box, source, 2);
    await key(box, "Backspace");
    expect(value()).toBe("ab{cursor}c");
    place(box, "ab{cursor}c", 2);
    await key(box, "Delete");
    expect(value()).toBe("abc");
  });

  it("open their card with Enter; the card changes the chip; Esc puts the caret after it", async () => {
    const source = "x{key name=tab}y";
    const { box, value } = await setup(source);
    place(box, source, 1, 2);
    await key(box, "Enter");
    const card = screen.getByRole("dialog", { name: "Key press" });
    fireEvent.change(within(card).getByLabelText("Times"), { target: { value: "3" } });
    await flush();
    expect(value()).toBe("x{key name=tab times=3}y");
    fireEvent.click(within(card).getByLabelText("Hold Shift"));
    await flush();
    expect(value()).toBe("x{key name=tab times=3 shift=yes}y");
    expect(chips(box)[0]?.textContent).toBe("Shift+Tab ×3");
    // Out of range: the card says so and the template keeps the last good value.
    fireEvent.change(within(card).getByLabelText("Times"), { target: { value: "60" } });
    await flush();
    expect(within(card).getByRole("alert").textContent).toBe("Press it 1 to 50 times.");
    expect(value()).toBe("x{key name=tab times=3 shift=yes}y");
    fireEvent.keyDown(within(card).getByLabelText("Key"), { key: "Escape" });
    await flush();
    expect(screen.queryByRole("dialog")).toBeNull();
    await type(box, "!");
    expect(value()).toBe("x{key name=tab times=3 shift=yes}!y");
  });

  it("open their card on a click, and Remove takes the chip out", async () => {
    const source = "a{wait ms=500}b";
    const { box, value } = await setup(source);
    fireEvent.mouseDown(chips(box)[0] as HTMLElement);
    await flush();
    const card = screen.getByRole("dialog", { name: "Wait" });
    fireEvent.change(within(card).getByLabelText("Seconds"), { target: { value: "1.5" } });
    await flush();
    expect(value()).toBe("a{wait ms=1500}b");
    expect(chips(box)[0]?.textContent).toBe("Wait 1.5 s");
    fireEvent.click(within(card).getByRole("button", { name: "Remove" }));
    await flush();
    expect(value()).toBe("ab");
  });

  it("an unchanged chip keeps its source exactly", async () => {
    const source = 'a{date   fmt="%Y"}b';
    const { box, onChange } = await setup(source);
    place(box, source, 1, 2);
    await key(box, "Enter");
    const card = screen.getByRole("dialog", { name: "Date" });
    fireEvent.click(within(card).getByRole("button", { name: /Done/ }));
    await flush();
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("chip cards", () => {
  async function open(source: string, name: string) {
    const ctx = await setup(source);
    fireEvent.mouseDown(chips(ctx.box)[0] as HTMLElement);
    await flush();
    return { ...ctx, card: screen.getByRole("dialog", { name }) };
  }

  it("fill-in field: name, kind, options, starting value, on and off", async () => {
    const { card, value } = await open("{field name=Size}", "Fill-in field");
    fireEvent.change(within(card).getByLabelText("Name"), { target: { value: "Bike size" } });
    await flush();
    expect(value()).toBe('{field name="Bike size"}');
    fireEvent.click(within(card).getByLabelText("Choice"));
    await flush();
    expect(value()).toBe('{field name="Bike size" type=choice options="Option 1|Option 2"}');
    fireEvent.change(within(card).getByLabelText("Options"), { target: { value: "S\nM\nL" } });
    await flush();
    fireEvent.change(within(card).getByLabelText("Starts with"), { target: { value: "M" } });
    await flush();
    expect(value()).toBe('{field name="Bike size" type=choice options="S|M|L" default=M}');
    fireEvent.click(within(card).getByLabelText("Toggle"));
    await flush();
    fireEvent.change(within(card).getByLabelText("Types when on"), { target: { value: "Yes, please" } });
    await flush();
    expect(value()).toBe('{field name="Bike size" type=toggle on="Yes, please"}');
  });

  it("date: format and shift, with today's result from the service", async () => {
    const { card, value, service } = await open("{date}", "Date");
    fireEvent.change(within(card).getByLabelText("Shift"), { target: { value: "3" } });
    await flush();
    fireEvent.change(within(card).getByLabelText("Unit"), { target: { value: "bd" } });
    await flush();
    expect(value()).toBe("{date shift=+3bd}");
    expect(within(card).getByText("Working days are Monday to Friday. Public holidays are not known.")).toBeTruthy();
    fireEvent.change(within(card).getByLabelText("Format"), { target: { value: "%d/%m/%Y" } });
    await flush();
    expect(value()).toBe("{date shift=+3bd fmt=%d/%m/%Y}");
    fireEvent.change(within(card).getByLabelText("Format"), { target: { value: "custom" } });
    await flush();
    fireEvent.change(within(card).getByLabelText("Format code"), { target: { value: "%A %-d" } });
    await flush();
    expect(value()).toBe('{date shift=+3bd fmt="%A %-d"}');
    // "Today this gives" and the format list come from RenderPreview, never computed here.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 200));
    });
    expect(vi.mocked(service.preview).mock.calls.some(([s]) => s === '{date shift=+3bd fmt="%A %-d"}')).toBe(true);
  });

  it("optional part: shown when a field is on, is or is not a value", async () => {
    const { card, value } = await open("{if field=Voucher}x{endif}{field name=Voucher type=toggle}", "Optional part");
    fireEvent.click(within(card).getByLabelText("is"));
    await flush();
    fireEvent.change(within(card).getByLabelText("Value"), { target: { value: "Gravel" } });
    await flush();
    expect(value()).toBe("{if field=Voucher is=Gravel}x{endif}{field name=Voucher type=toggle}");
    // Remove takes out its end too, and keeps the text.
    fireEvent.click(within(card).getByRole("button", { name: "Remove" }));
    await flush();
    expect(value()).toBe("x{field name=Voucher type=toggle}");
  });

  it("another snippet, clipboard and cursor", async () => {
    const a = await open('{snippet abbr=";sig"}', "Another snippet");
    fireEvent.change(within(a.card).getByLabelText("Abbreviation"), { target: { value: ";sig2" } });
    await flush();
    expect(a.value()).toBe('{snippet abbr=";sig2"}');
    cleanup();
    const b = await open("{clipboard}", "Clipboard");
    fireEvent.click(within(b.card).getByLabelText("Remove spaces and line breaks at both ends"));
    await flush();
    expect(b.value()).toBe("{clipboard trim=yes}");
    cleanup();
    const c = await open("{cursor}", "Cursor position");
    expect(within(c.card).getByText("After the snippet is typed, the caret is put here.")).toBeTruthy();
  });
});

describe("the insert list", () => {
  it("opens on {, filters as the user types, and Enter inserts the chip", async () => {
    const { box, value } = await setup("Hi ");
    place(box, "Hi ", 3);
    await type(box, "{");
    const list = screen.getByRole("listbox", { name: "Insert" });
    expect(within(list).getAllByRole("option")).toHaveLength(9);
    // `aria-expanded` is not an attribute of a textbox: the open list is `aria-controls`.
    expect(box.getAttribute("aria-controls")).toBe(list.id);
    expect(box.hasAttribute("aria-expanded")).toBe(false);
    await type(box, "da");
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual(["Datetoday, or shifted"]);
    await key(box, "Enter");
    expect(value()).toBe("Hi {date}");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("Esc keeps a plain brace, stored with {lbrace} when it would read as a macro", async () => {
    const { box, value } = await setup("");
    place(box, "", 0);
    await type(box, "{");
    await key(box, "Escape");
    expect(screen.queryByRole("listbox")).toBeNull();
    await type(box, "date}");
    expect(box.textContent).toBe("{date}");
    expect(value()).toBe("{lbrace}date}");
  });

  it("inserting a fill-in field opens its card; an optional part goes around the selection", async () => {
    const source = "Hello there";
    const { box, value } = await setup(source);
    place(box, source, 6, 11);
    fireEvent.click(screen.getByRole("button", { name: "More" }));
    await flush();
    fireEvent.click(screen.getByRole("menuitem", { name: /Optional part/ }));
    await flush();
    expect(value()).toBe("Hello {if field=\"Option 1\"}there{endif}");
    expect(screen.getByRole("dialog", { name: "Optional part" })).toBeTruthy();
  });

  it("the toolbar inserts at the caret", async () => {
    const { box, value } = await setup("ab");
    place(box, "ab", 1);
    fireEvent.click(screen.getByRole("button", { name: "Date" }));
    await flush();
    expect(value()).toBe("a{date}b");
    fireEvent.click(screen.getByRole("button", { name: "Fill-in field" }));
    await flush();
    expect(value()).toBe('a{date}{field name="Field 1"}b');
    expect(screen.getByRole("dialog", { name: "Fill-in field" })).toBeTruthy();
  });
});

describe("copy and paste", () => {
  function clipboardEvent(type: string, data: Record<string, string> = {}) {
    const store = { ...data };
    const e = new Event(type, { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { getData: (t: string) => string; setData: (t: string, v: string) => void };
    };
    e.clipboardData = { getData: (t) => store[t] ?? "", setData: (t, v) => (store[t] = v) };
    return { e, store };
  }

  it("copies chips as template text and pastes template text as chips", async () => {
    const source = "a {key name=tab} b";
    const { box, value } = await setup(source);
    place(box, source, 2, 3);
    const copy = clipboardEvent("copy");
    act(() => {
      box.dispatchEvent(copy.e);
    });
    expect(copy.store["text/plain"]).toBe("{key name=tab}");
    place(box, source, 5);
    const paste = clipboardEvent("paste", { "text/plain": "{cursor} and {" });
    act(() => {
      box.dispatchEvent(paste.e);
    });
    await flush();
    expect(value()).toBe("a {key name=tab} b{cursor} and {");
    expect(chips(box).map((c) => c.textContent)).toEqual(["Tab", "Cursor"]);
  });

  it("cut removes what it copied", async () => {
    const source = "a{cursor}b";
    const { box, value } = await setup(source);
    place(box, source, 0, 2);
    const cut = clipboardEvent("cut");
    act(() => {
      box.dispatchEvent(cut.e);
    });
    await flush();
    expect(cut.store["text/plain"]).toBe("a{cursor}");
    expect(value()).toBe("b");
  });
});

describe("errors", () => {
  it("are shown with the specification's sentence, the code and the faulty chip", async () => {
    const source = "{if field=A}As an apology";
    const { box, value } = await setup(source, {
      [source]: { code: "E_UNBALANCED_IF", start: 0, end: 12, snippet: null },
    });
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain(
      "This optional part has a start but no end. Put the caret where it should stop and insert “End if”.",
    );
    expect(alert.textContent).toContain("E_UNBALANCED_IF");
    expect(chips(box)[0]?.classList.contains("is-error")).toBe(true);
    expect(box.getAttribute("aria-invalid")).toBe("true");
    fireEvent.click(within(alert).getByRole("button", { name: "Insert it at the end" }));
    await flush();
    expect(value()).toBe("{if field=A}As an apology{endif}");
  });

  it("mark the text of a template the service cannot split, which is kept as written", async () => {
    const source = "ab {date fmt=";
    const { box, value } = await setup(source, {
      [source]: { code: "E_UNCLOSED_MACRO", start: 3, end: 13, snippet: null },
    });
    expect(box.querySelector(".tpl-error-text")?.textContent).toBe("{date fmt=");
    expect(screen.getByRole("alert").textContent).toContain(
      "Something that starts with { is never closed. Add the closing } or delete it.",
    );
    place(box, source, 0);
    await type(box, "!");
    expect(value()).toBe("!ab {date fmt=");
  });

  it("name the snippet that does not exist and offer to choose another", async () => {
    const source = '{snippet abbr=";sig2"}';
    await setup(source, { [source]: { code: "E_SNIPPET_NOT_FOUND", start: 0, end: 22, snippet: null } });
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("It includes the snippet ;sig2, which does not exist.");
    fireEvent.click(within(alert).getByRole("button", { name: "Choose another" }));
    await flush();
    expect(screen.getByRole("dialog", { name: "Another snippet" })).toBeTruthy();
  });
});

describe("the field", () => {
  it("is labelled by its label, never traps Tab, and Esc goes back", async () => {
    const { box, onEscape } = await setup("x");
    expect(box.getAttribute("aria-labelledby")).toBe("content-label");
    const tab = fireEvent.keyDown(box, { key: "Tab" });
    expect(tab).toBe(true); // not prevented: Tab moves on
    await key(box, "Escape");
    expect(onEscape).toHaveBeenCalledTimes(1);
  });

  it("takes a new value from outside and undoes edits", async () => {
    const { box, value } = await setup("one");
    place(box, "one", 3);
    await type(box, " two");
    expect(value()).toBe("one two");
    await key(box, "z", { ctrlKey: true, metaKey: true });
    expect(value()).toBe("one");
  });

  it("asks the service for the tokens after every change, and for the preview after a pause", async () => {
    vi.useFakeTimers();
    try {
      const { service } = fakeService();
      let current = "";
      const view = render(
        <TemplateField id="c" value={current} onChange={(v) => (current = v)} service={service} />,
      );
      const box = screen.getByRole("textbox");
      act(() => box.focus());
      for (const ch of "abc") {
        act(() => {
          box.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: ch, bubbles: true, cancelable: true }));
        });
        view.rerender(<TemplateField id="c" value={current} onChange={(v) => (current = v)} service={service} />);
        await act(async () => {
          await vi.advanceTimersByTimeAsync(50);
        });
      }
      expect(current).toBe("abc");
      expect(vi.mocked(service.parse).mock.calls.map(([s]) => s)).toEqual(["", "a", "b".replace("b", "ab"), "abc"]);
      // The preview: once for the empty start, then once 150 ms after the last key.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });
      const previews = vi.mocked(service.preview).mock.calls.map(([s]) => s);
      expect(previews.at(-1)).toBe("abc");
      expect(previews).not.toContain("a");
      expect(previews).not.toContain("ab");
    } finally {
      vi.useRealTimers();
    }
  });
});
