// The template editor's document: template text ⇄ a list of pieces (text and chips), and the
// edits the editor makes to it. Pure: no DOM, no service.
//
// How the round trip is kept exact
//
// - Every piece keeps the exact source it came from. Text keeps its characters; a chip keeps
//   the macro as it was written (spacing, quoting, attribute order). Writing the template back
//   is the concatenation of the pieces' sources, so a document built from a template writes
//   back the same bytes, whatever the template holds (template language §2).
// - What the editor shows differently from the source is a piece of its own, one character
//   wide: `{lbrace}` shows as "{", a CRLF or lone CR line ending as a line break. Each keeps its
//   source until the user deletes it.
// - The pieces come from the service's tokens (`ParseTemplate`, ADR-0017): `docFromTokens`. A
//   source the service cannot split into tokens (an unclosed or malformed macro), or one not
//   parsed yet, is shown as *raw* text, kept byte for byte.
// - Only what the user changes is written anew: typed text, a new chip, a chip whose settings
//   changed. Typed text is literal: a "{" the user types that would start a macro where it
//   stands is stored as `{lbrace}` (template language §2.1), and so is an existing literal "{"
//   that an edit next to it would turn into a macro. Raw text is never touched.
//
// Positions in the document ("display positions") count UTF-16 code units of text, and one for
// each chip and each one-character piece, as the editor's DOM does.

import { MACRO_NAMES, type MacroName, type TemplateAttribute, type TemplateToken } from "./api";

/** A macro shown as a chip: every macro but `lbrace`. */
export type ChipName = Exclude<MacroName, "lbrace">;

export interface Macro {
  name: ChipName;
  attrs: TemplateAttribute[];
}

/** Text. `source === text` for ordinary text; otherwise one character shown for a different
 * source (`{lbrace}` → "{", CRLF or CR → "\n"). `raw`: kept as written, never escaped (text the
 * service has not split, or pasted template text waiting for its tokens). */
export interface TextPiece {
  kind: "text";
  text: string;
  source: string;
  raw: boolean;
}

/** A macro. `id` is stable while the chip lives, for the DOM and the settings card. */
export interface ChipPiece {
  kind: "chip";
  id: number;
  macro: Macro;
  source: string;
}

export type Piece = TextPiece | ChipPiece;
export type Doc = readonly Piece[];

let nextId = 1;
function newId(): number {
  return nextId++;
}

const NAMES: ReadonlySet<string> = new Set(MACRO_NAMES);

// ---- Writing macros -----------------------------------------------------------------------

const BARE = /^[A-Za-z0-9_+\-.%:/]+$/;

/** An attribute value as the template language writes it: bare when it can be, else quoted
 * with `\"` and `\\` (template language §2.2). */
export function quoteValue(value: string): string {
  if (BARE.test(value)) return value;
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** The source of a macro. */
export function macroSource(macro: Macro): string {
  const attrs = macro.attrs.map((a) => ` ${a.name}=${quoteValue(a.value)}`).join("");
  return `{${macro.name}${attrs}}`;
}

/** True when the "{" at `i` in `source` starts a macro (template language §2.1): a macro name
 * follows, then "}", white space or the end. CR counts as white space (it becomes LF). */
export function startsMacro(source: string, i: number): boolean {
  let j = i + 1;
  while (j < source.length) {
    const c = source.charCodeAt(j);
    if (c < 97 || c > 122) break;
    j++;
  }
  if (!NAMES.has(source.slice(i + 1, j))) return false;
  if (j >= source.length) return true;
  const c = source[j];
  return c === "}" || c === " " || c === "\t" || c === "\n" || c === "\r";
}

export function attr(macro: Macro, name: string): string | undefined {
  return macro.attrs.find((a) => a.name === name)?.value;
}

function sameAttrs(a: readonly TemplateAttribute[], b: readonly TemplateAttribute[]): boolean {
  return a.length === b.length && a.every((x, i) => x.name === b[i]?.name && x.value === b[i]?.value);
}

// ---- Pieces --------------------------------------------------------------------------------

const LBRACE_SOURCE = "{lbrace}";

export function pieceLength(p: Piece): number {
  return p.kind === "chip" ? 1 : p.text.length;
}

export function isPlain(p: Piece): p is TextPiece {
  return p.kind === "text" && p.source === p.text;
}

/** A new chip for `macro`, with its source written. */
export function chip(macro: Macro): ChipPiece {
  return { kind: "chip", id: newId(), macro, source: macroSource(macro) };
}

/** Text as pieces: line endings other than LF become one-character pieces that keep them. */
export function textPieces(text: string, raw: boolean): Piece[] {
  const out: Piece[] = [];
  let run = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c !== "\r") {
      run += c;
      continue;
    }
    if (run) out.push({ kind: "text", text: run, source: run, raw });
    run = "";
    if (text[i + 1] === "\n") {
      out.push({ kind: "text", text: "\n", source: "\r\n", raw });
      i++;
    } else {
      out.push({ kind: "text", text: "\n", source: "\r", raw });
    }
  }
  if (run) out.push({ kind: "text", text: run, source: run, raw });
  return out;
}

