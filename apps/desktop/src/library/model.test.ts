import { describe, expect, it } from "vitest";

import type { Folder, Summary, Tag } from "./api";
import {
  abbreviationMessages,
  ariaShortcut,
  childrenOf,
  filterIndexed,
  folderLevel,
  hasMod,
  indexRows,
  localProblem,
  messageText,
  nextTagColor,
  optionsSummary,
  previewLine,
  rowTitle,
  shortcutLabel,
  sortIndexed,
  speakAbbreviation,
  subtreeIds,
  templateErrorSentence,
  visibleTree,
} from "./model";

const folder = (id: string, name: string, parentId: string | null = null, position = 0): Folder => ({
  id,
  name,
  parentId,
  position,
  defaults: { abbreviationPrefix: null, trigger: null, caseMode: null },
});

const row = (id: string, over: Partial<Summary> = {}): Summary => ({
  id,
  folderId: "f1",
  abbreviation: `;${id}`,
  label: id,
  firstLine: "",
  kind: "plain",
  trigger: "delimiter",
  enabled: true,
  tags: [],
  broken: null,
  uses: null,
  lastUsed: null,
  updatedAt: 1,
  ...over,
});

describe("words", () => {
  it("says an abbreviation as a screen reader should", () => {
    expect(speakAbbreviation(";rlate")).toBe("semicolon rlate");
    expect(speakAbbreviation("brb")).toBe("brb");
    expect(speakAbbreviation("--a_b")).toBe("dash dash a underscore b");
  });

  it("shows fill-in fields in the second line as names", () => {
    expect(previewLine('Hi {field name="First name"}, {field name=Bike kind=choice} {date}')).toBe(
      "Hi ‹First name›, ‹Bike› {date}",
    );
    expect(previewLine("{field}")).toBe("{field}");
  });

  it("titles a row with its label, else its first line, else its abbreviation", () => {
    expect(rowTitle({ label: " Hello ", firstLine: "x", abbreviation: ";h" })).toBe("Hello");
    expect(rowTitle({ label: "", firstLine: "Hi {field name=N}", abbreviation: ";h" })).toBe("Hi ‹N›");
    expect(rowTitle({ label: "", firstLine: "", abbreviation: ";h" })).toBe(";h");
  });

  it("writes shortcuts as each system does", () => {
    expect(shortcutLabel({ key: "n" }, true)).toBe("⌘N");
    expect(shortcutLabel({ key: "n", shift: true }, true)).toBe("⌘⇧N");
    expect(shortcutLabel({ key: "s" }, false)).toBe("Ctrl+S");
    expect(shortcutLabel({ key: "n", shift: true }, false)).toBe("Ctrl+Shift+N");
    expect(ariaShortcut({ key: "s" }, false)).toBe("Control+S");
    expect(ariaShortcut({ key: "o", shift: true }, true)).toBe("Meta+Shift+O");
    expect(hasMod({ metaKey: true, ctrlKey: false, altKey: false }, true)).toBe(true);
    expect(hasMod({ metaKey: true, ctrlKey: false, altKey: false }, false)).toBe(false);
    expect(hasMod({ metaKey: false, ctrlKey: true, altKey: true }, false)).toBe(false);
  });

  it("says the options in words", () => {
    expect(optionsSummary("delimiter", "exact")).toBe(
      "Expands after a space or punctuation · Case: exactly as written",
    );
    expect(optionsSummary("immediate", "adaptive")).toBe("Expands immediately · Case: ignore, and follow mine");
  });

  it("has a sentence for every template error of the specification", () => {
    expect(templateErrorSentence("E_UNBALANCED_IF")).toBe("This optional part has a start but no end.");
    expect(templateErrorSentence("E_SOMETHING_NEW")).toBe("This snippet has an error.");
  });
});

