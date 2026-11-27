// The sentences of "Why didn't it expand?" (specification §6.2): one entry for every reason code
// the service emits. A diagnostic carries a code, a snippet, an app and sometimes a technical
// detail of the reason (a code too), never typed text, so a sentence can name only the snippet's
// label and the app (architecture §8 rule 4).
//
// `diagnostics.test.ts` reads the service's sources and fails when a code it emits has no entry
// here, so a new code cannot reach the log as a bare code by accident.

import { templateErrorSentence } from "../model";

/** What an entry of the log can offer to do. */
export type ReasonAction = "open_snippet" | "manage_apps" | "typing_settings";

/** What a sentence may name. */
export interface ReasonContext {
  /** The snippet's label (its abbreviation when it has none); null when it is gone or unnamed. */
  label: string | null;
  /** The app's id or name, when the service named one. */
  app: string | null;
  /** The page runs on macOS. */
  mac: boolean;
  /**
   * The technical detail of the reason, a code: for an unknown focus, why the field could not be
   * judged (`timeout`, `elevated`, `not_inspectable`, `no_focus`, `focus_moved`).
   */
  detail?: string | null;
}

export interface Reason {
  title: (c: ReasonContext) => string;
  detail?: (c: ReasonContext) => string;
  action?: ReasonAction;
  /** Listed only under "Show technical entries", as "Technical: code" (§6.2, last row). */
  technical?: boolean;
}

/** "“Label”", or "A snippet" when the service named none or it was deleted. */
const quoted = (c: ReasonContext) => (c.label ? `“${c.label}”` : "A snippet");
const appName = (c: ReasonContext) => c.app ?? "this app";

const fixed = (title: string, detail?: string, action?: ReasonAction): Reason => ({
  title: () => title,
  ...(detail ? { detail: () => detail } : {}),
  ...(action ? { action } : {}),
});

const technical = (detail: string): Reason => ({
  title: () => "",
  detail: () => detail,
  technical: true,
});

/** The template error codes (specification §6.1), which share one entry. */
export const TEMPLATE_ERROR_CODES = [
  "E_UNCLOSED_MACRO",
  "E_BAD_SYNTAX",
  "E_UNKNOWN_ATTR",
  "E_DUPLICATE_ATTR",
  "E_MISSING_ATTR",
  "E_BAD_VALUE",
  "E_BAD_FORMAT",
  "E_BAD_SHIFT",
  "E_UNBALANCED_IF",
  "E_TOO_DEEP",
  "E_SNIPPET_NOT_FOUND",
  "E_SNIPPET_CYCLE",
  "E_FIELD_CONFLICT",
  "E_FIELD_UNDEFINED",
  "E_FORM_TOO_LARGE",
  "E_DATE_RANGE",
  "E_CURSOR_BEFORE_KEY",
  "E_WAIT_TOTAL",
  "E_OUTPUT_TOO_LARGE",
] as const;

