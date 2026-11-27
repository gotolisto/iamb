// @vitest-environment jsdom
// The template editor for a formatted snippet (task 3.8): the formatting toolbar, images from the
// file picker, a paste or a drop, and the formatted preview from the service's rendering.
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AddedAttachment, TemplatePreview, TemplateService } from "./api";
import { select } from "./dom";
import { referenceTokens } from "./reference-tokenizer";
import { IMAGE_FAILED, IMAGE_REFUSED, TemplateField } from "./TemplateField";

afterEach(cleanup);

const SHA = "ab".repeat(32);

function service(preview?: TemplatePreview) {
  const added: Array<{ folder: string; name: string; size: number }> = [];
  let refuse: string | null = null;
  const s: TemplateService = {
    parse: vi.fn((source: string) => Promise.resolve({ tokens: referenceTokens(source), error: null })),
    preview: vi.fn(
      (): Promise<TemplatePreview> =>
        Promise.resolve(preview ?? { parts: [], cursor: null, fields: [], error: null }),
    ),
    addAttachment: vi.fn((folder: string, name: string, bytes: Uint8Array): Promise<AddedAttachment> => {
      if (refuse) return Promise.reject({ code: refuse, message: "no" });
      added.push({ folder, name, size: bytes.length });
      return Promise.resolve({
        sha256: SHA,
        mime: "image/png",
        size: bytes.length,
        fileName: name,
        markdown: `![${name.replace(/\.[^.]*$/, "")}](attachment:${SHA})`,
      });
    }),
  };
  return { s, added, refuseWith: (code: string | null) => (refuse = code) };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

async function setup(value: string, svc: TemplateService, folder: string | null = "f1") {
  let current = value;
  const onChange = vi.fn((v: string) => {
    current = v;
    view.rerender(<Field />);
  });
  function Field() {
    return (
      <>
        <label htmlFor="content">Content</label>
        <TemplateField
          id="content"
          value={current}
          onChange={onChange}
          service={svc}
          kind="rich"
          imageFolder={folder ?? undefined}
          preview
        />
      </>
    );
  }
  const view = render(<Field />);
  await flush();
  const box = screen.getByRole("textbox", { name: "Content" });
  return { box, value: () => current, view };
}

function png(name: string): File {
  return new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2])], name, {
    type: "image/png",
  });
}

describe("a formatted snippet's toolbar", () => {
  it("has bold, italic, link, list and image, and no cursor position", async () => {
    const { s } = service();
    await setup("text", s);
    const bar = screen.getByRole("toolbar", { name: "Formatting" });
    const names = [...bar.querySelectorAll("button")].map((b) => b.textContent);
    expect(names).toEqual(["B Bold", "I Italic", "Link", "List", "Image…"]);
    const insert = screen.getByRole("toolbar", { name: "Insert" });
    expect(insert.textContent).not.toContain("Cursor");
  });

  it("writes Markdown around the selection", async () => {
    const { s } = service();
    const { box, value } = await setup("Thanks for your order", s);
    act(() => {
      box.focus();
      select(box, [{ kind: "text", text: "Thanks for your order", source: "Thanks for your order", raw: false }], 11, 21);
    });
    fireEvent.click(screen.getByRole("button", { name: /Bold/ }));
    await flush();
    expect(value()).toBe("Thanks for **your order**");
    fireEvent.keyDown(box, { key: "z", ctrlKey: true });
    await flush();
    expect(value()).toBe("Thanks for your order");
  });

  it("has keyboard shortcuts for bold, italic and link", async () => {
    const { s } = service();
    const { box, value } = await setup("", s);
    act(() => box.focus());
    fireEvent.keyDown(box, { key: "b", ctrlKey: true });
    await flush();
    expect(value()).toBe("**bold text**");
  });

  it("adds an image from the file picker where the caret was", async () => {
    const { s, added } = service();
    const { value } = await setup("Kim", s, "folder-7");
    const input = screen.getByTestId("image-file") as HTMLInputElement;
    await act(async () => {
      fireEvent.change(input, { target: { files: [png("Team logo.png")] } });
    });
    await flush();
    expect(added).toEqual([{ folder: "folder-7", name: "Team logo.png", size: 10 }]);
    expect(value()).toContain(`![Team logo](attachment:${SHA})`);
  });

  it("adds an image pasted or dropped into the text", async () => {
    const { s, added } = service();
    const { box, value } = await setup("", s);
    act(() => box.focus());
    const paste = new Event("paste", { bubbles: true, cancelable: true }) as Event & { clipboardData: unknown };
    paste.clipboardData = { files: [png("shot.png")], getData: () => "" };
    await act(async () => {
      box.dispatchEvent(paste);
    });
    await flush();
    expect(added.map((a) => a.name)).toEqual(["shot.png"]);
    const drop = new Event("drop", { bubbles: true, cancelable: true }) as Event & { dataTransfer: unknown };
    drop.dataTransfer = { files: [png("dropped.png")] };
    await act(async () => {
      box.dispatchEvent(drop);
    });
    await flush();
    expect(added.map((a) => a.name)).toEqual(["shot.png", "dropped.png"]);
    expect(value().match(/attachment:/g)).toHaveLength(2);
  });

  it("says which images it can add", async () => {
    const { s, refuseWith } = service();
    await setup("", s);
    const input = screen.getByTestId("image-file") as HTMLInputElement;
    const svg = new File(["<svg/>"], "x.svg", { type: "image/svg+xml" });
    await act(async () => {
      fireEvent.change(input, { target: { files: [svg] } });
    });
    await flush();
    expect(screen.getByRole("alert").textContent).toBe(IMAGE_REFUSED);
    refuseWith("invalid");
    await act(async () => {
      fireEvent.change(input, { target: { files: [png("big.png")] } });
    });
    await flush();
    expect(screen.getByRole("alert").textContent).toBe(IMAGE_REFUSED);
    refuseWith("disconnected");
    await act(async () => {
      fireEvent.change(input, { target: { files: [png("a.png")] } });
    });
    await flush();
    expect(screen.getByRole("alert").textContent).toBe(IMAGE_FAILED);
  });

  it("refuses a file over 5 MB before reading it", async () => {
    const { s } = service();
    await setup("", s);
    const input = screen.getByTestId("image-file") as HTMLInputElement;
    const big = png("huge.png");
    Object.defineProperty(big, "size", { value: 5 * 1024 * 1024 + 1 });
    const read = vi.spyOn(big, "arrayBuffer");
    await act(async () => {
      fireEvent.change(input, { target: { files: [big] } });
    });
    await flush();
    expect(screen.getByRole("alert").textContent).toBe(IMAGE_REFUSED);
    expect(read).not.toHaveBeenCalled();
    expect(s.addAttachment).not.toHaveBeenCalled();
  });

  it("has no Image button without a folder to keep images in", async () => {
    const { s } = service();
    await setup("", s, null);
    expect(screen.queryByRole("button", { name: /Image/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Bold/ })).toBeTruthy();
  });
});

