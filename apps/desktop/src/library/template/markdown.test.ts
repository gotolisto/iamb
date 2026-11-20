import { describe, expect, it } from "vitest";

import { hasFormatting, insertMarkdown, LINK_ADDRESS, linkSelection, toggleList, wrapSelection } from "./markdown";
import { docFromTokens, displayText, serialize, type EditorState } from "./model";
import { referenceTokens } from "./reference-tokenizer";

function state(source: string, anchor: number, focus = anchor): EditorState {
  return { doc: docFromTokens(source, referenceTokens(source)), anchor, focus };
}

describe("the formatting toolbar's edits", () => {
  it("wraps the selection and keeps it on the content", () => {
    const s = wrapSelection(state("Thanks for your order", 11, 21), "**", "**", "bold text");
    expect(serialize(s.doc)).toBe("Thanks for **your order**");
    expect(displayText(s.doc).slice(s.anchor, s.focus)).toBe("your order");
  });

  it("puts a selected placeholder between the marks of an empty selection", () => {
    const s = wrapSelection(state("Hi ", 3), "*", "*", "italic text");
    expect(serialize(s.doc)).toBe("Hi *italic text*");
    expect(displayText(s.doc).slice(s.anchor, s.focus)).toBe("italic text");
  });

  it("keeps a chip in the selection a chip", () => {
    const source = 'Dear {field name="First name"},';
    // The chip is one character at display position 5.
    const s = wrapSelection(state(source, 5, 6), "**", "**", "bold text");
    expect(serialize(s.doc)).toBe('Dear **{field name="First name"}**,');
    expect(s.doc.filter((p) => p.kind === "chip")).toHaveLength(1);
  });

  it("makes a link with the address selected", () => {
    const s = linkSelection(state("see the returns page", 8, 20));
    expect(serialize(s.doc)).toBe(`see the [returns page](${LINK_ADDRESS})`);
    expect(displayText(s.doc).slice(s.anchor, s.focus)).toBe(LINK_ADDRESS);
  });

  it("turns lines into a list and back", () => {
    const listed = toggleList(state("one\ntwo\nthree", 1, 6));
    expect(serialize(listed.doc)).toBe("- one\n- two\nthree");
    const back = toggleList(listed);
    expect(serialize(back.doc)).toBe("one\ntwo\nthree");
    // A selection that ends at the start of a line leaves that line alone.
    expect(serialize(toggleList(state("a\nb", 0, 2)).doc)).toBe("- a\nb");
  });

  it("inserts an image's Markdown at the caret", () => {
    const md = `![Logo](attachment:${"ab".repeat(32)})`;
    const s = insertMarkdown(state("Kim\n", 4), md);
    expect(serialize(s.doc)).toBe(`Kim\n${md}`);
    expect(s.anchor).toBe(4 + md.length);
  });
});

describe("whether switching to plain text loses formatting", () => {
  it("finds the Markdown the toolbar and the language write", () => {
    for (const source of [
      "**bold**",
      "an *italic* word",
      "an _italic_ word",
      "~~gone~~",
      "[site](https://example.com)",
      `![Logo](attachment:${"ab".repeat(32)})`,
      "- item",
      "1. item",
      "# Title",
      "> quoted",
      "`code`",
      "---",
      "| a | b |",
      "<https://example.com>",
    ]) {
      expect(hasFormatting(source), source).toBe(true);
    }
  });

  it("does not ask for text without formatting", () => {
    for (const source of [
      "Thanks for your order.",
      "2 * 3 = 6",
      "snake_case_name",
      'Hi {field name="Name"}, see you {date fmt="%A"}',
      "Price: 5 - 10 EUR",
      "",
    ]) {
      expect(hasFormatting(source), source).toBe(false);
    }
  });
});
