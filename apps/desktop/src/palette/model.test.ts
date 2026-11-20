import { describe, expect, it } from "vitest";

import type { TemplatePreview } from "../library/template/api";
import {
  actionFor,
  bodyRanges,
  copy,
  editAction,
  itemsFor,
  jump,
  keysFor,
  matchedTag,
  move,
  pieces,
  previewText,
  subLine,
  verbOf,
  type Answer,
} from "./model";
import { hit } from "./testkit";

const answer = (query: string, ...hits: ReturnType<typeof hit>[]): Answer => ({
  query,
  results: { total: 76, hits },
});

describe("the rows of the palette", () => {
  it("has no rows for an empty query, or the form to fill in again", () => {
    expect(itemsFor("", null, null)).toEqual([]);
    expect(itemsFor("   ", null, "Late delivery")).toEqual([{ kind: "again", label: "Late delivery" }]);
  });

  it("lists the hits, and one row that creates a snippet when nothing matches", () => {
    const a = hit();
    expect(itemsFor("ret", answer("ret", a), "Late delivery")).toEqual([{ kind: "hit", hit: a }]);
    expect(itemsFor("warranty", answer("warranty"), null)).toEqual([{ kind: "new", label: "warranty" }]);
  });

  it("shows nothing for a query that has not been answered yet, not the old answer", () => {
    expect(itemsFor("rete", answer("ret", hit()), null)).toEqual([]);
  });
});

describe("what Enter does", () => {
  const items = {
    plain: { kind: "hit" as const, hit: hit() },
    asks: { kind: "hit" as const, hit: hit({ firstLine: "Hi {field name=Name}" }) },
    review: { kind: "hit" as const, hit: hit({ needsReview: true }) },
    broken: { kind: "hit" as const, hit: hit({ broken: "E_UNKNOWN_MACRO" }) },
    off: { kind: "hit" as const, hit: hit({ enabled: false }) },
    clash: { kind: "hit" as const, hit: hit({ broken: "duplicate" }) },
  };

  it("follows the table of flow 5", () => {
    expect(verbOf(items.plain, false)).toBe("insert");
    expect(verbOf(items.asks, false)).toBe("fill");
    expect(verbOf(items.review, false)).toBe("review");
    expect(verbOf(items.broken, false)).toBe("open");
    expect(verbOf(items.off, false)).toBe("insert");
    expect(verbOf(items.clash, false)).toBe("insert");
    expect(verbOf({ kind: "again", label: "x" }, false)).toBe("form");
    expect(verbOf({ kind: "new", label: "x" }, false)).toBe("create");
  });

  it("types or opens the form for a snippet, and opens the library for the others", () => {
    expect(actionFor(items.plain, false)).toEqual({ kind: "insert", id: items.plain.hit.row.id });
    expect(actionFor(items.asks, false)).toEqual({ kind: "insert", id: items.asks.hit.row.id });
    expect(actionFor(items.review, false)).toEqual({ kind: "show", id: items.review.hit.row.id });
    expect(actionFor(items.broken, false)).toEqual({ kind: "show", id: items.broken.hit.row.id });
    expect(actionFor({ kind: "again", label: "x" }, false)).toEqual({ kind: "fill_again" });
    expect(actionFor({ kind: "new", label: "warranty" }, false)).toEqual({
      kind: "new",
      label: "warranty",
    });
  });

  it("selects the snippet in the library when opened from Iamb's own window", () => {
    expect(verbOf(items.asks, true)).toBe("select");
    expect(actionFor(items.asks, true)).toEqual({ kind: "show", id: items.asks.hit.row.id });
  });

  it("has an edit pick for snippets only", () => {
    expect(editAction(items.plain)).toEqual({ kind: "show", id: items.plain.hit.row.id });
    expect(editAction({ kind: "new", label: "x" })).toBeNull();
    expect(editAction(undefined)).toBeNull();
  });
});

