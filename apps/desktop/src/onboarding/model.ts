// The onboarding window's rules and copy (specification §4.6, flow 1), without any rendering: the
// steps of each system, the validation of the first snippet, how the window knows that the test
// field was expanded, and which single cause it names when nothing happened.
//
// What is typed in the test field is passed in and never stored here.

import type { Permissions } from "./api";

export type StepId = "welcome" | "permissions" | "snippet" | "try" | "done";

export const HEADINGS: Record<StepId, string> = {
  welcome: "Type a few letters. Get the whole thing.",
  permissions: "Let Iamb see and type keys",
  snippet: "Make your first snippet",
  try: "Try it here",
  done: "Iamb is running",
};

/** The heading of the "it worked" state of the Try step. */
export const WORKED_HEADING = "That is all there is to it";
/** The heading of the Try step when permissions were skipped (flow 1, "Skip for now"). */
export const CANNOT_EXPAND_HEADING = "Iamb cannot expand yet";

/** The suggested abbreviation and why it starts with `;` (flow 1, step 3). */
export const STARTER_ABBREVIATION = ";em";

/** How long the field may hold an abbreviation and its finishing key before the window asks why. */
export const NOTHING_HAPPENED_MS = 1500;

/** How long both grants may be present with the listener still off before a restart is offered. */
export const LISTENER_GRACE_MS = 5000;

/** How often the permissions are read while the permissions step is shown (specification §11.2, 9). */
export const PERMISSION_POLL_MS = 1000;

/**
 * The steps of this run, in order: the permissions step only where the system asks for
 * permissions and something is missing, the first snippet only when the library has none.
 */
export function stepsFor(options: { permissions: boolean; snippet: boolean }): StepId[] {
  return [
    "welcome",
    ...(options.permissions ? (["permissions"] as const) : []),
    ...(options.snippet ? (["snippet"] as const) : []),
    "try",
    "done",
  ];
}

/** "Step 2 of 5" for the step at `index` of `steps`. */
export function progressLine(index: number, total: number): string {
  return `Step ${index + 1} of ${total}`;
}

/** True when a permission is missing. */
export function anyMissing(p: Permissions): boolean {
  return p.accessibility === "denied" || p.inputMonitoring === "denied";
}

/** True when the system asks for permissions at all (macOS): Windows says `not_required`. */
export function asksForPermissions(p: Permissions): boolean {
  return p.accessibility !== "not_required" || p.inputMonitoring !== "not_required";
}

/** The permission whose settings page opens first: Input Monitoring, then Accessibility. */
export function firstMissing(p: Permissions): "input_monitoring" | "accessibility" | null {
  if (p.inputMonitoring === "denied") return "input_monitoring";
  if (p.accessibility === "denied") return "accessibility";
  return null;
}

export const PERMISSION_ROWS = [
  {
    kind: "input_monitoring",
    name: "Input Monitoring",
    why: "Lets Iamb notice when you type an abbreviation.",
  },
  {
    kind: "accessibility",
    name: "Accessibility",
    why: "Lets Iamb type your snippet for you, and recognise password fields so it can stay out of them.",
  },
] as const;

// ---- The first snippet ----------------------------------------------------------------------

export interface SnippetFields {
  abbreviation: string;
  content: string;
}

/** What is wrong with the first snippet's fields, in the sentence the window shows; null when nothing. */
export function snippetProblem(f: SnippetFields): { field: "abbreviation" | "content"; text: string } | null {
  if (/\s/.test(f.abbreviation.trim()) || /\s/.test(f.abbreviation)) {
    return { field: "abbreviation", text: "An abbreviation cannot contain spaces." };
  }
  if (f.abbreviation === "") {
    return { field: "abbreviation", text: "Type the abbreviation to use." };
  }
  if (f.content.trim() === "") {
    return { field: "content", text: "Type the text Iamb should type for you." };
  }
  return null;
}

/** The sentence for an abbreviation the library already uses. */
export function conflictText(abbreviation: string): string {
  return `${abbreviation} is already used by another snippet. Pick a different abbreviation.`;
}

/** A snippet's label: its text, cut short (the first snippet has no label of its own). */
export function labelOf(content: string): string {
  const line = content.trim().split("\n")[0] ?? "";
  return line.length > 40 ? `${line.slice(0, 39)}…` : line;
}

