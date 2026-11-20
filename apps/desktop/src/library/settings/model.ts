// The pure parts of Settings: what the stored strings mean and how the controls write them.
// Nothing here talks to the service.

import type { SettingKey } from "./api";

// ---- Delimiters (ADR-0026) -----------------------------------------------------------------

/** The default set: space and `. , ; : ! ? ) ] } " '` (`Delimiters::DEFAULT_CHARS`). */
export const DEFAULT_DELIMITERS = " .,;:!?)]}\"'";
export const PUNCTUATION = ".,;:!?";
export const CLOSERS = ")]}\"'";
export const ENTER = "\n";
export const TAB = "\t";

export type Mark = "on" | "off" | "mixed";

export interface DelimiterChoices {
  punctuation: Mark;
  closers: Mark;
  enter: boolean;
  tab: boolean;
}

const mark = (set: ReadonlySet<string>, chars: string): Mark => {
  const n = [...chars].filter((c) => set.has(c)).length;
  return n === 0 ? "off" : n === chars.length ? "on" : "mixed";
};

/** What the checkboxes show for the stored `delimiters` (absent: the default set). */
export function delimiterChoices(stored: string | undefined): DelimiterChoices {
  const set = new Set([...(stored ?? DEFAULT_DELIMITERS)]);
  return {
    punctuation: mark(set, PUNCTUATION),
    closers: mark(set, CLOSERS),
    enter: set.has(ENTER),
    tab: set.has(TAB),
  };
}

export type DelimiterGroup = "punctuation" | "closers" | "enter" | "tab";

const GROUP_CHARS: Record<DelimiterGroup, string> = {
  punctuation: PUNCTUATION,
  closers: CLOSERS,
  enter: ENTER,
  tab: TAB,
};

/** The set after turning a group on or off. A space always stays; characters of no group stay. */
export function withGroup(stored: string | undefined, group: DelimiterGroup, on: boolean): string {
  const set = new Set([...(stored ?? DEFAULT_DELIMITERS), " "]);
  for (const c of GROUP_CHARS[group]) {
    if (on) set.add(c);
    else set.delete(c);
  }
  return [...set].join("");
}

/** The value to store for `delimiters`: nothing when it is the default set. */
export function delimitersToStore(value: string): string | null {
  const a = [...new Set([...value])].sort().join("");
  const b = [...new Set([...DEFAULT_DELIMITERS])].sort().join("");
  return a === b ? null : value;
}

// ---- Dates and times --------------------------------------------------------------------------

export const DATE_PRESETS = ["%Y-%m-%d", "%d/%m/%Y", "%m/%d/%Y", "%d.%m.%Y", "%B %-d, %Y", "%-d %B %Y"];
export const TIME_PRESETS = ["%H:%M", "%H:%M:%S", "%-I:%M %p", "%I:%M %p"];
export const DEFAULT_DATE_FORMAT = "%Y-%m-%d";
export const DEFAULT_TIME_FORMAT = "%H:%M";

// `%-m` and the like before `%m`: the longest code wins.
const CODES = ["%-m", "%-d", "%-H", "%-I", "%Y", "%y", "%m", "%d", "%B", "%b", "%A", "%a", "%H", "%I", "%M", "%S", "%p", "%j", "%%"];

/** The first code of `format` that Iamb does not know (`%Q`, or a lone `%`), else null. */
export function unknownFormatCode(format: string): string | null {
  let i = 0;
  while (i < format.length) {
    if (format[i] !== "%") {
      i += 1;
      continue;
    }
    const code = CODES.find((c) => format.startsWith(c, i));
    if (!code) return format.slice(i, i + 2);
    i += code.length;
  }
  return null;
}

type Names = readonly [months: string[], monthsShort: string[], days: string[], daysShort: string[]];

// The names of the template language's four locales (crates/iamb-core/src/datetime.rs).
const NAMES: Record<Language, Names> = {
  en: [
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
  ],
  es: [
    ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"],
    ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"],
    ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
  ],
  de: [
    ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
    ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"],
    ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
  ],
  fr: [
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"],
    ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
  ],
};

export type Language = "en" | "es" | "de" | "fr";
export const LANGUAGES: { value: Language; label: string }[] = [
  { value: "en", label: "English" },
  { value: "es", label: "Español" },
  { value: "de", label: "Deutsch" },
  { value: "fr", label: "Français" },
];