/** Drops empty text and merges neighbouring ordinary text of the same kind. */
function tidy(pieces: readonly Piece[]): Piece[] {
  const out: Piece[] = [];
  for (const p of pieces) {
    if (p.kind === "text" && p.text === "" && p.source === "") continue;
    const last = out[out.length - 1];
    if (last && isPlain(last) && isPlain(p) && last.raw === p.raw) {
      out[out.length - 1] = { ...last, text: last.text + p.text, source: last.source + p.text };
    } else {
      out.push(p);
    }
  }
  return out;
}

/** Stores literal braces that would start a macro where they stand as `{lbrace}` (module
 * docs). Raw text and existing one-character pieces are left alone. */
function escapeBraces(pieces: readonly Piece[]): Piece[] {
  const source = pieces.map((p) => p.source).join("");
  let at = 0;
  let changed = false;
  const out: Piece[] = [];
  for (const p of pieces) {
    if (!isPlain(p) || p.raw || !p.text.includes("{")) {
      out.push(p);
      at += p.source.length;
      continue;
    }
    let run = "";
    for (let i = 0; i < p.text.length; i++) {
      const c = p.text[i] as string;
      if (c === "{" && startsMacro(source, at + i)) {
        if (run) out.push({ kind: "text", text: run, source: run, raw: false });
        run = "";
        out.push({ kind: "text", text: "{", source: LBRACE_SOURCE, raw: false });
        changed = true;
      } else {
        run += c;
      }
    }
    if (run) out.push({ kind: "text", text: run, source: run, raw: false });
    at += p.source.length;
  }
  return changed ? out : [...pieces];
}

function settle(pieces: readonly Piece[]): Doc {
  return tidy(escapeBraces(tidy(pieces)));
}

// ---- Template text ⇄ document --------------------------------------------------------------

/** The template text of `doc`. */
export function serialize(doc: Doc): string {
  let out = "";
  for (const p of doc) out += p.source;
  return out;
}

/** `source` as raw text: kept byte for byte, before (or without) the service's tokens. */
export function docFromRaw(source: string): Doc {
  return tidy(textPieces(source, true));
}

/** True when `tokens` cover `source` from start to end, one after the other. */
export function tokensTile(source: string, tokens: readonly TemplateToken[]): boolean {
  let at = 0;
  for (const t of tokens) {
    if (t.start !== at || t.end < t.start) return false;
    at = t.end;
  }
  return at === source.length;
}

/** The document of `source` from the service's tokens for it. Without tokens (a source the
 * service cannot split), or with tokens that do not fit the source, the source is raw text. */
export function docFromTokens(source: string, tokens: readonly TemplateToken[]): Doc {
  if (tokens.length === 0 || !tokensTile(source, tokens)) return docFromRaw(source);
  const pieces: Piece[] = [];
  for (const t of tokens) {
    const slice = source.slice(t.start, t.end);
    if (t.kind === "text") {
      pieces.push(...textPieces(slice, false));
    } else if (t.kind === "lbrace") {
      pieces.push({ kind: "text", text: "{", source: slice, raw: false });
    } else {
      pieces.push({
        kind: "chip",
        id: newId(),
        macro: { name: t.kind, attrs: t.attributes.map((a) => ({ ...a })) },
        source: slice,
      });
    }
  }
  return tidy(pieces);
}

/** What the editor shows: text, with U+FFFC for each chip. */
export function displayText(doc: Doc): string {
  return doc.map((p) => (p.kind === "chip" ? "\uFFFC" : p.text)).join("");
}

export function docLength(doc: Doc): number {
  let n = 0;
  for (const p of doc) n += pieceLength(p);
  return n;
}

/** The source offset of display position `pos`. */
export function sourceOffset(doc: Doc, pos: number): number {
  let d = 0;
  let s = 0;
  for (const p of doc) {
    const len = pieceLength(p);
    if (pos < d + len) return s + (isPlain(p) ? pos - d : 0);
    d += len;
    s += p.source.length;
  }
  return s;
}

