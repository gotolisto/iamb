// The fill-in form's rules, without React: which control each field gets, what it starts
// with, how values are made to fit their field before they are sent, the key names of each
// system, and the copy (docs/ux/spec.md §4.4, §5.4; flows/4-fill-in.md; design-system §4.13;
// template-language §4.5).

import type { FieldView } from "../ipc";
import type { FormOpen } from "./api";

/** The system whose conventions the panel follows (button order, key names). */
export type Os = "mac" | "win";

/** The system the page runs on; Linux (development only) follows Windows. */
export function currentOs(): Os {
  if (typeof navigator === "undefined") return "win";
  const platform = navigator.platform || navigator.userAgent;
  return /Mac|iPhone|iPad/i.test(platform) ? "mac" : "win";
}

/** A choice of up to this many options is a list of radio buttons; more is a select. */
export const MAX_RADIO_OPTIONS = 6;

export type Control = "input" | "textarea" | "radios" | "select" | "checkbox";

/** The control a field is drawn with (design system §4.13). */
export function controlOf(field: FieldView): Control {
  switch (field.kind) {
    case "area":
      return "textarea";
    case "choice":
      return field.options.length <= MAX_RADIO_OPTIONS ? "radios" : "select";
    case "toggle":
      return "checkbox";
    default:
      return "input";
  }
}

const LINE_BREAKS = /\r\n|\r|\n/g;

/**
 * `value` made to fit `field`, as the template language corrects a submitted value
 * (§4.5): a one-line value has no line breaks, a choice is one of its options, a toggle is
 * `yes` or `no`. Nothing is ever reported as an error: no field is required.
 */
export function fit(field: FieldView, value: string | undefined): string {
  switch (field.kind) {
    case "text":
      return (value ?? field.default).replace(LINE_BREAKS, " ");
    case "area":
      return value ?? field.default;
    case "choice": {
      if (value !== undefined && field.options.includes(value)) return value;
      if (field.options.includes(field.default)) return field.default;
      return field.options[0] ?? "";
    }
    case "toggle":
      return (value ?? field.default) === "yes" ? "yes" : "no";
    default:
      return value ?? "";
  }
}

export type Values = Record<string, string>;

/** The snippet's defaults, for every field ("Start empty"). */
export function defaults(fields: FieldView[]): Values {
  return Object.fromEntries(fields.map((f) => [f.name, fit(f, undefined)]));
}

/** Whether the form opened with remembered answers for any of its fields. */
export function remembered(form: FormOpen): boolean {
  const last = form.lastValues;
  return last !== null && form.fields.some((f) => Object.hasOwn(last, f.name));
}

/** What the fields start with: the remembered answers, else the snippet's defaults. */
export function initialValues(form: FormOpen): Values {
  const last = form.lastValues ?? {};
  return Object.fromEntries(
    form.fields.map((f) => [f.name, fit(f, Object.hasOwn(last, f.name) ? last[f.name] : undefined)]),
  );
}

/** The values sent with Insert: one per field, each made to fit (see `fit`). */
export function submission(fields: FieldView[], values: Values): Values {
  return Object.fromEntries(fields.map((f) => [f.name, fit(f, values[f.name])]));
}

/**
 * The option that typing `typed` jumps to in a list whose choice is `current`: the next option
 * after the current one that starts with it, ignoring case, wrapping around; for one letter
 * typed again, the next option with that letter (spec §5.4: "type its first letters").
 */
export function typeahead(options: string[], current: string, typed: string): string | undefined {
  const query = typed.toLocaleLowerCase();
  if (query === "" || options.length === 0) return undefined;
  const repeated = [...query].every((c) => c === query[0]);
  const start = Math.max(0, options.indexOf(current));
  const order = [...options.slice(start + 1), ...options.slice(0, start + 1)];
  const starts = (o: string, q: string) => o.toLocaleLowerCase().startsWith(q);
  return (
    order.find((o) => starts(o, query)) ??
    (repeated ? order.find((o) => starts(o, query[0] ?? "")) : undefined)
  );
}

/** Key names as each system writes them (design system §4.7). */
export interface Keys {
  cancel: string;
  insert: string;
  insertAnywhere: string;
  startEmpty: string;
  /** `aria-keyshortcuts` values. */
  aria: { cancel: string; insert: string; startEmpty: string };
}

export function keysFor(os: Os): Keys {
  return os === "mac"
    ? {
        cancel: "esc",
        insert: "⏎",
        insertAnywhere: "⌘⏎",
        startEmpty: "⌘⌫",
        aria: { cancel: "Escape", insert: "Enter Meta+Enter", startEmpty: "Meta+Backspace" },
      }
    : {
        cancel: "Esc",
        insert: "Enter",
        insertAnywhere: "Ctrl+Enter",
        startEmpty: "Ctrl+Backspace",
        aria: { cancel: "Escape", insert: "Enter Control+Enter", startEmpty: "Control+Backspace" },
      };
}

/** The copy of the panel (spec §4.4, flow 4). One place, for the message catalogue (3.11). */
export const copy = {
  insert: "Insert",
  cancel: "Cancel",
  goesInto: (app: string) => `Goes into ${app}`,
  lastAnswers: "Your last answers.",
  startEmpty: "Start empty",
  stillHere: "Your answers are still here.",
  couldNotGetBack: (app: string | null) =>
    app ? `Iamb could not get back to ${app}.` : "Iamb could not get back to the app.",
  nothingTyped: "Nothing was typed. Click where the text should go and press Insert again, or cancel.",
  description: (fields: number) => `Fill-in form, ${fields} ${fields === 1 ? "field" : "fields"}`,
} as const;

/** The panel's title: the snippet's label, or its abbreviation when it has none. */
export const titleOf = (form: FormOpen): string => form.label || form.abbreviation;
