// The library window's typed commands: wrappers over the `library_*` Tauri commands of
// src-tauri/src/app/library_commands.rs, and the guards that check what comes back.
//
// The types mirror src-tauri/src/library.rs by hand. `library.fixtures.json` holds one sample of
// each payload, written by `tests/library_contract.rs`; `api.test.ts` runs them through the
// guards, so a change on one side that the other does not follow breaks a test.
//
// Everything here is library content. Nothing logs it.

import { type Invoke, createApi as createBaseApi, invoke } from "../ipc";

export type Trigger = "immediate" | "delimiter";
export type CaseMode = "exact" | "insensitive" | "adaptive";
export type SnippetKind = "plain" | "rich";

/** The eight named tag colours (`color.*.tag` in the design tokens). */
export const TAG_COLORS = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "grey"] as const;
export type TagColor = (typeof TAG_COLORS)[number];

/** A folder tree has at most this many levels (`MAX_FOLDER_DEPTH` of the store). */
export const MAX_FOLDER_DEPTH = 4;

export interface Defaults {
  abbreviationPrefix: string | null;
  trigger: Trigger | null;
  caseMode: CaseMode | null;
}

export interface Folder {
  id: string;
  parentId: string | null;
  name: string;
  position: number;
  defaults: Defaults;
}

export interface Folders {
  profile: string;
  folders: Folder[];
}

export interface Tag {
  id: string;
  name: string;
  color: TagColor;
  snippets: number;
}

export interface Summary {
  id: string;
  folderId: string;
  abbreviation: string;
  label: string;
  firstLine: string;
  kind: SnippetKind;
  trigger: Trigger;
  enabled: boolean;
  tags: string[];
  /** Why it does not expand although it is on: `duplicate` or an `E_…` code. */
  broken: string | null;
  updatedAt: number;
}

export interface SummaryPage {
  revision: number;
  total: number;
  summaries: Summary[];
  next: number | null;
}

export interface Snippet {
  id: string;
  folderId: string;
  abbreviation: string;
  label: string;
  kind: SnippetKind;
  body: string;
  trigger: Trigger;
  caseMode: CaseMode;
  wordStart: boolean;
  enabled: boolean;
  tags: string[];
  rev: number;
  createdAt: number;
  updatedAt: number;
}

export interface Saved {
  id: string;
  rev: number;
  updatedAt: number;
}

export interface AbbreviationCheck {
  conflict: string | null;
  hides: string[];
  hiddenBy: string[];
}

/** A snippet to write; with an `id` it replaces that row or restores a deleted one (Undo). */
export interface SnippetDraft {
  id?: string;
  folderId: string;
  abbreviation: string;
  label: string;
  kind: SnippetKind;
  body: string;
  trigger: Trigger;
  caseMode: CaseMode;
  wordStart: boolean;
  enabled: boolean;
  tags: string[];
}

export interface FolderDraft {
  id?: string;
  parentId: string | null;
  name: string;
  position: number;
  defaults: Defaults;
}

export interface TagDraft {
  id?: string;
  name: string;
  color: TagColor;
}

/** The commands of the library, over `invoke` (Tauri's, or a fake in tests). */
export function createLibraryApi(invoke: Invoke) {
  return {
    listFolders: () => invoke<Folders>("library_list_folders"),
    listTags: () => invoke<Tag[]>("library_list_tags"),
    listSummaries: (offset: number, limit: number) =>
      invoke<SummaryPage>("library_list_summaries", { offset, limit }),
    getSnippet: (id: string) => invoke<Snippet>("library_get_snippet", { id }),
    saveSnippet: (draft: SnippetDraft) => invoke<Saved>("library_save_snippet", { draft }),
    deleteSnippet: (id: string) => invoke<void>("library_delete_snippet", { id }),
    setEnabled: (id: string, enabled: boolean) =>
      invoke<Saved>("library_set_enabled", { id, enabled }),
    saveFolder: (draft: FolderDraft) => invoke<Saved>("library_save_folder", { draft }),
    deleteFolder: (id: string) => invoke<void>("library_delete_folder", { id }),
    saveTag: (draft: TagDraft) => invoke<Saved>("library_save_tag", { draft }),
    deleteTag: (id: string) => invoke<void>("library_delete_tag", { id }),
    checkAbbreviation: (abbreviation: string, trigger: Trigger, snippet?: string) =>
      invoke<AbbreviationCheck>("library_check_abbreviation", {
        abbreviation,
        trigger,
        snippet: snippet ?? null,
      }),
    /** Pause and the rest of the app's own commands, for the status line. */
    setPaused: (paused: boolean) => createBaseApi(invoke).setPaused(paused),
    dismiss: () => createBaseApi(invoke).dismiss(),
  };
}