/** The display position of source offset `offset`; inside a chip or a one-character piece,
 * the position after it (`before`: before it). */
export function displayOffset(doc: Doc, offset: number, before = false): number {
  let d = 0;
  let s = 0;
  for (const p of doc) {
    const end = s + p.source.length;
    if (offset < end) {
      if (offset <= s) return d;
      return isPlain(p) ? d + (offset - s) : before ? d : d + pieceLength(p);
    }
    d += pieceLength(p);
    s = end;
  }
  return d;
}

/** The piece at display position `pos` (the one that starts at or contains it) and where. */
export function locate(doc: Doc, pos: number): { index: number; start: number } {
  let d = 0;
  for (let i = 0; i < doc.length; i++) {
    const len = pieceLength(doc[i] as Piece);
    if (pos < d + len) return { index: i, start: d };
    d += len;
  }
  return { index: doc.length, start: d };
}

/** The display position where piece `index` starts. */
export function pieceStart(doc: Doc, index: number): number {
  let d = 0;
  for (let i = 0; i < index && i < doc.length; i++) d += pieceLength(doc[i] as Piece);
  return d;
}

function split(doc: Doc, pos: number): [Piece[], Piece[]] {
  const { index, start } = locate(doc, pos);
  const left = doc.slice(0, index);
  const right = doc.slice(index);
  const p = doc[index];
  if (p && isPlain(p) && pos > start) {
    const k = pos - start;
    left.push({ ...p, text: p.text.slice(0, k), source: p.source.slice(0, k) });
    right[0] = { ...p, text: p.text.slice(k), source: p.source.slice(k) };
  }
  return [left, right];
}

/** The pieces between two display positions. */
export function slice(doc: Doc, from: number, to: number): Doc {
  const [, right] = split(doc, from);
  const [mid] = split(right, to - from);
  return tidy(mid);
}

/** `doc` with display positions `from`…`to` replaced by `insert`. */
export function replace(doc: Doc, from: number, to: number, insert: readonly Piece[]): Doc {
  const [left] = split(doc, from);
  const [, right] = split(doc, to);
  return settle([...left, ...insert, ...right]);
}

/** The template text of a part of the document, as copied to the clipboard: standalone
 * (literal braces escaped for where they now stand). */
export function copySource(doc: Doc, from: number, to: number): string {
  return serialize(settle(slice(doc, Math.min(from, to), Math.max(from, to))));
}

/** True when two documents show and write the same thing (chip ids aside). */
export function sameDoc(a: Doc, b: Doc): boolean {
  if (a.length !== b.length) return false;
  return a.every((p, i) => {
    const q = b[i] as Piece;
    if (p.kind === "text") {
      return q.kind === "text" && p.text === q.text && p.source === q.source && p.raw === q.raw;
    }
    return (
      q.kind === "chip" &&
      p.source === q.source &&
      p.macro.name === q.macro.name &&
      sameAttrs(p.macro.attrs, q.macro.attrs)
    );
  });
}

// ---- Editor state and edits ----------------------------------------------------------------

/** The document with its selection (display positions; `anchor === focus` is a caret). */
export interface EditorState {
  doc: Doc;
  anchor: number;
  focus: number;
}

export function stateOf(doc: Doc, caret = docLength(doc)): EditorState {
  return { doc, anchor: caret, focus: caret };
}

export function selectionRange(s: EditorState): [number, number] {
  return s.anchor <= s.focus ? [s.anchor, s.focus] : [s.focus, s.anchor];
}

export function isCollapsed(s: EditorState): boolean {
  return s.anchor === s.focus;
}

/** Replaces the selection with `pieces`; the caret goes after them. */
export function insertPieces(s: EditorState, pieces: readonly Piece[]): EditorState {
  const [from, to] = selectionRange(s);
  const before = docLength(s.doc) - to;
  const doc = replace(s.doc, from, to, pieces);
  const caret = docLength(doc) - before;
  return { doc, anchor: caret, focus: caret };
}

/** Typed text: literal. */
export function insertText(s: EditorState, text: string): EditorState {
  return insertPieces(s, textPieces(text.replace(/\r\n?/g, "\n"), false));
}

/** Pasted template text, with the service's tokens for it alone: its macros become chips and
 * its text is literal where it lands. Without tokens (text that does not split into tokens, or
 * no service), it is raw: kept as written, and the template's error says what is wrong. */
