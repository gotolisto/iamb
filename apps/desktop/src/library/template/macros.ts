// What each macro looks like in the editor: its chip (design system §4.6), its accessible name,
// the insert list (flow 2, step 3) and the macros the insert list writes. The copy is the UX
// specification's; English only until the message catalogue (task 3.11).

import { attr, type ChipName, type Macro } from "./model";

/** The chip's colour group: what the macro does (design system §4.6). */
export type ChipGroup = "insert" | "ask" | "act" | "read";

export function chipGroup(name: ChipName): ChipGroup {
  switch (name) {
    case "date":
    case "time":
    case "snippet":
      return "insert";
    case "field":
    case "if":
    case "else":
    case "endif":
      return "ask";
    case "key":
    case "wait":
    case "cursor":
      return "act";
    case "clipboard":
      return "read";
  }
}

export type IconName =
  | "date"
  | "time"
  | "snippet"
  | "field"
  | "area"
  | "choice"
  | "toggle"
  | "if"
  | "key"
  | "wait"
  | "cursor"
  | "clipboard"
  | "down"
  | "warn";

export type FieldKind = "text" | "area" | "choice" | "toggle";

export function fieldKind(macro: Macro): FieldKind {
  const t = attr(macro, "type");
  return t === "area" || t === "choice" || t === "toggle" ? t : "text";
}

export function chipIcon(macro: Macro): IconName {
  switch (macro.name) {
    case "field": {
      const kind = fieldKind(macro);
      return kind === "text" ? "field" : kind;
    }
    case "else":
    case "endif":
      return "if";
    default:
      return macro.name;
  }
}

// ---- Words ----------------------------------------------------------------------------------

/** The keys of `{key}` as the keyboard shows them, and in words for assistive technology. */
export const KEYS: ReadonlyArray<{ name: string; label: string; spoken: string }> = [
  { name: "enter", label: "Enter", spoken: "Enter" },
  { name: "tab", label: "Tab", spoken: "Tab" },
  { name: "escape", label: "Esc", spoken: "Escape" },
  { name: "backspace", label: "Backspace", spoken: "Backspace" },
  { name: "delete", label: "Delete", spoken: "Delete" },
  { name: "space", label: "Space", spoken: "Space" },
  { name: "up", label: "↑", spoken: "Up arrow" },
  { name: "down", label: "↓", spoken: "Down arrow" },
  { name: "left", label: "←", spoken: "Left arrow" },
  { name: "right", label: "→", spoken: "Right arrow" },
  { name: "home", label: "Home", spoken: "Home" },
  { name: "end", label: "End", spoken: "End" },
  { name: "pageup", label: "Page Up", spoken: "Page Up" },
  { name: "pagedown", label: "Page Down", spoken: "Page Down" },
];

/** "Enter", "Tab ×2", "Shift+Tab" (design system §4.6). */
export function keyLabel(key: string, shift: boolean, times: number, spoken = false): string {
  const k = KEYS.find((x) => x.name === key);
  const name = k ? (spoken ? k.spoken : k.label) : key;
  const base = shift ? `Shift+${name}` : name;
  if (times <= 1) return base;
  return spoken ? `${base}, ${times} times` : `${base} ×${times}`;
}

/** "0.5 s": a wait as the chip and the preview show it. */
export function seconds(ms: number): string {
  return `${Number((ms / 1000).toFixed(3))} s`;
}

/** Shift units of the template language (§4.1) in words: [singular, plural]. */
export const SHIFT_UNITS: Record<string, [string, string]> = {
  d: ["day", "days"],
  bd: ["working day", "working days"],
  w: ["week", "weeks"],
  mo: ["month", "months"],
  y: ["year", "years"],
  h: ["hour", "hours"],
  min: ["minute", "minutes"],
  s: ["second", "seconds"],
};

export interface ShiftTerm {
  sign: "+" | "-";
  count: number;
  unit: string;
}

