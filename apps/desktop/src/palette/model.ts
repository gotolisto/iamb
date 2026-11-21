// The search palette's rules, without React: which rows there are, what Enter does for each,
// how matches are marked, and the copy (docs/ux/spec.md §4.5, §5.5; flows/5-search.md).

import { asksForValues, isTemplateError, lineShowsField, previewLine, rowTitle } from "../library/model";
import type { TemplatePreview } from "../library/template/api";
import type { PaletteHit, PaletteResults, PickAction } from "./api";

/** The system whose conventions the panel follows (key names). */
export type Os = "mac" | "win";

/** The system the page runs on; Linux (development only) follows Windows. */
export function currentOs(): Os {
  if (typeof navigator === "undefined") return "win";
  const platform = navigator.platform || navigator.userAgent;
  return /Mac|iPhone|iPad/i.test(platform) ? "mac" : "win";
}

/** The most rows the list shows without scrolling (design system §4.14). */
export const VISIBLE_ROWS = 8;

/** A row of the list. */
export type Item =
  | { kind: "again"; label: string }
  | { kind: "hit"; hit: PaletteHit }
  | { kind: "new"; label: string };

/** The answer to a query, with the query it answers. */
export interface Answer {
  query: string;
  results: PaletteResults;
}

/**
 * The rows for what is typed: with nothing typed, "Fill in … again" when there is a form to
 * fill in again, then the snippets used most recently on this computer (`recent`, at most
 * {@link VISIBLE_ROWS} in all); with a query, its hits, or, when nothing matches, the one row
 * that creates a snippet labelled with the query.
 */
export function itemsFor(
  query: string,
  answer: Answer | null,
  lastForm: string | null,
  recent: PaletteResults | null = null,
): Item[] {
  const typed = query.trim();
  if (typed === "") {
    const again: Item[] = lastForm ? [{ kind: "again", label: lastForm }] : [];
    const room = VISIBLE_ROWS - again.length;
    const used: Item[] = (recent?.hits ?? []).slice(0, room).map((hit) => ({ kind: "hit", hit }));
    return [...again, ...used];
  }
  if (!answer) return [];
  // The answer is to an earlier query and a newer one is on its way: the previous rows stay
  // (flow 5, the service that is slow to answer), never its "no match" row.
  if (answer.query.trim() !== typed) {
    return answer.results.hits.map((hit) => ({ kind: "hit", hit }));
  }
  if (answer.results.hits.length === 0) return [{ kind: "new", label: typed }];
  return answer.results.hits.map((hit) => ({ kind: "hit", hit }));
}

/** What Enter does for a row, which decides the footer and the pick. */
export type Verb = "insert" | "fill" | "review" | "open" | "select" | "form" | "create";

export function verbOf(item: Item, ownWindow: boolean): Verb {
  switch (item.kind) {
    case "again":
      return "form";
    case "new":
      return "create";
    case "hit": {
      const row = item.hit.row;
      if (ownWindow) return "select";
      if (isTemplateError(row.broken)) return "open";
      if (row.needsReview) return "review";
      return asksForValues(row) ? "fill" : "insert";
    }
  }
}

/** The pick for Enter on a row. */
export function actionFor(item: Item, ownWindow: boolean): PickAction {
  const verb = verbOf(item, ownWindow);
  switch (item.kind) {
    case "again":
      return { kind: "fill_again" };
    case "new":
      return { kind: "new", label: item.label };
    case "hit":
      return verb === "insert" || verb === "fill"
        ? { kind: "insert", id: item.hit.row.id }
        : { kind: "show", id: item.hit.row.id };
  }
}

/** The pick for ⌘E: the snippet in the library; rows that are not snippets have no such pick. */
export function editAction(item: Item | undefined): PickAction | null {
  return item?.kind === "hit" ? { kind: "show", id: item.hit.row.id } : null;
}

/** A piece of text, marked or not. */
export interface Piece {
  text: string;
  marked: boolean;
}

/**
 * `text` cut at `ranges` (UTF-16 code units, as the service counts them and as JavaScript
 * indexes strings). Ranges that are out of order, overlap or run past the text are tidied.
 */
export function pieces(text: string, ranges: ReadonlyArray<readonly [number, number]>): Piece[] {
  const sorted = [...ranges]
    .map(([a, b]) => [Math.max(0, a), Math.min(text.length, b)] as const)
    .filter(([a, b]) => a < b)
    .sort((x, y) => x[0] - y[0]);
  const out: Piece[] = [];
  let at = 0;
  for (const [a, b] of sorted) {
    const start = Math.max(a, at);
    if (b <= start) continue;
    if (start > at) out.push({ text: text.slice(at, start), marked: false });
    const last = out[out.length - 1];
    if (last?.marked) last.text += text.slice(start, b);
    else out.push({ text: text.slice(start, b), marked: true });
    at = b;
  }
  if (at < text.length) out.push({ text: text.slice(at), marked: false });
  return out.length ? out : [{ text, marked: false }];
}