export function insertTemplate(
  s: EditorState,
  source: string,
  tokens: readonly TemplateToken[],
): EditorState {
  const pieces =
    tokens.length > 0 && tokensTile(source, tokens) ? docFromTokens(source, tokens) : textPieces(source, true);
  return insertPieces(s, pieces);
}

const segmenter: { segment(t: string): Iterable<{ index: number }> } | null =
  typeof Intl !== "undefined" && "Segmenter" in Intl
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;

/** Offsets of the grapheme boundaries of `text`, 0 and its length included. */
function boundaries(text: string): number[] {
  if (segmenter) {
    const out = [...segmenter.segment(text)].map((g) => g.index);
    out.push(text.length);
    return out;
  }
  const out: number[] = [];
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (!(c >= 0xdc00 && c <= 0xdfff && i > 0)) out.push(i);
  }
  out.push(text.length);
  return out;
}

/** The position one character (a grapheme, a chip, a line ending) before `pos`. */
export function prevPosition(doc: Doc, pos: number): number {
  if (pos <= 0) return 0;
  const { index, start } = locate(doc, pos - 1);
  const p = doc[index];
  if (!p || !isPlain(p)) return pos - 1;
  const k = pos - start;
  const b = boundaries(p.text);
  let prev = 0;
  for (const x of b) {
    if (x >= k) break;
    prev = x;
  }
  return start + prev;
}

/** The position one character after `pos`. */
export function nextPosition(doc: Doc, pos: number): number {
  const len = docLength(doc);
  if (pos >= len) return len;
  const { index, start } = locate(doc, pos);
  const p = doc[index];
  if (!p || !isPlain(p)) return pos + 1;
  const k = pos - start;
  for (const x of boundaries(p.text)) if (x > k) return start + x;
  return start + p.text.length;
}

/** The start of the word before `pos`; a chip is a word of its own. */
export function prevWord(doc: Doc, pos: number): number {
  const t = displayText(doc);
  let i = pos;
  while (i > 0 && /\s/.test(t[i - 1] as string) && t[i - 1] !== "\uFFFC") i--;
  if (i > 0 && t[i - 1] === "\uFFFC") return i === pos ? i - 1 : i;
  while (i > 0 && !/\s/.test(t[i - 1] as string) && t[i - 1] !== "\uFFFC") i--;
  return i;
}

/** The end of the word after `pos`. */
export function nextWord(doc: Doc, pos: number): number {
  const t = displayText(doc);
  let i = pos;
  while (i < t.length && /\s/.test(t[i] as string)) i++;
  if (i < t.length && t[i] === "\uFFFC") return i === pos ? i + 1 : i;
  while (i < t.length && !/\s/.test(t[i] as string) && t[i] !== "\uFFFC") i++;
  return i;
}

/** The start of the line `pos` is on. */
export function lineStart(doc: Doc, pos: number): number {
  const t = displayText(doc);
  const nl = t.lastIndexOf("\n", pos - 1);
  return nl < 0 ? 0 : nl + 1;
}

/** The end of the line `pos` is on. */
export function lineEnd(doc: Doc, pos: number): number {
  const t = displayText(doc);
  const nl = t.indexOf("\n", pos);
  return nl < 0 ? t.length : nl;
}

/** Deletes the range `from`…`to`; the caret goes to `from`. */
export function deleteRange(s: EditorState, from: number, to: number): EditorState {
  const a = Math.min(from, to);
  const b = Math.max(from, to);
  const doc = a === b ? s.doc : replace(s.doc, a, b, []);
  // An escape may have been added before the caret (a literal "{" now followed by a name).
  const caret = Math.min(a, docLength(doc));
  return { doc, anchor: caret, focus: caret };
}

export type DeleteUnit = "char" | "word" | "line";

/** Backspace (and its word and line forms): the selection, or the unit before the caret. */
export function deleteBackward(s: EditorState, unit: DeleteUnit = "char"): EditorState {
  if (!isCollapsed(s)) return deleteRange(s, s.anchor, s.focus);
  const pos = s.focus;
  const from =
    unit === "char" ? prevPosition(s.doc, pos) : unit === "word" ? prevWord(s.doc, pos) : lineStart(s.doc, pos);
  return deleteRange(s, from === pos && unit === "line" ? prevPosition(s.doc, pos) : from, pos);
}