/** The terms of a shift, or null when it is not written correctly. */
export function parseShift(shift: string): ShiftTerm[] | null {
  const parts = shift.split(/[ \t\n]+/).filter(Boolean);
  if (parts.length === 0) return null;
  const terms: ShiftTerm[] = [];
  for (const p of parts) {
    const m = /^([+-])(\d{1,4})(bd|mo|min|d|w|y|h|s)$/.exec(p);
    if (!m) return null;
    terms.push({ sign: m[1] as "+" | "-", count: Number(m[2]), unit: m[3] as string });
  }
  return terms;
}

/** "+3 working days −1 day", or in words "plus 3 working days, minus 1 day". */
export function shiftWords(shift: string, spoken = false): string {
  const terms = parseShift(shift);
  if (!terms) return shift;
  return terms
    .map((t) => {
      const [one, many] = SHIFT_UNITS[t.unit] as [string, string];
      const sign = spoken ? (t.sign === "+" ? "plus" : "minus") : t.sign === "+" ? "+" : "−";
      return `${sign}${spoken ? " " : ""}${t.count} ${t.count === 1 ? one : many}`;
    })
    .join(spoken ? ", " : " ");
}

export const FIELD_KINDS: ReadonlyArray<{ kind: FieldKind; label: string; spoken: string }> = [
  { kind: "text", label: "One line", spoken: "text" },
  { kind: "area", label: "Several lines", spoken: "several lines" },
  { kind: "choice", label: "Choice", spoken: "choice" },
  { kind: "toggle", label: "Toggle", spoken: "toggle" },
];

function ifCondition(macro: Macro): string {
  const field = attr(macro, "field") ?? "";
  const is = attr(macro, "is");
  const not = attr(macro, "not");
  if (is !== undefined) return `${field} is ${is}`;
  if (not !== undefined) return `${field} is not ${not}`;
  return field;
}

/** The chip's label (design system §4.6). */
export function chipLabel(macro: Macro): string {
  switch (macro.name) {
    case "date":
    case "time": {
      const base = macro.name === "date" ? "Date" : "Time";
      const shift = attr(macro, "shift");
      return shift ? `${base} ${shiftWords(shift)}` : base;
    }
    case "snippet":
      return attr(macro, "abbr") || "Another snippet";
    case "field":
      return attr(macro, "name") || "Fill-in field";
    case "if":
      return `If ${ifCondition(macro)}`.trimEnd();
    case "else":
      return "Else";
    case "endif":
      return "End if";
    case "key":
      return keyLabel(
        attr(macro, "name") ?? "",
        attr(macro, "shift") === "yes",
        Number(attr(macro, "times") ?? "1") || 1,
      );
    case "wait": {
      const ms = Number(attr(macro, "ms"));
      return Number.isFinite(ms) && ms > 0 ? `Wait ${seconds(ms)}` : "Wait";
    }
    case "cursor":
      return "Cursor";
    case "clipboard":
      return "Clipboard";
  }
}

/** The chip's accessible name: "Fill-in field, First name, text" · "Date, plus 3 working
 * days" · "Key press, Enter" (design system §4.6). */
export function chipSpokenName(macro: Macro): string {
  switch (macro.name) {
    case "date":
    case "time": {
      const base = macro.name === "date" ? "Date" : "Time";
      const shift = attr(macro, "shift");
      return shift ? `${base}, ${shiftWords(shift, true)}` : base;
    }
    case "snippet":
      return `Another snippet, ${attr(macro, "abbr") ?? ""}`;
    case "field": {
      const kind = FIELD_KINDS.find((k) => k.kind === fieldKind(macro))?.spoken ?? "text";
      return `Fill-in field, ${attr(macro, "name") ?? ""}, ${kind}`;
    }
    case "if":
      return `Optional part, if ${ifCondition(macro)}`;
    case "else":
      return "Optional part, else";
    case "endif":
      return "Optional part, end if";
    case "key":
      return `Key press, ${keyLabel(
        attr(macro, "name") ?? "",
        attr(macro, "shift") === "yes",
        Number(attr(macro, "times") ?? "1") || 1,
        true,
      )}`;
    case "wait": {
      const ms = Number(attr(macro, "ms"));
      return Number.isFinite(ms) && ms > 0 ? `Wait, ${seconds(ms).replace(" s", " seconds")}` : "Wait";
    }
    case "cursor":
      return "Cursor position";
    case "clipboard":
      return "Clipboard";
  }
}

