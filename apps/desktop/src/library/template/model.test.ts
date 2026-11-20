import { describe, expect, it } from "vitest";

import type { TemplateToken } from "./api";
import {
  applyTokens,
  chip,
  copySource,
  deleteBackward,
  deleteForward,
  displayOffset,
  displayText,
  docFromRaw,
  docFromTokens,
  docLength,
  fieldNames,
  insertPieces,
  insertTemplate,
  insertText,
  macroSource,
  moveCaret,
  optionalPartOf,
  quoteValue,
  removeChips,
  selectedChip,
  serialize,
  sourceOffset,
  stateOf,
  updateChip,
  type ChipPiece,
  type EditorState,
} from "./model";
import { referenceTokens } from "./reference-tokenizer";
import fixtures from "./spec.fixtures.json";

interface Example {
  id: string;
  template: string;
  tokens: TemplateToken[];
  error: { code: string } | null;
}

const examples = fixtures.examples as Example[];

/** The document of `source`, with the tokens the service would give. */
const load = (source: string) => docFromTokens(source, referenceTokens(source));

describe("round trip: every example of the template language (KPI: 100%)", () => {
  // The tokens are the service's own answers (src-tauri/tests/template_commands.rs).
  it("loads each example's template with the service's tokens and writes back the same bytes", () => {
    const failed: string[] = [];
    for (const ex of examples) {
      const doc = docFromTokens(ex.template, ex.tokens);
      if (serialize(doc) !== ex.template) failed.push(ex.id);
    }
    const passed = examples.length - failed.length;
    // Reported by the task: N of the examples round-trip with no change.
    console.info(`template round trip: ${passed} of ${examples.length} examples unchanged`);
    expect(failed).toEqual([]);
    expect(examples).toHaveLength(173);
  });

  it("also after the editor has shown it: text, chips and positions survive", () => {
    for (const ex of examples) {
      const doc = docFromTokens(ex.template, ex.tokens);
      const chips = doc.filter((p) => p.kind === "chip").length;
      const macros = ex.tokens.filter((t) => t.kind !== "text" && t.kind !== "lbrace").length;
      expect(chips, ex.id).toBe(macros);
      // Every display position maps to a source offset and back.
      for (let pos = 0; pos <= docLength(doc); pos++) {
        expect(displayOffset(doc, sourceOffset(doc, pos)), ex.id).toBe(pos);
      }
      // A document rebuilt from its own text after an edit elsewhere keeps the untouched part.
      const edited = insertText(stateOf(doc), "!");
      expect(serialize(edited.doc), ex.id).toBe(`${ex.template}!`);
    }
  });

  it("raw (before the tokens, or for a template the service cannot split) is exact too", () => {
    for (const ex of examples) {
      expect(serialize(docFromRaw(ex.template)), ex.id).toBe(ex.template);
      const state = applyTokens(stateOf(docFromRaw(ex.template)), ex.template, ex.tokens);
      expect(serialize(state.doc), ex.id).toBe(ex.template);
    }
  });

  it("the test oracle agrees with the service on every example", () => {
    for (const ex of examples) expect(referenceTokens(ex.template), ex.id).toEqual(ex.tokens);
  });
});

describe("the document", () => {
  it("shows {lbrace} as a brace and CRLF as a line break, and keeps both", () => {
    const source = "a{lbrace}date}\r\nb\rc";
    const doc = load(source);
    expect(displayText(doc)).toBe("a{date}\nb\nc");
    expect(serialize(doc)).toBe(source);
  });

  it("keeps a macro's spacing and quoting as written", () => {
    const source = '{field  name="Bike"\n type=choice options="Road|Gravel" }';
    const doc = load(source);
    expect(doc).toHaveLength(1);
    expect((doc[0] as ChipPiece).macro.attrs).toEqual([
      { name: "name", value: "Bike" },
      { name: "type", value: "choice" },
      { name: "options", value: "Road|Gravel" },
    ]);
    expect(serialize(doc)).toBe(source);
  });

  it("stores typed text that would read as a macro with {lbrace}", () => {
    let s = stateOf(load("Hi "));
    s = insertText(s, "{");
    s = insertText(s, "date}");
    expect(displayText(s.doc)).toBe("Hi {date}");
    expect(serialize(s.doc)).toBe("Hi {lbrace}date}");
    // JSON and code stay as they are.
    s = insertText(stateOf(load("")), '{"a": 1} {name}');
    expect(serialize(s.doc)).toBe('{"a": 1} {name}');
  });

  it("escapes an existing literal brace only when an edit next to it would make it a macro", () => {
    let s = stateOf(load("{dat} {x}"), 4);
    s = insertText(s, "e");
    expect(displayText(s.doc)).toBe("{date} {x}");
    expect(serialize(s.doc)).toBe("{lbrace}date} {x}");
  });

  it("leaves raw text alone: a broken template is kept as written around an edit", () => {
    const source = "Hi {date fmt=";
    const s = insertText(stateOf(load(source)), " there");
    expect(serialize(s.doc)).toBe(`${source} there`);
  });

  it("writes new macros with bare or quoted values", () => {
    expect(quoteValue("enter")).toBe("enter");
    expect(quoteValue("%H:%M")).toBe("%H:%M");
    expect(quoteValue("First name")).toBe('"First name"');
    expect(quoteValue('say "hi" \\ bye')).toBe('"say \\"hi\\" \\\\ bye"');
    expect(quoteValue("")).toBe('""');
    const source = macroSource({
      name: "field",
      attrs: [
        { name: "name", value: 'A "q"' },
        { name: "default", value: "x\\y" },
      ],
    });
    expect(referenceTokens(source)[0]?.attributes).toEqual([
      { name: "name", value: 'A "q"' },
      { name: "default", value: "x\\y" },
    ]);
  });
});

