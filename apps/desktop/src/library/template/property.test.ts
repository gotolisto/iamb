// Random edits, as a user makes them in the editor, on the examples of the template language.
// After every edit the template must still split into tokens, and mean exactly what the editor
// shows: the same text, the same chips (the service's tokenizer, transcribed in
// `reference-tokenizer.ts` and checked against the service on every example, is the oracle).
// And an edit must leave the template text outside the edited part as it was.

import { describe, expect, it } from "vitest";

import type { TemplateAttribute, TemplateToken } from "./api";
import {
  applyTokens,
  chip,
  copySource,
  deleteBackward,
  deleteForward,
  docFromTokens,
  docLength,
  insertPieces,
  insertTemplate,
  insertText,
  moveCaret,
  sameDoc,
  serialize,
  sourceOffset,
  type ChipName,
  type DeleteUnit,
  type Doc,
  type EditorState,
  updateChip,
} from "./model";
import { referenceTokens } from "./reference-tokenizer";
import fixtures from "./spec.fixtures.json";

/** A small, seeded generator (mulberry32), so a failure can be replayed. */
function rng(seed: number) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const int = (n: number) => Math.floor(next() * n);
  const pick = <T>(list: readonly T[]): T => list[int(list.length)] as T;
  return { next, int, pick };
}

type Rng = ReturnType<typeof rng>;

const BITS = [
  "a", "Z", " ", "\n", "{", "}", "=", '"', "\\", "|", "é", "👍", "%Y",
  "date", "time", "field", "lbrace", "endif", "cursor", "if ", "{date", "{field name=x}", "{lbrace}",
];

function text(r: Rng): string {
  let out = "";
  for (let i = r.int(4) + 1; i > 0; i--) out += r.pick(BITS);
  return out;
}

function value(r: Rng): string {
  return r.next() < 0.5 ? r.pick(["x", "enter", "%H:%M", "+3bd", "First name"]) : text(r);
}

function macro(r: Rng): { name: ChipName; attrs: TemplateAttribute[] } {
  const name = r.pick<ChipName>(["date", "time", "clipboard", "cursor", "snippet", "field", "key", "wait", "if", "else", "endif"]);
  const allowed: Record<ChipName, string[]> = {
    date: ["fmt", "shift"],
    time: ["fmt", "shift"],
    clipboard: ["trim"],
    snippet: ["abbr"],
    field: ["name", "type", "default", "options", "on", "off"],
    key: ["name", "shift", "times"],
    wait: ["ms"],
    if: ["field", "is", "not"],
    cursor: [],
    else: [],
    endif: [],
  };
  const attrs = allowed[name].filter(() => r.next() < 0.5).map((n) => ({ name: n, value: value(r) }));
  return { name, attrs };
}

/** The service's answer for `source`, through the oracle. */
function tokens(source: string): TemplateToken[] {
  return referenceTokens(source);
}

function select(r: Rng, s: EditorState): EditorState {
  const len = docLength(s.doc);
  const anchor = r.int(len + 1);
  const focus = r.next() < 0.6 ? anchor : r.int(len + 1);
  return { ...s, anchor, focus };
}

function edit(r: Rng, s: EditorState): { next: EditorState; from: number; to: number; touchesAll?: boolean } {
  s = select(r, s);
  const from = Math.min(s.anchor, s.focus);
  const to = Math.max(s.anchor, s.focus);
  const choice = r.int(9);
  switch (choice) {
    case 0:
    case 1:
      return { next: insertText(s, text(r)), from, to };
    case 2:
      return { next: deleteBackward(s, r.pick<DeleteUnit>(["char", "char", "word", "line"])), from: Math.max(0, from - 64), to };
    case 3:
      return { next: deleteForward(s, r.pick<DeleteUnit>(["char", "char", "word", "line"])), from, to: Math.max(to, from) + 64 };
    case 4:
      return { next: insertPieces(s, [chip(macro(r))]), from, to };
    case 5: {
      const chips = s.doc.flatMap((p, i) => (p.kind === "chip" ? [i] : []));
      if (chips.length === 0) return { next: s, from, to };
      const index = r.pick(chips);
      return { next: { ...s, doc: updateChip(s.doc, index, macro(r)) }, from: 0, to: docLength(s.doc), touchesAll: true };
    }
    case 6: {
      // Copy a part, paste it somewhere (as the editor does: with the tokens of the copy).
      const len = docLength(s.doc);
      const a = r.int(len + 1);
      const b = r.int(len + 1);
      const copied = copySource(s.doc, a, b);
      return { next: insertTemplate(s, copied, tokens(copied)), from, to };
    }
    case 7:
      return { next: moveCaret(s, r.next() < 0.5 ? -1 : 1, r.next() < 0.5), from: 0, to: 0 };
    default: {
      // The service's answer for the current text arrives.
      const source = serialize(s.doc);
      return { next: applyTokens(s, source, tokens(source)), from: 0, to: 0 };
    }
  }
}

function commonPrefix(a: string, b: string): number {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  return i;
}

function commonSuffix(a: string, b: string): number {
  let i = 0;
  while (i < a.length && i < b.length && a[a.length - 1 - i] === b[b.length - 1 - i]) i++;
  return i;
}

/** The longest macro name: how far before an edit a literal "{" can be escaped by it. */
const ESCAPE_REACH = "{clipboard".length;

function check(doc: Doc, seed: number, step: number) {
  const source = serialize(doc);
  const toks = tokens(source);
  const where = `seed ${seed}, step ${step}: ${JSON.stringify(source)}`;
  // Well formed: it splits into tokens.
  expect(toks.length > 0 || source === "", where).toBe(true);
  // What the editor shows is what the template means.
  expect(sameDoc(docFromTokens(source, toks), doc), where).toBe(true);
}

const starts = (fixtures.examples as Array<{ template: string; tokens: TemplateToken[] }>).filter(
  (e) => e.tokens.length > 0,
);

describe("random edits", () => {
  it("keep every example well formed and meaning what the editor shows", () => {
    const runs = 400;
    for (let seed = 1; seed <= runs; seed++) {
      const r = rng(seed);
      const start = r.pick(starts);
      let s: EditorState = { doc: docFromTokens(start.template, start.tokens), anchor: 0, focus: 0 };
      check(s.doc, seed, 0);
      for (let step = 1; step <= 30; step++) {
        const before = serialize(s.doc);
        const { next, from, to, touchesAll } = edit(r, s);
        const after = serialize(next.doc);
        if (!touchesAll && before !== after) {
          // Outside the edited part, the template text is unchanged.
          const srcFrom = sourceOffset(s.doc, Math.min(from, docLength(s.doc)));
          const srcTo = sourceOffset(s.doc, Math.min(to, docLength(s.doc)));
          expect(commonPrefix(before, after), `seed ${seed}, step ${step}`).toBeGreaterThanOrEqual(
            Math.max(0, srcFrom - ESCAPE_REACH),
          );
          expect(commonSuffix(before, after), `seed ${seed}, step ${step}`).toBeGreaterThanOrEqual(
            Math.min(before.length - srcTo, after.length) - 0,
          );
        }
        s = next;
        check(s.doc, seed, step);
      }
    }
  });
});
