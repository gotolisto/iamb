// The library's logic without React: the folder tree, the list's order and filter, the words the
// interface says about an abbreviation, and the keyboard's platform differences.
//
// Everything here works on library content already in memory. Nothing logs it.

import {
  type AbbreviationCheck,
  type CaseMode,
  type Folder,
  MAX_FOLDER_DEPTH,
  type Summary,
  TAG_COLORS,
  type Tag,
  type TagColor,
  type Trigger,
} from "./api";

// ---- Platform -------------------------------------------------------------------------------

/** macOS writes shortcuts as symbols and uses ⌘; Windows writes words and uses Ctrl. */
export function isMac(platform: string = typeof navigator === "undefined" ? "" : navigator.platform): boolean {
  return /mac|iphone|ipad/i.test(platform);
}

/** A shortcut as the platform writes it, from `mod` (⌘ or Ctrl), `shift` and a key. */
export function shortcutLabel(
  parts: { shift?: boolean; key: string },
  mac: boolean = isMac(),
): string {
  const key = parts.key.length === 1 ? parts.key.toUpperCase() : parts.key;
  return mac
    ? `⌘${parts.shift ? "⇧" : ""}${key}`
    : `Ctrl+${parts.shift ? "Shift+" : ""}${key}`;
}

/** The value of `aria-keyshortcuts` for the same shortcut. */
export function ariaShortcut(parts: { shift?: boolean; key: string }, mac: boolean = isMac()): string {
  return `${mac ? "Meta" : "Control"}${parts.shift ? "+Shift" : ""}+${parts.key.toUpperCase()}`;
}

/** Whether `e` carries the platform's command modifier (and no other of Ctrl, Alt, Meta). */
export function hasMod(
  e: { metaKey: boolean; ctrlKey: boolean; altKey: boolean },
  mac: boolean = isMac(),
): boolean {
  return !e.altKey && (mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey);
}

// ---- Words ----------------------------------------------------------------------------------

const SPOKEN: Record<string, string> = {
  ";": "semicolon",
  ":": "colon",
  ",": "comma",
  ".": "dot",
  "/": "slash",
  "\\": "backslash",
  "-": "dash",
  _: "underscore",
  "#": "hash",
  "@": "at",
  "!": "exclamation mark",
  "?": "question mark",
  "'": "apostrophe",
  '"': "quote",
  "`": "backtick",
  "+": "plus",
  "=": "equals",
  "*": "star",
  "&": "and",
  "%": "percent",
  $: "dollar",
  "(": "open parenthesis",
  ")": "close parenthesis",
  "[": "open bracket",
  "]": "close bracket",
  "{": "open brace",
  "}": "close brace",
  "<": "less than",
  ">": "greater than",
  "|": "bar",
  "~": "tilde",
  "^": "caret",
};

/** An abbreviation as a screen reader should say it: ";rlate" is "semicolon r late". */
export function speakAbbreviation(abbreviation: string): string {
  const words: string[] = [];
  let run = "";
  const flush = () => {
    if (run) words.push(run);
    run = "";
  };
  for (const ch of abbreviation) {
    const spoken = SPOKEN[ch];
    if (spoken) {
      flush();
      words.push(spoken);
    } else if (ch === " ") {
      flush();
    } else {
      run += ch;
    }
  }
  flush();
  return words.join(" ");
}

/** The row's second line: the first line of the content with fill-in fields shown as ‹Name›. */
export function previewLine(source: string): string {
  return source.replace(/\{field\b([^}]*)\}/g, (whole, attrs: string) => {
    const m = /\bname\s*=\s*(?:"([^"]*)"|([^\s}"]+))/.exec(attrs);
    const name = m?.[1] ?? m?.[2];
    return name ? `‹${name}›` : whole;
  });
}