/** Delete (and its word and line forms): the selection, or the unit after the caret. */
export function deleteForward(s: EditorState, unit: DeleteUnit = "char"): EditorState {
  if (!isCollapsed(s)) return deleteRange(s, s.anchor, s.focus);
  const pos = s.focus;
  const to =
    unit === "char" ? nextPosition(s.doc, pos) : unit === "word" ? nextWord(s.doc, pos) : lineEnd(s.doc, pos);
  return deleteRange(s, pos, to === pos && unit === "line" ? nextPosition(s.doc, pos) : to);
}

/** ← and → (a chip is one character); with `extend`, Shift: the selection grows or shrinks. */
export function moveCaret(s: EditorState, dir: -1 | 1, extend: boolean): EditorState {
  if (!extend && !isCollapsed(s)) {
    const [from, to] = selectionRange(s);
    const caret = dir < 0 ? from : to;
    return { ...s, anchor: caret, focus: caret };
  }
  const focus = dir < 0 ? prevPosition(s.doc, s.focus) : nextPosition(s.doc, s.focus);
  return { ...s, anchor: extend ? s.anchor : focus, focus };
}

/** The index of the chip the selection covers exactly, if it covers one chip. */
export function selectedChip(s: EditorState): number | null {
  const [from, to] = selectionRange(s);
  if (to - from !== 1) return null;
  const { index } = locate(s.doc, from);
  return s.doc[index]?.kind === "chip" ? index : null;
}

/** Replaces chip `index`'s macro. The source is written anew only if the attributes changed. */
export function updateChip(doc: Doc, index: number, macro: Macro): Doc {
  const p = doc[index];
  if (!p || p.kind !== "chip") return doc;
  if (p.macro.name === macro.name && sameAttrs(p.macro.attrs, macro.attrs)) return doc;
  const next: ChipPiece = { kind: "chip", id: p.id, macro, source: macroSource(macro) };
  return settle([...doc.slice(0, index), next, ...doc.slice(index + 1)]);
}

/** The chips that belong to the optional part chip `index` opens: its `else` and `endif`. */
export function optionalPartOf(doc: Doc, index: number): number[] {
  const open = doc[index];
  if (!open || open.kind !== "chip" || open.macro.name !== "if") return [];
  const out: number[] = [];
  let depth = 0;
  for (let i = index + 1; i < doc.length; i++) {
    const p = doc[i] as Piece;
    if (p.kind !== "chip") continue;
    if (p.macro.name === "if") depth++;
    else if (p.macro.name === "endif") {
      if (depth === 0) {
        out.push(i);
        return out;
      }
      depth--;
    } else if (p.macro.name === "else" && depth === 0) out.push(i);
  }
  return out;
}

/** Removes the chips at `indices` (text between them stays). */
export function removeChips(doc: Doc, indices: readonly number[]): Doc {
  const drop = new Set(indices);
  return settle(doc.filter((_, i) => !drop.has(i)));
}

/** The names of the fill-in fields the document defines or looks at, in order. */
export function fieldNames(doc: Doc): string[] {
  const out: string[] = [];
  for (const p of doc) {
    if (p.kind !== "chip") continue;
    const name = p.macro.name === "field" ? attr(p.macro, "name") : p.macro.name === "if" ? attr(p.macro, "field") : undefined;
    if (name && !out.includes(name)) out.push(name);
  }
  return out;
}

/**
 * The document after the service's tokens for `source` arrived: the chips are drawn from them.
 * Nothing changes when `source` is no longer the document's (an answer overtaken by an edit),
 * when there are no tokens (the document keeps what it shows; the error says what is wrong),
 * or when the tokens describe what the document already shows. The selection keeps its place
 * in the source, and chips that did not move keep their ids.
 */
export function applyTokens(
  s: EditorState,
  source: string,
  tokens: readonly TemplateToken[],
): EditorState {
  if (serialize(s.doc) !== source) return s;
  if (tokens.length === 0 && source !== "") return s;
  const fresh = docFromTokens(source, tokens);
  if (sameDoc(fresh, s.doc)) return s;
  const ids = new Map<string, number>();
  let at = 0;
  for (const p of s.doc) {
    if (p.kind === "chip") ids.set(`${at}:${p.source}`, p.id);
    at += p.source.length;
  }
  at = 0;
  const doc = fresh.map((p) => {
    const start = at;
    at += p.source.length;
    const id = p.kind === "chip" ? ids.get(`${start}:${p.source}`) : undefined;
    return p.kind === "chip" && id !== undefined ? { ...p, id } : p;
  });
  const anchor = displayOffset(doc, sourceOffset(s.doc, s.anchor));
  const focus = displayOffset(doc, sourceOffset(s.doc, s.focus));
  return { doc, anchor, focus };
}
