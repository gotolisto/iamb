// The formatting toolbar of a formatted snippet (task 3.8; UX specification §4.3, "Kinds"): bold,
// italic, link, list and image, as edits of the editor's model. A formatted snippet is written as
// Markdown, so each button writes Markdown marks around or in front of the selection; the chips
// in the selection stay chips (the pieces are moved, never re-parsed), so a field inside bold
// text keeps working.
//
// `hasFormatting` decides whether switching a snippet back to plain text would lose formatting
// (the editor asks first then). It looks for the Markdown the toolbar and the language write; a
// false alarm only costs a question.

import {
  docLength,
  displayText,
  lineStart,
  replace,
  selectionRange,
  slice,
  textPieces,
  type Doc,
  type EditorState,
} from "./model";

/** What an empty selection gets between the marks, selected so typing replaces it. */
export const PLACEHOLDERS = { bold: "bold text", italic: "italic text", link: "link text" } as const;

/** The address a new link starts with, selected so typing replaces it. */
export const LINK_ADDRESS = "https://";

function literal(text: string): Doc {
  return textPieces(text, false);
}

/** Wraps the selection in `before` and `after` (`**` for bold, `*` for italic); an empty
 * selection gets `placeholder` between the marks. The selection ends up on the wrapped content. */
export function wrapSelection(s: EditorState, before: string, after: string, placeholder: string): EditorState {
  const [from, to] = selectionRange(s);
  const inner: Doc = from === to ? literal(placeholder) : slice(s.doc, from, to);
  const innerLength = from === to ? placeholder.length : to - from;
  const doc = replace(s.doc, from, to, [...literal(before), ...inner, ...literal(after)]);
  const start = from + before.length;
  return { doc, anchor: start, focus: start + innerLength };
}

/** Makes the selection the text of a link, `[text](https://)`, with the address selected. */
export function linkSelection(s: EditorState): EditorState {
  const [from, to] = selectionRange(s);
  const inner: Doc = from === to ? literal(PLACEHOLDERS.link) : slice(s.doc, from, to);
  const innerLength = from === to ? PLACEHOLDERS.link.length : to - from;
  const doc = replace(s.doc, from, to, [...literal("["), ...inner, ...literal(`](${LINK_ADDRESS})`)]);
  const start = from + 1 + innerLength + 2;
  return { doc, anchor: start, focus: start + LINK_ADDRESS.length };
}

const BULLET = "- ";

/** Turns the lines the selection touches into a bulleted list (`- ` in front of each), or back
 * into plain lines when every one of them already is a list item. */
export function toggleList(s: EditorState): EditorState {
  const [from, to] = selectionRange(s);
  const text = displayText(s.doc);
  const first = lineStart(s.doc, from);
  // A selection that ends at the start of a line does not touch that line.
  const last = to > from && text[to - 1] === "\n" ? to - 1 : to;
  const starts: number[] = [first];
  for (let i = first; i < last; i++) if (text[i] === "\n") starts.push(i + 1);
  const all = starts.every((p) => text.startsWith(BULLET, p));
  let doc = s.doc;
  let shift = 0;
  // From the last line up, so earlier positions stay where they are.
  for (const p of [...starts].reverse()) {
    if (all) doc = replace(doc, p, p + BULLET.length, []);
    else if (!text.startsWith(BULLET, p)) doc = replace(doc, p, p, literal(BULLET));
    else continue;
    shift += all ? -BULLET.length : BULLET.length;
  }
  const end = Math.max(first, to + shift);
  return { doc, anchor: Math.min(docLength(doc), first), focus: Math.min(docLength(doc), end) };
}

/** Inserts Markdown (an image) at the selection, on its own when it would touch other text. */
export function insertMarkdown(s: EditorState, markdown: string): EditorState {
  const [from, to] = selectionRange(s);
  const doc = replace(s.doc, from, to, literal(markdown));
  const caret = from + markdown.length;
  return { doc, anchor: caret, focus: caret };
}

const FORMATTING: readonly RegExp[] = [
  /(\*\*|__)(?=\S)[\s\S]*?\S\1/, // bold
  /(^|[^\w*\\])\*(?=\S)[^*\n]*\S\*/m, // italic with *
  /(^|[^\w_\\])_(?=\S)[^_\n]*\S_(?!\w)/m, // italic with _
  /~~(?=\S)[\s\S]*?\S~~/, // strikethrough
  /!?\[[^\]\n]*\]\([^)\n]*\)/, // links and images
  /<(https?:|mailto:)[^>\s]+>/i, // autolinks
  /^ {0,3}([-*+]|\d{1,9}[.)])[ \t]+\S/m, // lists
  /^ {0,3}#{1,6}[ \t]+\S/m, // headings
  /^ {0,3}>/m, // quotes
  /`[^`\n]+`/, // code
  /^ {0,3}(```|~~~)/m, // code blocks
  /^ {0,3}((-[ \t]*){3,}|(\*[ \t]*){3,}|(_[ \t]*){3,})$/m, // rules
  /^ {0,3}\|.*\|[ \t]*$/m, // tables
];

/** Whether Markdown text holds formatting that plain text would show as marks. */
export function hasFormatting(source: string): boolean {
  return FORMATTING.some((re) => re.test(source));
}