describe("marking matches", () => {
  it("cuts text at ranges in UTF-16 code units", () => {
    expect(pieces("Return label", [[0, 3]])).toEqual([
      { text: "Ret", marked: true },
      { text: "urn label", marked: false },
    ]);
    // A surrogate pair counts two units, as the service counts them.
    expect(pieces("😀 Résumé", [[3, 6]])).toEqual([
      { text: "😀 ", marked: false },
      { text: "Rés", marked: true },
      { text: "umé", marked: false },
    ]);
  });

  it("tidies ranges that overlap, run out of order or past the text", () => {
    expect(pieces("abcdef", [[4, 9], [1, 3], [2, 5]])).toEqual([
      { text: "a", marked: false },
      { text: "bcdef", marked: true },
    ]);
    expect(pieces("abc", [])).toEqual([{ text: "abc", marked: false }]);
    expect(pieces("", [[0, 2]])).toEqual([{ text: "", marked: false }]);
    expect(pieces("abc", [[2, 2], [5, 6]])).toEqual([{ text: "abc", marked: false }]);
  });

  it("marks a body match only where the row shows the content", () => {
    const first = "You can return any part";
    const inLine = hit({ firstLine: first }, { field: "body", ranges: [[8, 14]] });
    expect(bodyRanges(inLine)).toEqual([[8, 14]]);
    const later = hit({ firstLine: first }, { field: "body", ranges: [[400, 406]] });
    expect(bodyRanges(later)).toEqual([]);
    const blanks = hit({ firstLine: "Hi {field name=Name}" }, { field: "body", ranges: [[0, 2]] });
    expect(bodyRanges(blanks)).toEqual([]);
    expect(bodyRanges(hit({}, { field: "label", ranges: [[0, 2]] }))).toEqual([]);
  });

  it("finds the tag a hit matched", () => {
    const h = hit({ tags: ["orders", "returns"] }, { field: "tag", ranges: [[0, 3]] });
    expect(matchedTag(h, "ret")).toBe(1);
    expect(matchedTag(hit({ tags: ["a"] }, { field: "label" }), "a")).toBe(-1);
    // A typo: no tag holds the typed text; the first long enough is the one.
    const typo = hit({ tags: ["orders", "returns"] }, { field: "tag", ranges: [[0, 7]] });
    expect(matchedTag(typo, "retrun")).toBe(1);
  });

  it("describes a row by what Enter does when it is not plain", () => {
    expect(subLine(hit({ needsReview: true }))).toContain("review");
    expect(subLine(hit({ broken: "E_X" }))).toContain("template error");
    expect(subLine(hit({ firstLine: "Hi {field name=Name}," }))).toBe("Hi ‹Name›,");
  });
});

describe("moving", () => {
  it("wraps with the arrows and stops at the ends with the page keys", () => {
    expect(move(0, -1, 4)).toBe(3);
    expect(move(3, 1, 4)).toBe(0);
    expect(move(0, 1, 0)).toBe(0);
    expect(jump(2, 1, 30)).toBe(10);
    expect(jump(25, 1, 30)).toBe(29);
    expect(jump(3, -1, 30)).toBe(0);
  });
});

describe("the preview", () => {
  it("is the text the snippet would type, with keys marked", () => {
    const preview: TemplatePreview = {
      parts: [
        { part: "text", text: "Hi ", blanks: [], clipboard: [] },
        { part: "key", key: "tab", shift: false, times: 2 },
        { part: "wait", ms: 100 },
      ],
      cursor: null,
      fields: [],
      error: null,
    };
    expect(previewText(preview)).toBe("Hi ⟨tab ×2⟩");
    expect(previewText(null)).toBe("");
    expect(previewText({ ...preview, error: { code: "E_X", message: "m", start: 0, end: 1 } as never })).toBe("");
  });
});

describe("the copy", () => {
  it("is the approved wording", () => {
    expect(copy.again("Late delivery")).toBe("Fill in “Late delivery” again");
    expect(copy.noMatch("warranty")).toBe("No snippet matches “warranty”.");
    expect(copy.newSnippet("warranty")).toBe("New snippet labelled “warranty”");
    expect(copy.count(4, 76)).toBe("4 of 76");
    expect(copy.hint).toBe("Type to search by abbreviation, label, text or tag.");
    expect(copy.couldNotGetBack("Mail")).toBe(
      "Iamb could not get back to Mail. Click where the text should go and press Enter again.",
    );
    expect(copy.notRunning).toBe("Iamb is not running.");
    expect(keysFor("mac").edit).toBe("⌘E");
    expect(keysFor("win").edit).toBe("Ctrl+E");
  });
});