/** The ranges of `hit` that fall in `field`, or none. */
export function rangesIn(hit: PaletteHit, field: PaletteHit["field"]): Array<[number, number]> {
  return hit.field === field ? hit.ranges : [];
}

/**
 * The tag whose name the hit matched: the first whose name holds the marked text, else the
 * first tag whose name is long enough for the ranges.
 */
export function matchedTag(hit: PaletteHit, query: string): number {
  if (hit.field !== "tag") return -1;
  const q = query.toLocaleLowerCase();
  const names = hit.row.tags;
  const fits = (name: string) => hit.ranges.every(([, b]) => b <= name.length);
  const exact = names.findIndex(
    (n) =>
      fits(n) &&
      hit.ranges.length > 0 &&
      hit.ranges.every(([a, b]) => q.includes(n.slice(a, b).toLocaleLowerCase())),
  );
  return exact >= 0 ? exact : names.findIndex(fits);
}

/** The first line a row shows under its title (the content's first line, blanks by name). */
export function subLine(hit: PaletteHit): string {
  const row = hit.row;
  if (row.needsReview) return "Enter opens the review; it cannot be inserted yet.";
  if (isTemplateError(row.broken)) return "Has a template error. Enter opens it in the library.";
  return previewLine(row.firstLine);
}

/** The row's title: its label, or the first line of the content (the library's rule). */
export const titleOf = (hit: PaletteHit): string => rowTitle(hit.row);

/** The ranges of a body hit that fall inside the first line, which is all a row shows. */
export function bodyRanges(hit: PaletteHit): Array<[number, number]> {
  if (hit.field !== "body" || lineShowsField(hit.row.firstLine)) return [];
  const length = hit.row.firstLine.length;
  return hit.ranges.filter(([a]) => a < length).map(([a, b]) => [a, Math.min(b, length)]);
}

/** The index after moving `delta` rows in a list of `length`, wrapping. */
export function move(index: number, delta: number, length: number): number {
  if (length <= 0) return 0;
  return (((index + delta) % length) + length) % length;
}

/** The index after Page Up / Page Down: a page, but not past either end. */
export function jump(index: number, direction: 1 | -1, length: number): number {
  if (length <= 0) return 0;
  return Math.min(length - 1, Math.max(0, index + direction * VISIBLE_ROWS));
}

/** What the preview strip shows: the text the snippet would type, keys and waits as marks. */
export function previewText(preview: TemplatePreview | null): string {
  if (!preview || preview.error) return "";
  return preview.parts
    .map((p) => {
      if (p.part === "text") return p.text;
      if (p.part === "key") return `⟨${p.key}${p.times > 1 ? ` ×${p.times}` : ""}⟩`;
      // A formatted snippet: the text without its formatting, never its Markdown source.
      if (p.part === "formatted") return p.plain;
      return "";
    })
    .join("");
}

/** Key names as each system writes them (design system §4.7). */
export interface Keys {
  enter: string;
  edit: string;
  close: string;
  /** `aria-keyshortcuts` of the row picks and the edit key. */
  aria: { edit: string; rows: string };
}

export function keysFor(os: Os): Keys {
  return os === "mac"
    ? { enter: "⏎", edit: "⌘E", close: "esc", aria: { edit: "Meta+E", rows: "Meta+1" } }
    : { enter: "Enter", edit: "Ctrl+E", close: "Esc", aria: { edit: "Control+E", rows: "Control+1" } };
}

/** The copy of the panel (spec §4.5, flow 5). One place, for the message catalogue (3.11). */
export const copy = {
  title: "Search snippets",
  placeholder: "Search snippets",
  hint: "Type to search by abbreviation, label, text or tag.",
  again: (label: string) => `Fill in “${label}” again`,
  badgeAgain: "your last answers",
  noMatch: (query: string) => `No snippet matches “${query}”.`,
  newSnippet: (label: string) => `New snippet labelled “${label}”`,
  badgeNew: "opens the library",
  off: "Off",
  review: "Needs your review",
  broken: "Broken",
  asks: "Asks for values",
  count: (shown: number, total: number) => `${shown} of ${total}`,
  results: (n: number) => (n === 1 ? "1 result" : `${n} results`),
  verb: {
    insert: "insert",
    fill: "fill in and insert",
    review: "review",
    open: "open",
    select: "select",
    form: "open the form",
    create: "create",
  } satisfies Record<Verb, string>,
  move: "move",
  edit: "edit",
  close: "close",
  couldNotGetBack: (app: string | null) =>
    `Iamb could not get back to ${app ?? "the app"}. Click where the text should go and press Enter again.`,
  notRunning: "Iamb is not running.",
  startAgain: "Start it again",
  starting: "Starting…",
  couldNotStart: "Iamb could not start it from here. Quit Iamb from its menu and open it again.",
  pickFailed: "Iamb could not do that. Try again.",
  preview: "Preview",
} as const;
