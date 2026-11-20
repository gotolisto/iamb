// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TemplatePreview as Preview, TemplateService } from "./api";
import { TemplatePreview } from "./TemplatePreview";

afterEach(cleanup);
beforeEach(() => {
  vi.useFakeTimers();
  return () => vi.useRealTimers();
});

const empty: Preview = { parts: [], cursor: null, fields: [], error: null };

function service(answer: (source: string) => Promise<Preview>): TemplateService {
  return {
    parse: vi.fn(() => Promise.resolve({ tokens: [], error: null })),
    preview: vi.fn(answer),
  };
}

const advance = (ms: number) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });

describe("the preview", () => {
  it("asks the service 150 ms after the last change, once", async () => {
    const s = service(() => Promise.resolve(empty));
    const view = render(<TemplatePreview source="a" service={s} />);
    await advance(100);
    view.rerender(<TemplatePreview source="ab" service={s} />);
    await advance(100);
    view.rerender(<TemplatePreview source="abc" service={s} />);
    await advance(149);
    expect(s.preview).not.toHaveBeenCalled();
    await advance(1);
    expect(vi.mocked(s.preview).mock.calls).toEqual([["abc", undefined]]);
  });

  it("drops an older answer that arrives after a newer one", async () => {
    const resolvers: Array<(p: Preview) => void> = [];
    const s = service(() => new Promise<Preview>((r) => resolvers.push(r)));
    const text = (t: string): Preview => ({ ...empty, parts: [{ part: "text", text: t, blanks: [], clipboard: [] }] });
    const view = render(<TemplatePreview source="old" service={s} />);
    await advance(150);
    view.rerender(<TemplatePreview source="new" service={s} />);
    await advance(150);
    expect(resolvers).toHaveLength(2);
    await act(async () => resolvers[1]?.(text("NEW")));
    await act(async () => resolvers[0]?.(text("OLD")));
    expect(screen.getByLabelText("Preview").textContent).toContain("NEW");
    expect(screen.getByLabelText("Preview").textContent).not.toContain("OLD");
  });

  it("shows text, grey name tags, the clipboard sample, key badges, waits, the cursor and the fields", async () => {
    const preview: Preview = {
      parts: [
        {
          part: "text",
          text: "Hi Name, Clipboard",
          blanks: [{ start: 3, end: 7, field: "Name", sample: true }],
          clipboard: [[9, 18]],
        },
        { part: "key", key: "tab", shift: true, times: 2 },
        { part: "wait", ms: 500 },
        { part: "text", text: "M!", blanks: [], clipboard: [] },
      ],
      cursor: { part: 3, offset: 1 },
      fields: [
        { name: "Name", kind: "text", options: [], default: "" },
        { name: "Size", kind: "choice", options: ["S", "M"], default: "M" },
      ],
      error: null,
    };
    const onTryForm = vi.fn();
    render(<TemplatePreview source="x" service={service(() => Promise.resolve(preview))} onTryForm={onTryForm} />);
    await advance(150);
    const region = screen.getByLabelText("Preview");
    expect(region.querySelector(".tpl-tag")?.textContent).toBe("Name");
    expect(Array.from(region.querySelectorAll(".tpl-tag")).map((t) => t.textContent)).toEqual(["Name", "Clipboard"]);
    const kbd = region.querySelector("kbd");
    expect(kbd?.textContent).toBe("Shift+Tab ×2");
    expect(kbd?.getAttribute("aria-label")).toBe("Key press, Shift+Tab, 2 times");
    expect(region.querySelector(".tpl-wait")?.textContent).toBe(" · 0.5 s · ");
    // The caret sits between "M" and "!".
    const caret = screen.getByRole("img", { name: "Cursor position" });
    expect(caret.previousSibling?.textContent).toBe("M");
    expect(caret.nextSibling?.textContent).toBe("!");
    expect(region.textContent).toContain("Asks for 2 values");
    expect(region.textContent).toContain("Choice: S, M · starts with “M”");
    fireEvent.click(screen.getByRole("button", { name: "Try the form" }));
    expect(onTryForm).toHaveBeenCalledWith(preview.fields);
  });

  it("sends test values only in the request", async () => {
    const s = service(() => Promise.resolve(empty));
    render(<TemplatePreview source="Hi {field name=Name}" service={s} values={{ Name: "Ada" }} />);
    await advance(150);
    expect(vi.mocked(s.preview).mock.calls).toEqual([["Hi {field name=Name}", { Name: "Ada" }]]);
  });

  it("says “Preview unavailable” with the reason for a template error", async () => {
    const s = service(() =>
      Promise.resolve({ ...empty, error: { code: "E_WAIT_TOTAL", start: null, end: null, snippet: null } }),
    );
    render(<TemplatePreview source="x" service={s} />);
    await advance(150);
    expect(screen.getByLabelText("Preview").textContent).toContain(
      "Preview unavailable The waits add up to more than 30 seconds.",
    );
  });

  it("says so when Iamb is not running", async () => {
    const s = service(() => Promise.reject({ code: "disconnected", message: "no service" }));
    render(<TemplatePreview source="x" service={s} />);
    await advance(150);
    expect(screen.getByLabelText("Preview").textContent).toContain("Preview unavailable Iamb is not running.");
  });

  it("never reads the clipboard", async () => {
    const readText = vi.fn(() => Promise.resolve("secret"));
    Object.defineProperty(navigator, "clipboard", { value: { readText }, configurable: true });
    const preview: Preview = {
      ...empty,
      parts: [{ part: "text", text: "Clipboard", blanks: [], clipboard: [[0, 9]] }],
    };
    render(<TemplatePreview source="{clipboard}" service={service(() => Promise.resolve(preview))} />);
    await advance(150);
    expect(readText).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Preview").textContent).not.toContain("secret");
  });
});