describe("the formatted preview", () => {
  it("asks the service for the formatted rendering and shows its HTML", async () => {
    const { s } = service({
      parts: [
        {
          part: "formatted",
          html: '<p><strong>Kim</strong> <a href="https://example.com">site</a></p>',
          plain: "Kim site",
        },
        { part: "key", key: "tab", shift: false, times: 1 },
      ],
      cursor: null,
      fields: [],
      error: null,
    });
    await setup("**Kim** [site](https://example.com){key name=tab}", s);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 200));
    });
    await flush();
    expect(s.preview).toHaveBeenCalledWith("**Kim** [site](https://example.com){key name=tab}", undefined, true);
    const preview = screen.getByRole("region", { name: "Preview" });
    expect(preview.querySelector("strong")?.textContent).toBe("Kim");
    const link = preview.querySelector("a") as HTMLAnchorElement;
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(preview.querySelector("kbd")).toBeTruthy();
  });

  async function withLink(openLink: ((url: string) => Promise<void>) | undefined) {
    const { s } = service({
      parts: [
        { part: "formatted", html: '<p><a href="https://example.com/a?b=1&amp;c=2">site</a></p>', plain: "site" },
      ],
      cursor: null,
      fields: [],
      error: null,
    });
    if (openLink) s.openLink = openLink;
    await setup("x", s);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 200));
    });
    await flush();
    return screen.getByRole("link", { name: "site" });
  }

  it("keeps a link a link, with its address as the tooltip", async () => {
    const link = await withLink(vi.fn(() => Promise.resolve()));
    expect(link.getAttribute("href")).toBe("https://example.com/a?b=1&c=2");
    expect(link.getAttribute("title")).toBe("https://example.com/a?b=1&c=2");
    link.focus();
    expect(document.activeElement).toBe(link);
  });

  it("opens a clicked link in the system browser and does not navigate", async () => {
    const open = vi.fn(() => Promise.resolve());
    const link = await withLink(open);
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith("https://example.com/a?b=1&c=2");
    const aux = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    link.dispatchEvent(aux);
    expect(aux.defaultPrevented).toBe(true);
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("opens a focused link on Enter, once, and other keys do nothing", async () => {
    const open = vi.fn(() => Promise.resolve());
    const link = await withLink(open);
    link.focus();
    const space = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    link.dispatchEvent(space);
    expect(open).not.toHaveBeenCalled();
    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    link.dispatchEvent(enter);
    // Prevented, so the browser makes no click of its own: a second open and a navigation.
    expect(enter.defaultPrevented).toBe(true);
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith("https://example.com/a?b=1&c=2");
  });

  it("does not navigate, and does not fail, when the link cannot be opened", async () => {
    const link = await withLink(vi.fn(() => Promise.reject({ code: "link_refused", message: "no" })));
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    await flush();
    expect(click.defaultPrevented).toBe(true);
    // Without an opener at all (a service that has none) a click is still harmless.
    cleanup();
    const bare = await withLink(undefined);
    const again = new MouseEvent("click", { bubbles: true, cancelable: true });
    bare.dispatchEvent(again);
    expect(again.defaultPrevented).toBe(true);
  });

  it("keeps a column's alignment without inline style, and keeps a link focusable", async () => {
    const { s } = service({
      parts: [
        {
          part: "formatted",
          html:
            '<p><a href="https://example.com/a?b=1&amp;c=2">site</a></p>\n<table>\n<thead><tr><th style="text-align:left">a</th><th style="text-align:right">b</th></tr></thead>\n<tr><td style="text-align:center">1</td><td>2</td></tr>\n</table>',
          plain: "site",
        },
      ],
      cursor: null,
      fields: [],
      error: null,
    });
    await setup("x", s);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 200));
    });
    await flush();
    const preview = screen.getByRole("region", { name: "Preview" });
    expect(preview.querySelector("[style]")).toBeNull();
    expect(preview.querySelector("th:last-child")?.getAttribute("data-align")).toBe("right");
    expect(preview.querySelector("td")?.getAttribute("data-align")).toBe("center");
    const link = preview.querySelector("a") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("https://example.com/a?b=1&c=2");
    expect(link.getAttribute("title")).toBe("https://example.com/a?b=1&c=2");
    expect(screen.getByRole("link", { name: "site" })).toBe(link);
  });
});