// ---- The insert list ------------------------------------------------------------------------

export type InsertKind =
  | "date"
  | "time"
  | "field"
  | "if"
  | "cursor"
  | "clipboard"
  | "snippet"
  | "key"
  | "wait";

export interface InsertItem {
  kind: InsertKind;
  label: string;
  hint: string;
  icon: IconName;
  /** Words the filter matches the start of, besides the label's. */
  words: readonly string[];
}

/** The nine items, in the order of flow 2, step 3. */
export const INSERT_ITEMS: readonly InsertItem[] = [
  { kind: "date", label: "Date", hint: "today, or shifted", icon: "date", words: ["today"] },
  { kind: "time", label: "Time", hint: "now, or shifted", icon: "time", words: ["now"] },
  { kind: "field", label: "Fill-in field", hint: "ask when expanding", icon: "field", words: ["ask", "blank"] },
  { kind: "if", label: "Optional part", hint: "only if a field says so", icon: "if", words: ["if", "condition"] },
  { kind: "cursor", label: "Cursor position", hint: "where typing continues", icon: "cursor", words: ["caret"] },
  { kind: "clipboard", label: "Clipboard", hint: "what you last copied", icon: "clipboard", words: ["paste"] },
  { kind: "snippet", label: "Another snippet", hint: "by abbreviation", icon: "snippet", words: ["snippet", "include"] },
  { kind: "key", label: "Key press", hint: "Enter, Tab, arrows…", icon: "key", words: ["enter", "tab", "press"] },
  { kind: "wait", label: "Wait", hint: "pause before the next step", icon: "wait", words: ["pause", "delay"] },
];

/** The items whose label or words start with `query` (case-insensitive), in list order. A
 * formatted snippet has no cursor position (UX specification §4.3). */
export function filterItems(query: string, kind: "plain" | "rich" = "plain"): InsertItem[] {
  const q = query.trim().toLowerCase();
  const items = INSERT_ITEMS.filter((item) => kind === "plain" || item.kind !== "cursor");
  if (!q) return items;
  // Items whose label starts with the query first, then those with a word that does.
  const byLabel = items.filter((item) => item.label.toLowerCase().startsWith(q));
  const byWord = items.filter(
    (item) =>
      !byLabel.includes(item) &&
      [...item.label.toLowerCase().split(/[\s-]+/), ...item.words].some((w) => w.startsWith(q)),
  );
  return [...byLabel, ...byWord];
}

/** A name not used yet: "Field 1", "Field 2", … */
export function freshName(base: string, taken: readonly string[]): string {
  for (let n = 1; ; n++) {
    const name = `${base} ${n}`;
    if (!taken.includes(name)) return name;
  }
}

/** The macro an insert item writes first; its card then opens to set it. */
export function newMacro(kind: Exclude<InsertKind, "if">, fields: readonly string[]): Macro {
  switch (kind) {
    case "date":
    case "time":
    case "cursor":
    case "clipboard":
      return { name: kind, attrs: [] };
    case "field":
      return { name: "field", attrs: [{ name: "name", value: freshName("Field", fields) }] };
    case "snippet":
      return { name: "snippet", attrs: [{ name: "abbr", value: "" }] };
    case "key":
      return { name: "key", attrs: [{ name: "name", value: "enter" }] };
    case "wait":
      return { name: "wait", attrs: [{ name: "ms", value: "500" }] };
  }
}

/** The opening chip of a new optional part: the first field of the snippet, or a new toggle. */
export function newOptionalPart(fields: readonly string[]): Macro {
  return { name: "if", attrs: [{ name: "field", value: fields[0] ?? freshName("Option", fields) }] };
}

/** Whether inserting `kind` opens its card at once: those that need a value from the user (a
 * name, a key, an abbreviation, a field, a duration). Date, time, clipboard and cursor are
 * complete as inserted; Enter on the chip opens their card. */
export function opensCard(kind: InsertKind): boolean {
  return kind === "field" || kind === "if" || kind === "snippet" || kind === "key" || kind === "wait";
}