// ---- The test field -------------------------------------------------------------------------

/** What finishes a word: a space or punctuation (ADR-0026); Enter and Tab never do. */
const FINISHERS = " .,;:!?)]}\"'";

/**
 * The field holds the abbreviation and the key that finishes it (or, for a snippet that expands
 * at once, only the abbreviation): the service should have replaced it by now.
 */
export function holdsUnexpanded(value: string, abbreviation: string, immediate: boolean): boolean {
  if (abbreviation === "") return false;
  if (immediate) return value.endsWith(abbreviation);
  if (value.length <= abbreviation.length) return false;
  return value.endsWith(abbreviation + value.slice(-1)) && FINISHERS.includes(value.slice(-1));
}

/** The field now holds the snippet's text in place of the abbreviation: it expanded. */
export function didExpand(value: string, abbreviation: string, content: string): boolean {
  if (content === "" || abbreviation === "") return false;
  return value.includes(content) && !value.includes(abbreviation);
}

/** The abbreviation was finished with Enter or Tab, which do not expand (ADR-0026). */
export function endsWithAbbreviation(textBeforeCaret: string, abbreviation: string): boolean {
  return abbreviation !== "" && textBeforeCaret.endsWith(abbreviation);
}

export const ENTER_HINT = {
  lead: "Enter does not expand a snippet.",
  rest: " Apps act on Enter at once (they send the message or move on) before Iamb could type. Finish with a space or punctuation, or let this snippet expand without one.",
  action: "Expand without a space",
} as const;

// ---- Nothing happened -----------------------------------------------------------------------

export type Cause =
  | { kind: "no_keyboard"; lead: string; rest: string; action: string }
  | { kind: "paused"; lead: string; rest: string; action: string }
  | { kind: "permission"; lead: string; rest: string; action: string }
  | { kind: "composing"; lead: string; rest: string; action: null }
  | { kind: "other"; lead: string; rest: string; action: string };

export interface Evidence {
  /** The bridge is connected to the service. */
  connected: boolean;
  /** The service's last status; null when it did not answer. */
  service: { listener: boolean; paused: boolean } | null;
  permissions: Permissions | null;
  /** The field saw an input method compose text. */
  composing: boolean;
  mac: boolean;
}

/** The first cause found, in the order of flow 1; always one, with its one button. */
export function firstCause(e: Evidence): Cause {
  if (!e.connected || e.service === null || !e.service.listener) {
    return e.mac
      ? {
          kind: "no_keyboard",
          lead: "Iamb is not seeing the keyboard.",
          rest: " macOS sometimes needs Iamb’s helper to restart after a permission changes.",
          action: "Restart the helper",
        }
      : {
          kind: "no_keyboard",
          lead: "Iamb is not seeing the keyboard.",
          rest: " Its background service is not running.",
          action: "Start it again",
        };
  }
  if (e.service.paused) {
    return { kind: "paused", lead: "Iamb is paused.", rest: "", action: "Resume" };
  }
  if (e.permissions && anyMissing(e.permissions)) {
    return { kind: "permission", lead: "A permission was turned off.", rest: "", action: "Fix" };
  }
  if (e.composing) {
    return {
      kind: "composing",
      lead: "Your keyboard is composing text.",
      rest: " An input method is on. Switch to a plain layout to try.",
      action: null,
    };
  }
  return {
    kind: "other",
    lead: "Something else is in the way.",
    rest: "",
    action: "Open “Why didn’t it expand?”",
  };
}

// ---- Where things are -----------------------------------------------------------------------

export function isMac(platform: string = typeof navigator === "undefined" ? "" : navigator.platform): boolean {
  return /mac|iphone|ipad/i.test(platform);
}

/** Where Iamb lives when the window is closed (step 5). */
export function whereItLives(mac: boolean): string {
  return mac
    ? "It lives in the menu bar, at the top right of your screen."
    : "It lives in the notification area, at the bottom right of your screen.";
}

/** The menu-bar icon or the tray icon, in the words of the point about pausing. */
export function iconName(mac: boolean): string {
  return mac ? "menu-bar icon" : "tray icon";
}

/** The key the library remembers its selection under (src/library/Library.tsx). */
export const LIBRARY_KEY = "iamb.library";
/** The one-shot request for a view (src/library/Library.tsx). */
export const LIBRARY_OPEN_KEY = "iamb.library.open";