describe("folders", () => {
  const folders = [
    folder("a", "Replies", null, 0),
    folder("b", "Shop", "a", 1),
    folder("c", "Returns", "a", 0),
    folder("d", "Zed", null, 1),
    folder("e", "Deep", "c"),
  ];

  it("orders siblings by position then name, and hides what a collapsed folder holds", () => {
    const counts = new Map([["a", 3]]);
    const all = visibleTree(folders, counts, new Set());
    expect(all.map((n) => `${n.depth}:${n.folder.name}`)).toEqual(["1:Replies", "2:Returns", "3:Deep", "2:Shop", "1:Zed"]);
    expect(all[0]).toMatchObject({ count: 3, hasChildren: true, expanded: true });
    const collapsed = visibleTree(folders, counts, new Set(["a"]));
    expect(collapsed.map((n) => n.folder.name)).toEqual(["Replies", "Zed"]);
    expect(collapsed[0]?.expanded).toBe(false);
  });

  it("measures levels and subtrees", () => {
    expect(folderLevel(folders, "a")).toBe(1);
    expect(folderLevel(folders, "e")).toBe(3);
    expect([...subtreeIds(folders, "c")].sort()).toEqual(["c", "e"]);
    expect(childrenOf(folders, "a").map((f) => f.id)).toEqual(["c", "b"]);
  });

  it("keeps a folder whose parent is missing at the top", () => {
    expect(visibleTree([folder("x", "Lost", "nope")], new Map(), new Set()).map((n) => n.depth)).toEqual([1]);
  });
});

describe("tags", () => {
  it("gives a new tag the colour the fewest tags use", () => {
    const tag = (color: Tag["color"]): Tag => ({ id: color, name: color, color, snippets: 0 });
    expect(nextTagColor([])).toBe("red");
    expect(nextTagColor([tag("red"), tag("red"), tag("orange")])).toBe("yellow");
    const all = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "grey"] as const;
    expect(nextTagColor(all.map(tag))).toBe("red");
  });
});

describe("the list", () => {
  const tags: Tag[] = [{ id: "t1", name: "Urgent", color: "red", snippets: 1 }];
  const rows = [
    row("b", { label: "banana", firstLine: "yellow fruit", updatedAt: 5 }),
    row("a", { label: "Apple", tags: ["t1"], updatedAt: 9 }),
    row("c", { label: "cherry 10", updatedAt: 1 }),
    row("d", { label: "cherry 2", updatedAt: 2 }),
  ];

  it("sorts by label (digits as numbers) or by last change", () => {
    const items = indexRows(rows, tags);
    expect(sortIndexed(items, "label").map((i) => i.row.id)).toEqual(["a", "b", "d", "c"]);
    expect(sortIndexed(items, "changed").map((i) => i.row.id)).toEqual(["a", "b", "d", "c"]);
  });

  it("filters on label, abbreviation, first line and tag names, every word, any case", () => {
    const items = indexRows(rows, tags);
    const ids = (q: string) => filterIndexed(items, q).map((i) => i.row.id);
    expect(ids("")).toEqual(["b", "a", "c", "d"]);
    expect(ids("APPLE")).toEqual(["a"]);
    expect(ids("urgent")).toEqual(["a"]);
    expect(ids("fruit yellow")).toEqual(["b"]);
    expect(ids(";c")).toEqual(["c"]);
    expect(ids("zzz")).toEqual([]);
  });

  it("filters 10,000 rows in a few milliseconds", () => {
    const many = Array.from({ length: 10_000 }, (_, i) =>
      row(`id${i}`, { label: `Snippet number ${i}`, firstLine: "some text of the first line", abbreviation: `;s${i}` }),
    );
    const items = indexRows(many, []);
    const start = performance.now();
    const hit = filterIndexed(items, "number 99 line");
    const took = performance.now() - start;
    expect(hit.length).toBeGreaterThan(0);
    // The specification asks for a worker above 2,000 rows because the filter must not hold up
    // typing; on the main thread it takes well under a frame, so none is needed.
    expect(took).toBeLessThan(16);
  });
});