const REASONS: Record<string, Reason> = {
  // Suspensions: what the service held back by itself.
  suspended_secure_field: fixed("Not expanding in a password field", "Iamb stores no keys there."),
  suspended_excluded_app: {
    title: (c) => `Not expanding in ${appName(c)}`,
    detail: () => "You excluded this app.",
    action: "manage_apps",
  },
  suspended_unknown_focus: {
    title: (c) => `Not expanding in ${appName(c)}`,
    // The hint only when Windows said the window's program has a higher privilege level: for
    // any other reason it would send the user looking in the wrong place.
    detail: (c) =>
      `Iamb cannot see into this window, so it stays out.${!c.mac && c.detail === "elevated" ? " It may be running as administrator." : ""}`,
  },
  suspended_composing: fixed("Not expanding while your keyboard composes text", "An input method is on."),

  // Explicit requests (a palette pick, a form, "fill in the last form again") that were refused.
  refused_secure_field: {
    title: (c) => `${quoted(c)} was not inserted: the cursor was in a password field`,
  },
  refused_excluded_app: {
    title: (c) => `${quoted(c)} was not inserted: you excluded ${appName(c)}`,
  },
  refused_unknown_focus: {
    title: (c) => `${quoted(c)} was not inserted: Iamb cannot see into this window`,
  },
  refused_own_window: {
    title: (c) => `${quoted(c)} was not inserted into Iamb's own window`,
  },
  refused_composing: {
    title: (c) => `${quoted(c)} was not inserted: your keyboard was composing text`,
  },
  request_cancelled: {
    title: (c) => `${quoted(c)} was not inserted: you moved to another app first`,
  },

  // Typing.
  injection_busy: {
    title: (c) => `${quoted(c)} was not inserted: Iamb was still typing the previous snippet`,
  },
  injection_cancelled: {
    title: (c) => `${quoted(c)} was stopped halfway`,
    detail: () =>
      "You switched windows or pressed a key that moves the cursor while Iamb was typing. What was already typed stayed where it was.",
  },
  typed_during_injection: {
    title: (c) => `Keys you typed while ${quoted(c)} was being typed may be out of order`,
  },
  // Task 3.8; not in the specification's table, which predates formatted snippets.
  attachment_missing: {
    title: (c) => `${quoted(c)} was typed without its formatting`,
    detail: () =>
      "An image or file it shows is not on this computer, so Iamb typed the plain text instead.",
    action: "open_snippet",
  },

  // Forms.
  form_open: {
    title: (c) => `${quoted(c)} was not started: another fill-in form was still open`,
  },
  form_no_ui_host: {
    title: (c) => `${quoted(c)} could not show its form`,
    detail: () => "Iamb's window process was restarting. Type the abbreviation again.",
  },
  form_not_acknowledged: {
    title: (c) => `${quoted(c)} could not show its form`,
    detail: () => "Iamb's window process was restarting. Type the abbreviation again.",
  },
  // Not in the specification's table: the window process vanished while a form was open. It is
  // the same experience as the two above, so it reads the same way.
  form_ui_host_gone: {
    title: () => "A fill-in form was closed",
    detail: () => "Iamb's window process stopped while the form was open. Nothing was typed. Type the abbreviation again.",
  },
  // Not in the specification's table (the Phase 3 UI review's finding 1): a form whose window
  // was closed, or whose app quit, ends instead of waiting for ever.
  form_target_gone: {
    title: (c) => `${quoted(c)} was not inserted: its window was closed`,
    detail: () => "Your answers are kept for 30 minutes. Fill in the form again where the text should go.",
  },
  form_expired: {
    title: () => "A fill-in form was closed",
    detail: () => "Iamb's window process stopped answering while the form was open. Nothing was typed. Type the abbreviation again.",
  },
  no_last_form: fixed("There is no earlier form to fill in again", "Answers are kept for 30 minutes."),

  unknown_snippet: fixed("The snippet no longer exists"),
  not_a_delimiter: {
    title: (c) => `${quoted(c)} did not expand: Enter and Tab do not count as the end of a word`,
    action: "typing_settings",
  },

  // Technical entries (§6.2, last row; the sentences are this page's, for bug reports).
  keys_for_unreported_window: technical(
    "Keys were typed in a window the system had not reported yet. Iamb did not match them until it caught up.",
  ),
  unexpected_injection_event: technical(
    "A signal arrived that did not belong to the snippet being typed. It was ignored.",
  ),
  unknown_form_session: technical("An answer named a form that was not open. It was ignored."),
  panel_focus_expired: technical(
    "Iamb's window process stopped renewing its hold on the keyboard, so keys count as typed into the app again.",
  ),
  endpoint_unavailable: technical(
    "Iamb's windows could not connect to the service. Expanding goes on; Iamb tries again every 5 seconds.",
  ),
  request_focus_timeout: technical(
    "A request waited for its window to take the focus and gave up. Nothing was typed.",
  ),
  suspended_own_window: technical("One of Iamb's own windows has the focus."),
};

for (const code of TEMPLATE_ERROR_CODES) {
  REASONS[code] = {
    title: (c) => `${quoted(c)} has an error`,
    detail: () => `${templateErrorSentence(code)} Nothing was typed.`,
    action: "open_snippet",
  };
}

/** Every code with a sentence. */
export const KNOWN_CODES: readonly string[] = Object.keys(REASONS);

/** What an entry with this code says; a code this version does not know is technical. */
export function reasonOf(code: string): Reason {
  return (
    REASONS[code] ?? {
      title: () => "",
      detail: () => "This code is not known to this version of Iamb.",
      technical: true,
    }
  );
}

/**
 * The words of the technical details the service sends. A value this version does not know is
 * not shown: the detail is one of a closed set of words, and anything else is not one of them.
 */
const DETAIL_WORDS: Record<string, string> = {
  timeout: "timeout",
  elevated: "elevated",
  not_inspectable: "not inspectable",
  no_focus: "no focus",
  focus_moved: "focus moved",
};

/** "Reason: not inspectable", for an entry with a technical detail; null without one. */
export function reasonLine(detail: string | null | undefined): string | null {
  const words = detail && Object.hasOwn(DETAIL_WORDS, detail) ? DETAIL_WORDS[detail] : undefined;
  return words ? `Reason: ${words}` : null;
}

export interface Rendered {
  title: string;
  detail: string | null;
  action: ReasonAction | null;
  technical: boolean;
  /** The code, shown small for template errors and for technical entries. */
  code: string | null;
  /** The technical detail of the reason ("Reason: timeout"), shown small under the entry. */
  reason: string | null;
}

/** The words of one entry. */
export function render(code: string, c: ReasonContext): Rendered {
  const reason = reasonOf(code);
  const isTemplate = code.startsWith("E_");
  return {
    title: reason.technical ? `Technical: ${code}` : reason.title(c),
    detail: reason.detail ? reason.detail(c) : null,
    action: reason.action ?? null,
    technical: reason.technical === true,
    code: isTemplate ? code : null,
    reason: reasonLine(c.detail),
  };
}
