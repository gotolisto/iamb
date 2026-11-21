// The snippet the Try step expands: the one the First snippet step saves, or, when the library
// already has snippets, one of those (flow 1: "A library that already has snippets skips step 3").

import type { LibraryApi, SnippetDraft } from "../library/api";
import { labelOf } from "./model";

type Library = Pick<LibraryApi, "listFolders" | "listSummaries" | "getSnippet" | "saveSnippet">;

export interface Example {
  id: string;
  abbreviation: string;
  /** What the abbreviation expands to: plain text on one line. */
  content: string;
  /** The snippet expands the moment the abbreviation is finished, without a space. */
  immediate: boolean;
  /** The row as it was saved, to be saved again with another trigger. */
  draft: SnippetDraft;
}

/** How many summaries are read and how many snippets are opened to find an example. */
const READ = 200;
const OPENED = 5;

function exampleOf(id: string, draft: SnippetDraft): Example {
  return {
    id,
    abbreviation: draft.abbreviation,
    content: draft.body,
    immediate: draft.trigger === "immediate",
    draft: { ...draft, id },
  };
}

/**
 * A snippet of the library that the Try step can expand and recognise: on, plain, one line of
 * text without macros. Null when the library has none, so the First snippet step runs.
 */
export async function pickExample(library: Library): Promise<Example | null> {
  const page = await library.listSummaries(0, READ);
  const usable = page.summaries
    .filter((s) => s.enabled && s.kind === "plain" && s.broken === null && !s.hasFields && !s.firstLine.includes("{") && s.abbreviation !== "")
    .sort((a, b) => Number(b.trigger === "delimiter") - Number(a.trigger === "delimiter"));
  for (const s of usable.slice(0, OPENED)) {
    const full = await library.getSnippet(s.id);
    if (full.body === "" || full.body.includes("\n") || full.body.includes("{")) continue;
    return exampleOf(full.id, {
      folderId: full.folderId,
      abbreviation: full.abbreviation,
      label: full.label,
      kind: full.kind,
      body: full.body,
      trigger: full.trigger,
      caseMode: full.caseMode,
      wordStart: full.wordStart,
      enabled: full.enabled,
      tags: full.tags,
    });
  }
  return null;
}

/**
 * Saves the first snippet (plain text, trigger `delimiter`, case `exact`, word start on) in the
 * root folder, or, when `existing` is the one saved before, writes the new fields over it.
 *
 * @throws what the library commands throw: `conflict` for an abbreviation another snippet has.
 */
export async function saveStarter(
  library: Library,
  abbreviation: string,
  content: string,
  existing: Example | null,
): Promise<Example> {
  let draft: SnippetDraft;
  if (existing) {
    draft = { ...existing.draft, abbreviation, body: content, label: labelOf(content) };
  } else {
    const { folders } = await library.listFolders();
    const root = folders.find((f) => f.parentId === null) ?? folders[0];
    if (!root) throw { code: "not_found", message: "the library has no folder" };
    draft = {
      folderId: root.id,
      abbreviation,
      label: labelOf(content),
      kind: "plain",
      body: content,
      trigger: "delimiter",
      caseMode: "exact",
      wordStart: true,
      enabled: true,
      tags: [],
    };
  }
  const saved = await library.saveSnippet(draft);
  return exampleOf(saved.id, draft);
}

/** Saves the example again: expanding at once (trigger `immediate`, word start off), or after a space. */
export async function setExpandsAtOnce(library: Library, example: Example, immediate: boolean): Promise<Example> {
  const draft: SnippetDraft = {
    ...example.draft,
    trigger: immediate ? "immediate" : "delimiter",
    wordStart: !immediate,
  };
  const saved = await library.saveSnippet(draft);
  return exampleOf(saved.id, draft);
}