export type LibraryApi = ReturnType<typeof createLibraryApi>;

/** The commands of the running app. */
export const libraryApi: LibraryApi = createLibraryApi(invoke);

// ---- Guards ---------------------------------------------------------------------------------

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const isNum = (v: unknown): v is number => typeof v === "number";
const isBool = (v: unknown): v is boolean => typeof v === "boolean";
const isStrOrNull = (v: unknown): v is string | null => v === null || isStr(v);
const isStrings = (v: unknown): v is string[] => Array.isArray(v) && v.every(isStr);
const isTrigger = (v: unknown): v is Trigger => v === "immediate" || v === "delimiter";
const isCase = (v: unknown): v is CaseMode =>
  v === "exact" || v === "insensitive" || v === "adaptive";
const isKind = (v: unknown): v is SnippetKind => v === "plain" || v === "rich";
const isColor = (v: unknown): v is TagColor => (TAG_COLORS as readonly unknown[]).includes(v);

function isDefaults(v: unknown): v is Defaults {
  return (
    isObject(v) &&
    isStrOrNull(v.abbreviationPrefix) &&
    (v.trigger === null || isTrigger(v.trigger)) &&
    (v.caseMode === null || isCase(v.caseMode))
  );
}

function isFolder(v: unknown): v is Folder {
  return (
    isObject(v) &&
    isStr(v.id) &&
    isStrOrNull(v.parentId) &&
    isStr(v.name) &&
    isNum(v.position) &&
    isDefaults(v.defaults)
  );
}

export function isFolders(v: unknown): v is Folders {
  return isObject(v) && isStr(v.profile) && Array.isArray(v.folders) && v.folders.every(isFolder);
}

export function isTag(v: unknown): v is Tag {
  return isObject(v) && isStr(v.id) && isStr(v.name) && isColor(v.color) && isNum(v.snippets);
}

export function isSummary(v: unknown): v is Summary {
  return (
    isObject(v) &&
    isStr(v.id) &&
    isStr(v.folderId) &&
    isStr(v.abbreviation) &&
    isStr(v.label) &&
    isStr(v.firstLine) &&
    isKind(v.kind) &&
    isTrigger(v.trigger) &&
    isBool(v.enabled) &&
    isStrings(v.tags) &&
    isStrOrNull(v.broken) &&
    isNum(v.updatedAt)
  );
}

export function isSummaryPage(v: unknown): v is SummaryPage {
  return (
    isObject(v) &&
    isNum(v.revision) &&
    isNum(v.total) &&
    (v.next === null || isNum(v.next)) &&
    Array.isArray(v.summaries) &&
    v.summaries.every(isSummary)
  );
}

export function isSnippet(v: unknown): v is Snippet {
  return (
    isObject(v) &&
    isStr(v.id) &&
    isStr(v.folderId) &&
    isStr(v.abbreviation) &&
    isStr(v.label) &&
    isKind(v.kind) &&
    isStr(v.body) &&
    isTrigger(v.trigger) &&
    isCase(v.caseMode) &&
    isBool(v.wordStart) &&
    isBool(v.enabled) &&
    isStrings(v.tags) &&
    isNum(v.rev) &&
    isNum(v.createdAt) &&
    isNum(v.updatedAt)
  );
}

export function isSaved(v: unknown): v is Saved {
  return isObject(v) && isStr(v.id) && isNum(v.rev) && isNum(v.updatedAt);
}

export function isAbbreviationCheck(v: unknown): v is AbbreviationCheck {
  return (
    isObject(v) && isStrOrNull(v.conflict) && isStrings(v.hides) && isStrings(v.hiddenBy)
  );
}