/** Whether the content asks for values when the snippet expands (it has a fill-in field). */
export function asksForValues(source: string): boolean {
  return /\{field\b/.test(source);
}

/** What a row shows as its title: the label, or the first line of the content. */
export function rowTitle(s: Pick<Summary, "label" | "firstLine" | "abbreviation">): string {
  return s.label.trim() || previewLine(s.firstLine).trim() || s.abbreviation;
}

// ---- Folders --------------------------------------------------------------------------------

export interface TreeNode {
  folder: Folder;
  depth: number;
  hasChildren: boolean;
  expanded: boolean;
  /** Snippets directly in the folder. */
  count: number;
}

/** Siblings are ordered by position, then name. */
function compareFolders(a: Folder, b: Folder): number {
  return a.position - b.position || a.name.localeCompare(b.name);
}

export function childrenOf(folders: Folder[], parentId: string | null): Folder[] {
  return folders.filter((f) => f.parentId === parentId).sort(compareFolders);
}

/** The folders of the tree in reading order, with the children of collapsed folders left out. */
export function visibleTree(
  folders: Folder[],
  counts: ReadonlyMap<string, number>,
  collapsed: ReadonlySet<string>,
): TreeNode[] {
  const byParent = new Map<string | null, Folder[]>();
  const ids = new Set(folders.map((f) => f.id));
  for (const f of folders) {
    // A folder whose parent is missing is shown at the top rather than lost.
    const key = f.parentId !== null && ids.has(f.parentId) ? f.parentId : null;
    byParent.set(key, [...(byParent.get(key) ?? []), f]);
  }
  for (const list of byParent.values()) list.sort(compareFolders);
  const out: TreeNode[] = [];
  const walk = (parent: string | null, depth: number) => {
    for (const folder of byParent.get(parent) ?? []) {
      const kids = byParent.get(folder.id) ?? [];
      const expanded = !collapsed.has(folder.id);
      out.push({
        folder,
        depth,
        hasChildren: kids.length > 0,
        expanded,
        count: counts.get(folder.id) ?? 0,
      });
      if (expanded && kids.length > 0) walk(folder.id, depth + 1);
    }
  };
  walk(null, 1);
  return out;
}

/** The level of a folder: 1 at the top. */
export function folderLevel(folders: Folder[], id: string): number {
  const byId = new Map(folders.map((f) => [f.id, f]));
  let level = 0;
  let cur: Folder | undefined = byId.get(id);
  while (cur && level <= MAX_FOLDER_DEPTH + 1) {
    level += 1;
    cur = cur.parentId === null ? undefined : byId.get(cur.parentId);
  }
  return level;
}

/** The folder and everything below it. */
export function subtreeIds(folders: Folder[], id: string): Set<string> {
  const out = new Set([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const f of folders) {
      if (f.parentId !== null && out.has(f.parentId) && !out.has(f.id)) {
        out.add(f.id);
        grew = true;
      }
    }
  }
  return out;
}

/** "Replies / Shop / Returns", for a folder select where nesting is shown with indents. */
export function folderOptions(folders: Folder[]): { id: string; label: string; depth: number }[] {
  return visibleTree(folders, new Map(), new Set()).map((n) => ({
    id: n.folder.id,
    label: n.folder.name,
    depth: n.depth,
  }));
}

export function countByFolder(rows: readonly Summary[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const r of rows) counts.set(r.folderId, (counts.get(r.folderId) ?? 0) + 1);
  return counts;
}

// ---- Tags -----------------------------------------------------------------------------------

/** The colour a new tag gets: the first of the eight that fewest tags use. */
export function nextTagColor(tags: readonly Tag[]): TagColor {
  const used = new Map<TagColor, number>(TAG_COLORS.map((c) => [c, 0]));
  for (const t of tags) used.set(t.color, (used.get(t.color) ?? 0) + 1);
  let best: TagColor = TAG_COLORS[0];
  for (const c of TAG_COLORS) if ((used.get(c) ?? 0) < (used.get(best) ?? 0)) best = c;
  return best;
}

/** Tag names compare without regard to case, as the service does. */
export function sameTagName(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

// ---- The list: order and filter -------------------------------------------------------------

export type SortMode = "label" | "changed" | "used";

export interface Indexed {
  row: Summary;
  /** The lower-case text the filter looks in: label, abbreviation, first line and tag names. */
  hay: string;
  /** The lower-case key of the label order. */
  key: string;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

/** Prepares the rows for filtering and ordering; done once per change of the library. */
export function indexRows(rows: readonly Summary[], tags: readonly Tag[]): Indexed[] {
  const names = new Map(tags.map((t) => [t.id, t.name.toLowerCase()]));
  return rows.map((row) => {
    const tagText = row.tags.map((id) => names.get(id) ?? "").join(" ");
    const title = rowTitle(row);
    return {
      row,
      hay: `${row.label}\n${row.abbreviation}\n${row.firstLine}\n${tagText}`.toLowerCase(),
      key: title.toLowerCase(),
    };
  });
}

export function sortIndexed(items: Indexed[], mode: SortMode): Indexed[] {
  const copy = items.slice();
  if (mode === "used") {
    copy.sort(
      (a, b) =>
        (b.row.uses ?? 0) - (a.row.uses ?? 0) ||
        (b.row.lastUsed ?? 0) - (a.row.lastUsed ?? 0) ||
        collator.compare(a.key, b.key),
    );
  } else if (mode === "changed") {
    copy.sort((a, b) => b.row.updatedAt - a.row.updatedAt || collator.compare(a.key, b.key));
  } else {
    copy.sort((a, b) => collator.compare(a.key, b.key) || a.row.id.localeCompare(b.row.id));
  }
  return copy;
}

/** Items whose text holds every word of `query` (any order, any case). */
export function filterIndexed(items: readonly Indexed[], query: string): Indexed[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return items.slice();
  return items.filter((i) => words.every((w) => i.hay.includes(w)));
}

// ---- The abbreviation -----------------------------------------------------------------------

export const ABBREVIATION_MAX = 64;

export type LocalProblem = "empty" | "space" | "long";

/** What is wrong with an abbreviation without asking the service; blocks saving. */
export function localProblem(text: string): LocalProblem | null {
  if (text.length === 0) return "empty";
  if (/\s/u.test(text)) return "space";
  if ([...text].length > ABBREVIATION_MAX) return "long";
  return null;
}

/** A plain word made only of letters, which can fire in the middle of ordinary typing. */
export function isPlainWord(text: string): boolean {
  return /^\p{L}+$/u.test(text);
}

export function expandsAfter(trigger: Trigger): string {
  return trigger === "immediate" ? "Immediately" : "After a space or punctuation";
}

export function caseLabel(mode: CaseMode): string {
  switch (mode) {
    case "exact":
      return "Exactly as written";
    case "insensitive":
      return "Ignore";
    case "adaptive":
      return "Ignore, and follow mine";
  }
}

/** The footer of the editor: the options in words. */
export function optionsSummary(trigger: Trigger, caseMode: CaseMode): string {
  const expand = trigger === "immediate" ? "Expands immediately" : "Expands after a space or punctuation";
  return `${expand} · Case: ${caseLabel(caseMode).toLowerCase()}`;
}

export interface Message {
  tone: "error" | "warning";
  /** Parts of the sentence; `strong` ones are shown in bold (an abbreviation). */
  parts: { text: string; strong?: boolean }[];
  /** A button after the sentence. */
  action?: { label: string; kind: "show-snippet"; snippet: string };
  /** Blocks saving. */
  blocks: boolean;
}

const t = (text: string) => ({ text });
const b = (text: string) => ({ text, strong: true });

/** The text of a message, for tests and accessible names. */
export function messageText(m: Message): string {
  return m.parts.map((p) => p.text).join("");
}

export interface MessageContext {
  abbreviation: string;
  trigger: Trigger;
  /** The snippets the service named, as rows, when known. */
  rowOf: (id: string) => Summary | undefined;
  folderName: (id: string) => string;
}

/** The messages under the Abbreviation field, in the order of the specification (§4.3). */
export function abbreviationMessages(
  ctx: MessageContext,
  check: AbbreviationCheck | null,
): Message[] {
  const out: Message[] = [];
  const problem = localProblem(ctx.abbreviation);
  if (problem === "empty") {
    return [{ tone: "error", parts: [t("Type an abbreviation.")], blocks: true }];
  }
  if (problem === "space") {
    return [
      { tone: "error", parts: [t("An abbreviation cannot contain spaces.")], blocks: true },
    ];
  }
  if (problem === "long") {
    return [
      {
        tone: "error",
        parts: [t(`An abbreviation can be at most ${ABBREVIATION_MAX} characters.`)],
        blocks: true,
      },
    ];
  }
  const abbr = ctx.abbreviation;
  if (check?.conflict) {
    const other = ctx.rowOf(check.conflict);
    const name = other ? rowTitle(other) : "another snippet";
    const where = other ? ` in ${ctx.folderName(other.folderId)}` : "";
    out.push({
      tone: "error",
      parts: [
        b(abbr),
        t(` is already the abbreviation of “${name}”${where}. Each abbreviation can belong to one snippet.`),
      ],
      action: { label: "Show that snippet", kind: "show-snippet", snippet: check.conflict },
      blocks: true,
    });
  }
  const hidden = check?.hides.map((id) => ctx.rowOf(id)).find((r) => r !== undefined);
  if (ctx.trigger === "immediate" && hidden) {
    out.push({
      tone: "warning",
      parts: [
        t("This snippet expands immediately, so "),
        b(abbr),
        t(" will fire before you can finish typing "),
        b(hidden.abbreviation),
        t(". Use a different abbreviation, or let it wait for a space."),
      ],
      blocks: false,
    });
  }
  const hider = check?.hiddenBy.map((id) => ctx.rowOf(id)).find((r) => r !== undefined);
  if (hider) {
    out.push({
      tone: "warning",
      parts: [
        b(hider.abbreviation),
        t(" expands immediately and will fire before "),
        b(abbr),
        t(" can be typed. Change one of the two."),
      ],
      blocks: false,
    });
  }
  if (ctx.trigger === "immediate" && isPlainWord(abbr)) {
    out.push({
      tone: "warning",
      parts: [
        t(
          "Short abbreviations made only of letters can fire in the middle of ordinary typing. Starting with a character such as ; avoids that.",
        ),
      ],
      blocks: false,
    });
  }
  return out;
}

// ---- Template errors (specification §6.1) -----------------------------------------------------

const TEMPLATE_ERRORS: Record<string, string> = {
  E_UNCLOSED_MACRO: "Something that starts with { is never closed. Add the closing } or delete it.",
  E_BAD_SYNTAX: "This part is not written correctly. Open it and set its values again.",
  E_UNKNOWN_ATTR:
    "This part has a setting it does not understand. Open it and set its values again.",
  E_DUPLICATE_ATTR: "This part has the same setting twice. Open it and set its values again.",
  E_MISSING_ATTR:
    "This part is missing something: a fill-in field needs a name, a key press needs a key.",
  E_BAD_VALUE: "This part has a value that is not allowed.",
  E_BAD_FORMAT: "Iamb does not know this date or time code. See the list of codes.",
  E_BAD_SHIFT:
    "The shift is not written correctly. Use a sign, a number and a unit, for example +3 days.",
  E_UNBALANCED_IF: "This optional part has a start but no end.",
  E_TOO_DEEP: "Optional parts, or snippets inside snippets, are nested more than 8 deep.",
  E_SNIPPET_NOT_FOUND: "It includes a snippet that does not exist.",
  E_SNIPPET_CYCLE: "It includes itself. A snippet cannot contain itself.",
  E_FIELD_CONFLICT:
    "Two fill-in fields have the same name but are of different kinds. Give one another name, or make them the same.",
  E_FIELD_UNDEFINED:
    "This optional part looks at a field, but there is no fill-in field with that name.",
  E_FORM_TOO_LARGE: "This snippet asks for more than 100 values.",
  E_DATE_RANGE: "The shifted date falls outside the years 1 to 9999.",
  E_CURSOR_BEFORE_KEY:
    "A key press comes after the cursor position. Move the cursor position to the end, or remove the key press: Iamb cannot know where the key leaves the cursor.",
  E_WAIT_TOTAL: "The waits add up to more than 30 seconds.",
  E_OUTPUT_TOO_LARGE: "The text is longer than 100,000 characters.",
};

/** The sentence for a template error code (`broken` of a summary), or a general one. */
export function templateErrorSentence(code: string): string {
  return TEMPLATE_ERRORS[code] ?? "This snippet has an error.";
}

/** Whether `broken` is a template error rather than a lost abbreviation clash. */
export function isTemplateError(broken: string | null): broken is string {
  return broken !== null && broken.startsWith("E_");
}

// ---- Usage on this computer (ADR-0015, specification §4.3 and §9 rule 6) -----------------------

/** "today", "yesterday", "3 days ago", or the date, for a time in the past. */
export function sinceText(at: number, now: Date = new Date()): string {
  const day = (d: Date) => Math.floor(new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() / 86_400_000);
  const days = day(now) - day(new Date(at));
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  const d = new Date(at);
  return `on ${d.toLocaleDateString("en-GB", { day: "numeric", month: "long", ...(d.getFullYear() === now.getFullYear() ? {} : { year: "numeric" }) })}`;
}

/**
 * The editor's footer fact, always labelled as local: "Used 31 times on this computer · last
 * used yesterday". null when the service does not count.
 */
export function usageText(uses: number | null, lastUsed: number | null, now: Date = new Date()): string | null {
  if (uses === null) return null;
  if (uses === 0) return "Not used yet on this computer";
  const times = uses === 1 ? "Used once" : `Used ${uses} times`;
  return `${times} on this computer${lastUsed === null ? "" : ` · last used ${sinceText(lastUsed, now)}`}`;
}
