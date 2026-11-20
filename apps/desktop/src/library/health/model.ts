// What the Library health view says (docs/ux/spec.md §4.9), without React.

import type { ExcludedEntry, Health, Summary } from "../api";
import { rowTitle, templateErrorSentence } from "../model";

/** Not used for this many days (the service's rule, `iamb_store::health::UNUSED_DAYS`). */
export const UNUSED_DAYS = 90;

/** The snippets with something to fix, each once: the sidebar's badge. */
export function affectedCount(health: Health | null): number {
  if (!health) return 0;
  const ids = new Set<string>();
  for (const e of health.excluded) ids.add(e.snippet);
  for (const s of health.shadowed) ids.add(s.hidden);
  for (const u of health.unused) ids.add(u.snippet);
  return ids.size;
}

/** An entry has nothing to fix: the whole report is empty. */
export function isClean(health: Health): boolean {
  return affectedCount(health) === 0;
}

/** "“Label”" for a snippet the list knows, else its id's absence is shown as a plain word. */
export function nameOf(row: Summary | undefined): string {
  return row ? `“${rowTitle(row)}”` : "A snippet";
}

/** Whether the entry is a duplicate abbreviation (as opposed to an error in the content). */
export const isTaken = (e: ExcludedEntry): boolean => e.reason === "duplicate";

/** The one-line explanation of an error entry (spec §4.9, first row). */
export function errorSentence(e: ExcludedEntry): string {
  const ref = e.reference ?? "";
  switch (e.reason) {
    case "E_SNIPPET_NOT_FOUND":
      if (e.included) return `It includes ${e.included}, which includes ${ref}, which does not exist.`;
      return `It includes ${ref}, which does not exist.`;
    case "E_SNIPPET_CYCLE":
      if (e.included) return `It includes ${e.included}, which includes itself${ref ? ` through ${ref}` : ""}.`;
      return ref ? `It includes itself, through ${ref}.` : "It includes itself.";
    case "E_TOO_DEEP":
      if (ref) return `Snippets inside snippets go more than 8 deep, at ${ref}.`;
      break;
    default:
  }
  const base = templateErrorSentence(e.reason);
  return e.included ? `It includes ${e.included}, which has an error. ${base}` : base;
}

/** "last used 14 December", with the year when it is not this one; never-used says so. */
export function lastUsedText(lastUsed: number | null, now: Date = new Date()): string {
  if (lastUsed === null) return "never used on this computer";
  const d = new Date(lastUsed);
  const sameYear = d.getFullYear() === now.getFullYear();
  const text = d.toLocaleDateString("en-GB", sameYear ? { day: "numeric", month: "long" } : { day: "numeric", month: "long", year: "numeric" });
  return `last used ${text}`;
}

export const copy = {
  title: "Library health",
  clean: (n: number) => `Nothing to fix. All ${n} ${n === 1 ? "snippet" : "snippets"} can expand.`,
  errors: "Will not expand: error in the snippet",
  taken: "Will not expand: abbreviation taken",
  never: "Never reached",
  unused: "Not used on this computer for 90 days",
  unusedNote: "Counted on this computer only.",
  unavailable: "Library health could not be read.",
  tryAgain: "Try again",
  open: "Open snippet",
  turnOff: "Turn off",
  delete: "Delete",
  taken_: (abbr: string) => `${abbr} is already used by another snippet, so this one does not expand.`,
  neverReached: (by: string) => `${by} expands immediately and fires first.`,
  badge: (n: number) => `${n} to fix`,
} as const;