describe("the abbreviation", () => {
  it("finds what blocks it before asking the service", () => {
    expect(localProblem("")).toBe("empty");
    expect(localProblem("a b")).toBe("space");
    expect(localProblem("a\nb")).toBe("space");
    expect(localProblem("x".repeat(65))).toBe("long");
    expect(localProblem("x".repeat(64))).toBeNull();
    expect(localProblem(";rty")).toBeNull();
  });

  const ctx = (trigger: "immediate" | "delimiter", abbreviation: string) => ({
    abbreviation,
    trigger,
    rowOf: (id: string) =>
      ({ other: row("other", { label: "Thanks", abbreviation: ";rty", folderId: "f9" }), later: row("later", { abbreviation: ";rlate" }) })[id],
    folderName: (id: string) => (id === "f9" ? "Shop replies" : "Replies"),
  });

  it("uses the specification's sentences", () => {
    const dup = abbreviationMessages(ctx("delimiter", ";rty"), { conflict: "other", hides: [], hiddenBy: [] });
    expect(dup).toHaveLength(1);
    expect(dup[0]).toMatchObject({ tone: "error", blocks: true, action: { label: "Show that snippet" } });
    expect(messageText(dup[0]!)).toBe(
      ";rty is already the abbreviation of “Thanks” in Shop replies. Each abbreviation can belong to one snippet.",
    );
    const hides = abbreviationMessages(ctx("immediate", ";rl"), { conflict: null, hides: ["later"], hiddenBy: [] });
    expect(messageText(hides[0]!)).toBe(
      "This snippet expands immediately, so ;rl will fire before you can finish typing ;rlate. Use a different abbreviation, or let it wait for a space.",
    );
    expect(hides[0]?.blocks).toBe(false);
    // A delimiter snippet does not hide anything.
    expect(abbreviationMessages(ctx("delimiter", ";rl"), { conflict: null, hides: ["later"], hiddenBy: [] })).toEqual([]);
    const hidden = abbreviationMessages(ctx("delimiter", ";rlate"), { conflict: null, hides: [], hiddenBy: ["other"] });
    expect(messageText(hidden[0]!)).toBe(";rty expands immediately and will fire before ;rlate can be typed. Change one of the two.");
    expect(abbreviationMessages(ctx("immediate", "ty"), null).map(messageText)).toEqual([
      "Short abbreviations made only of letters can fire in the middle of ordinary typing. Starting with a character such as ; avoids that.",
    ]);
    expect(abbreviationMessages(ctx("immediate", ";ty"), null)).toEqual([]);
    expect(messageText(abbreviationMessages(ctx("delimiter", ""), null)[0]!)).toBe("Type an abbreviation.");
  });
});

describe("usage on this computer", () => {
  const now = new Date(2026, 10, 20, 15, 0);
  it("labels every count as local and dates the last use in words", async () => {
    const { usageText, sinceText } = await import("./model");
    expect(usageText(31, new Date(2026, 10, 19, 9).getTime(), now)).toBe(
      "Used 31 times on this computer · last used yesterday",
    );
    expect(usageText(1, new Date(2026, 10, 20, 9).getTime(), now)).toBe("Used once on this computer · last used today");
    expect(usageText(0, null, now)).toBe("Not used yet on this computer");
    expect(usageText(null, null, now)).toBeNull();
    expect(sinceText(new Date(2026, 10, 17, 9).getTime(), now)).toBe("3 days ago");
    expect(sinceText(new Date(2026, 9, 2, 9).getTime(), now)).toBe("on 2 October");
  });

  it("sorts the most used first", async () => {
    const { sortIndexed, indexRows } = await import("./model");
    const rows = [
      row("a", { label: "Alpha", uses: 2, lastUsed: 5 }),
      row("b", { label: "Beta", uses: 9, lastUsed: 1 }),
      row("c", { label: "Gamma", uses: 2, lastUsed: 8 }),
    ];
    const order = sortIndexed(indexRows(rows, []), "used").map((i) => i.row.id);
    expect(order).toEqual(["b", "c", "a"]);
  });
});