/** The language of a locale tag: the region is ignored, unknown languages are English. */
export function languageOf(tag: string | undefined): Language {
  const lang = (tag ?? "en").split("-")[0]?.toLowerCase();
  return lang === "es" || lang === "de" || lang === "fr" ? lang : "en";
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

/** `format` shown for `date`, for the examples: the same codes and names the service writes. */
export function formatExample(format: string, date: Date, language: Language): string {
  const [months, monthsShort, days, daysShort] = NAMES[language];
  const weekday = (date.getDay() + 6) % 7;
  const hour12 = ((date.getHours() + 11) % 12) + 1;
  const start = new Date(date.getFullYear(), 0, 1);
  const dayOfYear = Math.round((new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime() - start.getTime()) / 86_400_000) + 1;
  const value: Record<string, string> = {
    "%Y": pad(date.getFullYear(), 4),
    "%y": pad(date.getFullYear() % 100),
    "%m": pad(date.getMonth() + 1),
    "%-m": String(date.getMonth() + 1),
    "%d": pad(date.getDate()),
    "%-d": String(date.getDate()),
    "%B": months[date.getMonth()] ?? "",
    "%b": monthsShort[date.getMonth()] ?? "",
    "%A": days[weekday] ?? "",
    "%a": daysShort[weekday] ?? "",
    "%H": pad(date.getHours()),
    "%-H": String(date.getHours()),
    "%I": pad(hour12),
    "%-I": String(hour12),
    "%M": pad(date.getMinutes()),
    "%S": pad(date.getSeconds()),
    "%p": date.getHours() < 12 ? "AM" : "PM",
    "%j": pad(dayOfYear, 3),
    "%%": "%",
  };
  let out = "";
  let i = 0;
  while (i < format.length) {
    const code = format[i] === "%" ? CODES.find((c) => format.startsWith(c, i)) : undefined;
    if (code) {
      out += value[code] ?? "";
      i += code.length;
    } else {
      out += format[i] ?? "";
      i += 1;
    }
  }
  return out;
}

// ---- The shortcut recorder -------------------------------------------------------------------

export interface KeyLike {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

export type Recorded =
  /** Only modifiers are down. */
  | { kind: "waiting" }
  /** A chord the service reads (`Ctrl+Shift+Space`). */
  | { kind: "chord"; chord: string }
  /** A key that cannot be a shortcut, or a chord that would swallow typing. */
  | { kind: "refused"; reason: "modifier" | "key" };

const NAMED: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Delete: "Delete",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
};

function keyOf(e: KeyLike): string | null {
  const letter = /^Key([A-Z])$/.exec(e.code);
  if (letter) return letter[1] ?? null;
  const digit = /^Digit([0-9])$/.exec(e.code);
  if (digit) return digit[1] ?? null;
  const fn = /^F([0-9]{1,2})$/.exec(e.code);
  if (fn && Number(fn[1]) >= 1 && Number(fn[1]) <= 24) return `F${fn[1]}`;
  return NAMED[e.code] ?? null;
}

const MODIFIER_CODES = /^(Control|Shift|Alt|Meta|AltGraph|OS)(Left|Right)?$/;

/** What a key press means to the recorder: the chord in the form the service stores. */
export function recordChord(e: KeyLike): Recorded {
  if (MODIFIER_CODES.test(e.code) || ["Control", "Shift", "Alt", "Meta", "AltGraph"].includes(e.key)) {
    return { kind: "waiting" };
  }
  const key = keyOf(e);
  if (key === null) return { kind: "refused", reason: "key" };
  const strong = e.ctrlKey || e.altKey || e.metaKey;
  if (!strong && !/^F[0-9]+$/.test(key)) return { kind: "refused", reason: "modifier" };
  const parts = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta", key].filter(
    (p): p is string => typeof p === "string",
  );
  return { kind: "chord", chord: parts.join("+") };
}

