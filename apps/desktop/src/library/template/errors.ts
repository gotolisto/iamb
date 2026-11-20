// The sentences for template errors (UX specification §6.1; flow 2, "Errors and recovery").
// The code is shown small after the sentence. Sentences never quote what the user typed beyond
// the names the specification puts in them (a field's name, a snippet's abbreviation).

import type { TemplateError } from "./api";
import { attr, type Macro } from "./model";

/** A run of a sentence; `strong` runs are bold (an abbreviation). */
export interface Run {
  text: string;
  strong?: boolean;
}

/** What the message offers to do about the error. */
export type ErrorAction =
  | { kind: "insert-endif"; label: string }
  | { kind: "open-chip"; label: string };

export interface ErrorMessage {
  runs: Run[];
  code: string;
  action?: ErrorAction;
}

const PLAIN: Record<string, string> = {
  E_UNCLOSED_MACRO: "Something that starts with { is never closed. Add the closing } or delete it.",
  E_BAD_SYNTAX: "This part is not written correctly. Open it and set its values again.",
  E_UNKNOWN_ATTR: "This part has a setting it does not understand. Open it and set its values again.",
  E_DUPLICATE_ATTR: "This part has the same setting twice. Open it and set its values again.",
  E_MISSING_ATTR:
    "This part is missing something: a fill-in field needs a name, a key press needs a key.",
  E_BAD_VALUE: "This part has a value that is not allowed.",
  E_BAD_FORMAT: "Iamb does not know this date or time code. See the list of codes.",
  E_BAD_SHIFT:
    "The shift is not written correctly. Use a sign, a number and a unit, for example +3 days.",
  E_TOO_DEEP: "Optional parts, or snippets inside snippets, are nested more than 8 deep.",
  E_FORM_TOO_LARGE: "This snippet asks for more than 100 values.",
  E_DATE_RANGE: "The shifted date falls outside the years 1 to 9999.",
  E_CURSOR_BEFORE_KEY:
    "A key press comes after the cursor position. Move the cursor position to the end, or remove the key press: Iamb cannot know where the key leaves the cursor.",
  E_WAIT_TOTAL: "The waits add up to more than 30 seconds.",
  E_OUTPUT_TOO_LARGE: "The text is longer than 100,000 characters.",
};

/** The sentence for `error`. `at` is the macro the error's position is in, when it is in one:
 * it names the field or the snippet, and tells the three cases of `E_UNBALANCED_IF` apart. */
export function errorMessage(error: TemplateError, at: Macro | null): ErrorMessage {
  const code = error.code;
  const msg = (runs: Run[], action?: ErrorAction): ErrorMessage =>
    action ? { runs, code, action } : { runs, code };
  const inner = error.snippet && error.start === null ? error.snippet : null;
  switch (code) {
    case "E_UNBALANCED_IF":
      if (at?.name === "endif") return msg([{ text: "This “End if” has no start." }]);
      if (at?.name === "else") return msg([{ text: "This “Else” is outside an optional part." }]);
      if (inner) return withSnippet(msg([{ text: "This optional part has a start but no end." }]), inner);
      return msg(
        [
          {
            text: "This optional part has a start but no end. Put the caret where it should stop and insert “End if”.",
          },
        ],
        { kind: "insert-endif", label: "Insert it at the end" },
      );
    case "E_SNIPPET_NOT_FOUND": {
      const abbr = (at?.name === "snippet" ? attr(at, "abbr") : undefined) ?? error.snippet ?? "";
      return msg(
        [{ text: "It includes the snippet " }, { text: abbr, strong: true }, { text: ", which does not exist." }],
        at?.name === "snippet" ? { kind: "open-chip", label: "Choose another" } : undefined,
      );
    }
    case "E_SNIPPET_CYCLE": {
      const abbr = error.snippet ?? (at?.name === "snippet" ? attr(at, "abbr") : undefined) ?? "";
      return msg([
        { text: "It includes itself, through " },
        { text: abbr, strong: true },
        { text: ". A snippet cannot contain itself." },
      ]);
    }
    case "E_FIELD_CONFLICT": {
      const name = (at?.name === "field" ? attr(at, "name") : undefined) ?? "";
      return msg([
        {
          text: `Two fill-in fields are both called “${name}” but are of different kinds. Give one another name, or make them the same.`,
        },
      ]);
    }
    case "E_FIELD_UNDEFINED": {
      const name = (at?.name === "if" ? attr(at, "field") : undefined) ?? "";
      return msg([
        {
          text: `This optional part looks at “${name}”, but there is no fill-in field with that name.`,
        },
      ]);
    }
    default: {
      const sentence = PLAIN[code] ?? "This snippet has an error.";
      const base = msg([{ text: sentence }]);
      return inner ? withSnippet(base, inner) : base;
    }
  }
}

/** For an error inside an included snippet: says which one holds it. */
function withSnippet(m: ErrorMessage, abbr: string): ErrorMessage {
  return {
    ...m,
    runs: [...m.runs, { text: " It is in the included snippet " }, { text: abbr, strong: true }, { text: "." }],
  };
}

/** The sentence as plain text (for tests and accessible names). */
export function messageText(m: ErrorMessage): string {
  return m.runs.map((r) => r.text).join("");
}
