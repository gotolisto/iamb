// The editor's text box in the DOM: drawing the document, and translating between DOM
// selection points and display positions.
//
// The text box is a `contenteditable` element whose content the editor draws itself, from the
// document, after every change (no React inside it: the browser and React would both own the
// same nodes). The browser never edits it: every input is turned into an edit of the document
// (`TemplateEditor.tsx`), except text being composed by an input method, which is read back
// when the composition ends.
//
// Each piece is one element with `data-p` (its index): a text piece is a span holding its text,
// a chip a non-editable span with its icon and label. A `<br>` always ends the content so that a
// final line break shows and the caret has a place after the last chip.

import { iconElement } from "./icons";
import { chipGroup, chipIcon, chipLabel, chipSpokenName } from "./macros";
import { docLength, pieceLength, type Doc } from "./model";

export interface DrawOptions {
  /** The chip shown as selected (the selection covers it exactly). */
  selectedChip: number | null;
  /** Display range `[from, to)` of the error, if it is in this document. */
  error: [number, number] | null;
}

/** Draws `doc` into `root`, replacing what was there. */
export function draw(root: HTMLElement, doc: Doc, opts: DrawOptions): void {
  const d = root.ownerDocument;
  const nodes: Node[] = [];
  let at = 0;
  doc.forEach((p, i) => {
    const start = at;
    const end = at + pieceLength(p);
    at = end;
    const span = d.createElement("span");
    span.dataset.p = String(i);
    const inError = opts.error !== null && start < opts.error[1] && end > opts.error[0];
    if (p.kind === "chip") {
      span.contentEditable = "false";
      span.className = `tpl-chip ${chipGroup(p.macro.name)}`;
      if (opts.selectedChip === i) span.classList.add("is-selected");
      if (inError) span.classList.add("is-error");
      span.dataset.chip = String(p.id);
      span.setAttribute("role", "img");
      span.setAttribute("aria-label", chipSpokenName(p.macro));
      span.append(iconElement(d, chipIcon(p.macro)), d.createTextNode(chipLabel(p.macro)));
    } else if (inError && opts.error) {
      // The faulty part of text that does not split into tokens is underlined.
      const a = Math.max(0, opts.error[0] - start);
      const b = Math.min(p.text.length, opts.error[1] - start);
      if (a > 0) span.append(d.createTextNode(p.text.slice(0, a)));
      const mark = d.createElement("span");
      mark.className = "tpl-error-text";
      mark.append(d.createTextNode(p.text.slice(a, b)));
      span.append(mark);
      if (b < p.text.length) span.append(d.createTextNode(p.text.slice(b)));
    } else {
      span.append(d.createTextNode(p.text));
    }
    nodes.push(span);
  });
  const br = d.createElement("br");
  br.dataset.end = "";
  nodes.push(br);
  root.replaceChildren(...nodes);
}

function pieceStarts(doc: Doc): number[] {
  const out: number[] = [];
  let at = 0;
  for (const p of doc) {
    out.push(at);
    at += pieceLength(p);
  }
  out.push(at);
  return out;
}

/** Text length inside `el` before the point (`node`, `offset`). */
function textBefore(el: Node, node: Node, offset: number): number {
  const range = el.ownerDocument?.createRange();
  if (!range) return 0;
  range.setStart(el, 0);
  range.setEnd(node, offset);
  return range.toString().length;
}

/** The display position of a DOM point inside `root`, or null when it is outside. */
export function positionOf(root: HTMLElement, doc: Doc, node: Node | null, offset: number): number | null {
  if (!node || !root.contains(node)) return null;
  const starts = pieceStarts(doc);
  const end = docLength(doc);
  if (node === root) {
    const child = root.childNodes[offset];
    if (!child || !(child instanceof HTMLElement) || child.dataset.p === undefined) return end;
    return starts[Number(child.dataset.p)] ?? end;
  }
  let el: HTMLElement | null = node instanceof HTMLElement ? node : node.parentElement;
  while (el && el !== root && el.dataset.p === undefined) el = el.parentElement;
  if (!el || el === root) return end;
  const index = Number(el.dataset.p);
  const start = starts[index] ?? end;
  const piece = doc[index];
  if (!piece) return end;
  if (piece.kind === "chip") return node === el && offset === 0 ? start : start + 1;
  return start + Math.min(pieceLength(piece), textBefore(el, node, offset));
}

/** The DOM point of display position `pos`. Inside text where there is text; otherwise between
 * the root's children. */
export function pointOf(root: HTMLElement, doc: Doc, pos: number): [Node, number] {
  const starts = pieceStarts(doc);
  for (let i = 0; i < doc.length; i++) {
    const p = doc[i];
    const start = starts[i] as number;
    const end = starts[i + 1] as number;
    if (!p || pos > end) continue;
    if (p.kind === "text" && pos >= start && pos <= end) {
      // At a boundary between text and a chip, stay in the text.
      return textPoint(root, i, pos - start);
    }
    if (pos <= start) return [root, i];
  }
  return [root, doc.length];
}

function textPoint(root: HTMLElement, index: number, offset: number): [Node, number] {
  const el = root.querySelector<HTMLElement>(`[data-p="${index}"]`);
  if (!el) return [root, index];
  const walker = root.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let left = offset;
  let last: Node | null = null;
  for (let t = walker.nextNode(); t; t = walker.nextNode()) {
    const len = t.textContent?.length ?? 0;
    if (left <= len) return [t, left];
    left -= len;
    last = t;
  }
  return last ? [last, last.textContent?.length ?? 0] : [el, 0];
}

/** Puts the DOM selection at display positions `anchor` and `focus`. */
export function select(root: HTMLElement, doc: Doc, anchor: number, focus: number): void {
  const sel = root.ownerDocument.getSelection();
  if (!sel) return;
  const [an, ao] = pointOf(root, doc, anchor);
  const [fn, fo] = pointOf(root, doc, focus);
  if (
    sel.anchorNode === an &&
    sel.anchorOffset === ao &&
    sel.focusNode === fn &&
    sel.focusOffset === fo
  ) {
    return;
  }
  sel.setBaseAndExtent(an, ao, fn, fo);
}

/** The display positions of the DOM selection, if it is inside `root`. */
export function selectionIn(root: HTMLElement, doc: Doc): { anchor: number; focus: number } | null {
  const sel = root.ownerDocument.getSelection();
  if (!sel || sel.rangeCount === 0) return null;
  const anchor = positionOf(root, doc, sel.anchorNode, sel.anchorOffset);
  const focus = positionOf(root, doc, sel.focusNode, sel.focusOffset);
  if (anchor === null || focus === null) return null;
  return { anchor, focus };
}

/** The chip element of piece `index`. */
export function chipElement(root: HTMLElement, index: number): HTMLElement | null {
  return root.querySelector<HTMLElement>(`[data-p="${index}"]`);
}
