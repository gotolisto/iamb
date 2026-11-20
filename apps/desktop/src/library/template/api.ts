// The template editor's way to the service: `ParseTemplate` and `RenderPreview` over the Tauri
// commands of `src-tauri/src/app/template_commands.rs` (types in `src-tauri/src/template.rs`,
// mirrored here by hand). Parsing and rendering happen in the service, with the same code that
// expands (ADR-0017); the editor never parses a template itself.
//
// `src-tauri/tests/front_end_contract.rs` writes sample answers into `contract.fixtures.json`
// (`template`), which `api.test.ts` runs through the guards below.
//
// Privacy: the source and the preview's test values travel only in the command's arguments and
// its answer. Nothing here logs or stores them (UX specification §9).

import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import { isCommandError, type CommandError, type FieldView } from "../../ipc";

/** The names of the template language's macros (template language §2). */
export const MACRO_NAMES = [
  "date",
  "time",
  "clipboard",
  "cursor",
  "snippet",
  "field",
  "key",
  "wait",
  "if",
  "else",
  "endif",
  "lbrace",
] as const;

export type MacroName = (typeof MACRO_NAMES)[number];

export interface TemplateAttribute {
  name: string;
  value: string;
}

/** One token of a template: literal text, or a macro with its attributes. UTF-16 offsets. */
export interface TemplateToken {
  kind: "text" | MacroName;
  start: number;
  end: number;
  attributes: TemplateAttribute[];
}

/** A template error. `start`/`end` are UTF-16 offsets in the source, absent for render-time
 * errors and for errors inside an included snippet (then `snippet` names it). */
export interface TemplateError {
  code: string;
  start: number | null;
  end: number | null;
  snippet: string | null;
}

export interface ParsedTemplate {
  /** Cover the source from start to end, or are empty when it does not split into tokens. */
  tokens: TemplateToken[];
  error: TemplateError | null;
}

export interface PreviewBlank {
  start: number;
  end: number;
  field: string;
  /** No value was given: the text is the field's name, shown as a grey name tag. */
  sample: boolean;
}

export type PreviewPart =
  | { part: "text"; text: string; blanks: PreviewBlank[]; clipboard: Array<[number, number]> }
  | { part: "key"; key: string; shift: boolean; times: number }
  | { part: "wait"; ms: number };

export interface PreviewCursor {
  /** The part; the number of parts when no text surrounds the cursor. */
  part: number;
  offset: number;
}

export interface TemplatePreview {
  parts: PreviewPart[];
  cursor: PreviewCursor | null;
  fields: FieldView[];
  error: TemplateError | null;
}

/** The commands this module calls (also listed in `COMMANDS` of `ipc.ts`). */
export const TEMPLATE_COMMANDS = ["template_parse", "template_preview"] as const;

export type TemplateCommand = (typeof TEMPLATE_COMMANDS)[number];

export type TemplateInvoke = (command: TemplateCommand, args: Record<string, unknown>) => Promise<unknown>;

/** What the editor needs from the service; a fake in tests. */
export interface TemplateService {
  parse(source: string): Promise<ParsedTemplate>;
  preview(source: string, values?: Record<string, string>): Promise<TemplatePreview>;
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isCount = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0;
const isOptCount = (v: unknown) => v === null || isCount(v);

export function isTemplateError(v: unknown): v is TemplateError {
  return (
    isObject(v) &&
    typeof v.code === "string" &&
    isOptCount(v.start) &&
    isOptCount(v.end) &&
    (v.snippet === null || typeof v.snippet === "string")
  );
}

function isAttribute(v: unknown): v is TemplateAttribute {
  return isObject(v) && typeof v.name === "string" && typeof v.value === "string";
}

function isToken(v: unknown): v is TemplateToken {
  return (
    isObject(v) &&
    (v.kind === "text" || (MACRO_NAMES as readonly unknown[]).includes(v.kind)) &&
    isCount(v.start) &&
    isCount(v.end) &&
    Array.isArray(v.attributes) &&
    v.attributes.every(isAttribute)
  );
}

export function isParsedTemplate(v: unknown): v is ParsedTemplate {
  return (
    isObject(v) &&
    Array.isArray(v.tokens) &&
    v.tokens.every(isToken) &&
    (v.error === null || isTemplateError(v.error))
  );
}

function isField(v: unknown): v is FieldView {
  return (
    isObject(v) &&
    typeof v.name === "string" &&
    (v.kind === "text" || v.kind === "area" || v.kind === "choice" || v.kind === "toggle") &&
    Array.isArray(v.options) &&
    v.options.every((o) => typeof o === "string") &&
    typeof v.default === "string"
  );
}

function isPart(v: unknown): v is PreviewPart {
  if (!isObject(v)) return false;
  switch (v.part) {
    case "text":
      return (
        typeof v.text === "string" &&
        Array.isArray(v.blanks) &&
        v.blanks.every(
          (b) =>
            isObject(b) &&
            isCount(b.start) &&
            isCount(b.end) &&
            typeof b.field === "string" &&
            typeof b.sample === "boolean",
        ) &&
        Array.isArray(v.clipboard) &&
        v.clipboard.every((r) => Array.isArray(r) && r.length === 2 && r.every(isCount))
      );
    case "key":
      return typeof v.key === "string" && typeof v.shift === "boolean" && isCount(v.times);
    case "wait":
      return isCount(v.ms);
    default:
      return false;
  }
}

export function isTemplatePreview(v: unknown): v is TemplatePreview {
  return (
    isObject(v) &&
    Array.isArray(v.parts) &&
    v.parts.every(isPart) &&
    (v.cursor === null || (isObject(v.cursor) && isCount(v.cursor.part) && isCount(v.cursor.offset))) &&
    Array.isArray(v.fields) &&
    v.fields.every(isField) &&
    (v.error === null || isTemplateError(v.error))
  );
}

const unexpected: CommandError = {
  code: "unexpected_response",
  message: "the answer did not have the expected shape",
};

/** The service over `invoke` (Tauri's, or a fake in tests). Answers are checked by the guards. */
export function createTemplateService(invoke: TemplateInvoke): TemplateService {
  return {
    async parse(source) {
      const answer = await invoke("template_parse", { source });
      if (!isParsedTemplate(answer)) throw unexpected;
      return answer;
    },
    async preview(source, values) {
      const answer = await invoke("template_preview", { source, values: values ?? null });
      if (!isTemplatePreview(answer)) throw unexpected;
      return answer;
    },
  };
}

/** The service of the running app. */
export const templateService: TemplateService = createTemplateService((command, args) =>
  tauriInvoke(command, args),
);

/** The error code of a failed command, or `internal`. */
export function failureCode(e: unknown): string {
  return isCommandError(e) ? e.code : "internal";
}
