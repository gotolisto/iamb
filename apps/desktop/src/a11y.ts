// Automated accessibility checks for the component tests (task 3.11, docs/ux/spec.md §8).
//
// axe-core runs on the rendered DOM of a view. jsdom has no layout and loads no style sheet, so
// the rules that need either are not run here and are covered elsewhere:
//   - `color-contrast`: the 88 contrast checks of design system §2.2 (`build.py --contrast`), and
//     axe itself in Chromium over the built front end (`scripts/keyboard-run.mjs`).
//   - `scrollable-region-focusable`, `target-size`: the same Chromium run.
// Page-level rules (`region`, `landmark-one-main`, `page-has-heading-one`) judge a whole page;
// a component test renders one window's content, so they are left to the Chromium run too.

import axe from "axe-core";

import { labelInName } from "../scripts/label-in-name.mjs";

/** The rules that cannot give a true answer in jsdom (no layout, no style sheets). */
export const NEEDS_A_BROWSER = [
  "color-contrast",
  "color-contrast-enhanced",
  "scrollable-region-focusable",
  "target-size",
  "link-in-text-block",
  "region",
  "landmark-one-main",
  "page-has-heading-one",
];

export interface Found {
  id: string;
  impact: string;
  help: string;
  nodes: string[];
}

/** Every violation axe finds in `root`, whatever its impact. */
export async function audit(root: Element | Document = document.body): Promise<Found[]> {
  const result = await axe.run(root as axe.ElementContext, {
    runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"] },
    rules: Object.fromEntries(NEEDS_A_BROWSER.map((id) => [id, { enabled: false }])),
    resultTypes: ["violations"],
  });
  return result.violations.map((v) => ({
    id: v.id,
    impact: v.impact ?? "minor",
    help: v.help,
    nodes: v.nodes.map((n) => `${n.target.join(" ")} :: ${n.html.slice(0, 160)}`),
  }));
}

/** What the tests record for the report: the counts by impact of what was found in each state. */
export const tally: Record<string, Record<string, number>> = {};

/**
 * Fails on a violation of impact "critical" or "serious" and on "moderate" ones too: the bar of
 * task 3.11 is zero critical or serious, and the views are held to moderate as well, so only
 * `allowed` rules (named, with the reason at the call) are let through.
 */
export async function expectAccessible(name: string, allowed: string[] = []): Promise<void> {
  const found = (await audit()).filter((f) => !allowed.includes(f.id));
  // 2.5.3 Label in name, by the spec's rule (scripts/label-in-name.mjs).
  const wrong = labelInName(document.body);
  if (wrong.length > 0) found.push({ id: "label-in-name", impact: "serious", help: "The accessible name starts with the visible label", nodes: wrong });
  const counts: Record<string, number> = { critical: 0, serious: 0, moderate: 0, minor: 0 };
  for (const f of found) counts[f.impact] = (counts[f.impact] ?? 0) + f.nodes.length;
  tally[name] = counts;
  if (found.length > 0) {
    const text = found.map((f) => `[${f.impact}] ${f.id}: ${f.help}\n    ${f.nodes.join("\n    ")}`).join("\n");
    throw new Error(`${name}: ${found.length} accessibility violation(s)\n${text}`);
  }
}