describe("chips are one character", () => {
  const source = "a{cursor}b";
  it("for the arrow keys", () => {
    let s: EditorState = stateOf(load(source), 1);
    s = moveCaret(s, 1, false);
    expect(s.focus).toBe(2);
    s = moveCaret(s, -1, true);
    expect([s.anchor, s.focus]).toEqual([2, 1]);
    expect(selectedChip(s)).toBe(1);
  });

  it("for Backspace and Delete", () => {
    expect(serialize(deleteBackward(stateOf(load(source), 2)).doc)).toBe("ab");
    expect(serialize(deleteForward(stateOf(load(source), 1)).doc)).toBe("ab");
    // A grapheme, not a code unit.
    expect(serialize(deleteBackward(stateOf(load("x👍🏽")), "char").doc)).toBe("x");
    // A CRLF line ending is one line break.
    expect(serialize(deleteBackward(stateOf(load("a\r\nb"), 2)).doc)).toBe("ab");
  });

  it("for words and lines", () => {
    const doc = load("one two{cursor}\nthree four");
    expect(serialize(deleteBackward(stateOf(doc, 8), "word").doc)).toBe("one two\nthree four");
    expect(serialize(deleteBackward(stateOf(doc, 7), "word").doc)).toBe("one {cursor}\nthree four");
    expect(serialize(deleteBackward(stateOf(doc), "line").doc)).toBe("one two{cursor}\n");
    expect(serialize(deleteForward(stateOf(doc, 0), "word").doc)).toBe(" two{cursor}\nthree four");
  });
});

describe("chips", () => {
  it("are inserted, changed and removed; an unchanged chip keeps its source", () => {
    let s = stateOf(load("Hi "));
    s = insertPieces(s, [chip({ name: "field", attrs: [{ name: "name", value: "First name" }] })]);
    expect(serialize(s.doc)).toBe('Hi {field name="First name"}');
    const doc = load('{date  fmt="%Y"}');
    expect(updateChip(doc, 0, { name: "date", attrs: [{ name: "fmt", value: "%Y" }] })).toBe(doc);
    const changed = updateChip(doc, 0, {
      name: "date",
      attrs: [
        { name: "fmt", value: "%Y" },
        { name: "shift", value: "+3bd" },
      ],
    });
    expect(serialize(changed)).toBe("{date fmt=%Y shift=+3bd}");
  });

  it("an optional part is found and removed with its else and end", () => {
    const doc = load("{if field=A}a{if field=B}b{endif}{else}c{endif}d");
    expect(optionalPartOf(doc, 0)).toEqual([5, 7]);
    expect(serialize(removeChips(doc, [0, 5, 7]))).toBe("a{if field=B}b{endif}cd");
    expect(fieldNames(load('{field name=X}{if field=Y}{endif}{field name="X"}'))).toEqual(["X", "Y"]);
  });

  it("copy writes standalone template text; paste reads it", () => {
    const doc = load("a {key name=tab} {dat");
    expect(copySource(doc, 2, 3)).toBe("{key name=tab}");
    // A literal brace that would start a macro once copied alone is escaped.
    const lit = load("{date x");
    expect(copySource(lit, 0, 5)).toBe("{date");
    const typed = insertText(stateOf(load("")), "{date");
    expect(copySource(typed.doc, 0, 5)).toBe("{lbrace}date");
    // Pasted text is template text: its macros are chips, its text literal where it lands.
    const s = insertTemplate(stateOf(load("x ")), "{cursor}{", referenceTokens("{cursor}{"));
    expect(displayText(s.doc)).toBe("x \uFFFC{");
    expect(s.focus).toBe(4);
    const before = insertTemplate(stateOf(load("date}"), 0), "{", referenceTokens("{"));
    expect(serialize(before.doc)).toBe("{lbrace}date}");
    // Pasted text that does not split into tokens is kept as written.
    const broken = insertTemplate(stateOf(load("a")), "{date fmt=", []);
    expect(serialize(broken.doc)).toBe("a{date fmt=");
  });
});

describe("tokens arriving", () => {
  it("are ignored when the document moved on, or when there are none", () => {
    const s = stateOf(load("abc"));
    expect(applyTokens(s, "ab", referenceTokens("ab"))).toBe(s);
    const broken = stateOf(docFromRaw("x {date"));
    expect(applyTokens(broken, "x {date", [])).toBe(broken);
  });

  it("keep the ids of chips that did not move", () => {
    const s = stateOf(load("{cursor} "));
    const id = (s.doc[0] as ChipPiece).id;
    const typed = { ...s, doc: [...s.doc, ...docFromRaw("{clipboard}")] };
    const next = applyTokens(typed, serialize(typed.doc), referenceTokens(serialize(typed.doc)));
    expect((next.doc[0] as ChipPiece).id).toBe(id);
    expect(next.doc.filter((p) => p.kind === "chip")).toHaveLength(2);
  });
});