const MAC_SYMBOL: Record<string, string> = { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘" };
const WIN_NAME: Record<string, string> = { Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift", Meta: "Win" };

/** The parts of a stored chord as the key badges show them. */
export function chordBadges(chord: string, mac: boolean): string[] {
  return chord
    .split("+")
    .map((p) => p.trim())
    .filter((p) => p !== "")
    .map((p) => {
      const canon = canonicalModifier(p);
      if (canon) return (mac ? MAC_SYMBOL : WIN_NAME)[canon] ?? p;
      return p.length === 1 ? p.toUpperCase() : p.charAt(0).toUpperCase() + p.slice(1);
    });
}

function canonicalModifier(part: string): "Ctrl" | "Alt" | "Shift" | "Meta" | null {
  switch (part.toLowerCase()) {
    case "ctrl":
    case "control":
      return "Ctrl";
    case "alt":
    case "option":
      return "Alt";
    case "shift":
      return "Shift";
    case "meta":
    case "cmd":
    case "command":
    case "win":
    case "super":
      return "Meta";
    default:
      return null;
  }
}

/** The chord as a screen reader reads it: "Control, Shift, Space". */
export function chordSpoken(chord: string, mac: boolean): string {
  const names: Record<string, string> = mac
    ? { Ctrl: "Control", Alt: "Option", Shift: "Shift", Meta: "Command" }
    : { Ctrl: "Control", Alt: "Alt", Shift: "Shift", Meta: "Windows" };
  return chord
    .split("+")
    .map((p) => {
      const canon = canonicalModifier(p.trim());
      return canon ? (names[canon] ?? p) : p.trim();
    })
    .join(", ");
}

// ---- Injection overrides (ADR-0025) -----------------------------------------------------------

export type Method = "keys" | "paste";
export type LineBreak = "enter" | "shift+enter";

export interface AppOverride {
  /** The application id as written in the document. */
  app: string;
  /** Absent: "Let Iamb decide". */
  method: Method | null;
  lineBreak: LineBreak | null;
  keyDelayMs: number | null;
}

type Doc = { defaults?: unknown; apps?: Record<string, Record<string, unknown>>; [k: string]: unknown };

function parseDoc(text: string | null | undefined): Doc | null {
  if (text === undefined || text === null || text.trim() === "") return {};
  try {
    const v: unknown = JSON.parse(text);
    return typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Doc) : null;
  } catch {
    return null;
  }
}

/** The per-app settings the document holds; null when the document is not one this page can edit. */
export function readOverrides(text: string | null | undefined): AppOverride[] | null {
  const doc = parseDoc(text);
  if (!doc) return null;
  const apps = doc.apps ?? {};
  return Object.entries(apps).map(([app, entry]) => {
    const e = entry ?? {};
    const method = e.method === "keys" || e.method === "paste" ? e.method : null;
    const lineBreak = e.line_break === "enter" || e.line_break === "shift+enter" ? e.line_break : null;
    return {
      app,
      method,
      lineBreak,
      keyDelayMs: typeof e.key_delay_ms === "number" ? e.key_delay_ms : null,
    };
  });
}

/** The same application, however it is spelled (Windows: without a folder or `.exe`). */
export function sameApp(a: string, b: string): boolean {
  const key = (s: string) => (s.split(/[\\/]/).pop() ?? s).trim().toLowerCase().replace(/\.exe$/, "");
  return key(a) === key(b);
}

export interface OverridePatch {
  method?: Method | null;
  lineBreak?: LineBreak | null;
  keyDelayMs?: number | null;
}

/**
 * The document with `patch` applied to `app`: other members and fields of the document stay as
 * they are. An app's entry stays, even when empty, until `withoutOverride` removes it; a
 * document left with nothing in it is no setting (null: remove it). The result is null itself
 * for a document this page cannot read (it is not touched).
 */
export function withOverride(text: string | null | undefined, app: string, patch: OverridePatch): { text: string | null } | null {
  const doc = parseDoc(text);
  if (!doc) return null;
  const apps: Record<string, Record<string, unknown>> = { ...(doc.apps ?? {}) };
  const existing = Object.keys(apps).find((k) => sameApp(k, app));
  const name = existing ?? app;
  const entry: Record<string, unknown> = { ...(apps[name] ?? {}) };
  const set = (field: string, value: unknown) => {
    if (value === null) delete entry[field];
    else entry[field] = value;
  };
  if ("method" in patch) set("method", patch.method ?? null);
  if ("lineBreak" in patch) set("line_break", patch.lineBreak ?? null);
  if ("keyDelayMs" in patch) set("key_delay_ms", patch.keyDelayMs ?? null);
  apps[name] = entry;
  const next: Doc = { ...doc, apps };
  return { text: serialise(next) };
}

/** The document without `app`. */
export function withoutOverride(text: string | null | undefined, app: string): { text: string | null } | null {
  const doc = parseDoc(text);
  if (!doc) return null;
  const apps: Record<string, Record<string, unknown>> = {};
  for (const [k, v] of Object.entries(doc.apps ?? {})) if (!sameApp(k, app)) apps[k] = v;
  return { text: serialise({ ...doc, apps }) };
}

function serialise(doc: Doc): string | null {
  const apps = doc.apps ?? {};
  const rest: Doc = { ...doc };
  delete rest.apps;
  if (Object.keys(apps).length > 0) rest.apps = apps;
  return Object.keys(rest).length === 0 ? null : JSON.stringify(rest);
}

// ---- Which key a control writes ---------------------------------------------------------------

/** Where a setting lives in the sections, for the test that every key has a control. */
export const SECTION_OF_KEY: Record<SettingKey, "general" | "typing" | "dates" | "shortcuts" | "apps" | "privacy" | "onboarding"> = {
  delimiters: "typing",
  date_format: "dates",
  time_format: "dates",
  locale: "dates",
  palette_hotkey: "shortcuts",
  injection_overrides: "apps",
  "ui.onboarding_done": "onboarding",
  "ui.theme": "general",
  "ui.tray_icon": "general",
  "ui.new_trigger": "typing",
  "ui.new_case_mode": "typing",
  "ui.crash_reports": "privacy",
};
