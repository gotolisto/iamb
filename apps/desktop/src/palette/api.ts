// The search palette's commands and event (src-tauri/src/app/palette_panel.rs,
// src-tauri/src/palette.rs, src-tauri/src/palette_data.rs).
//
// The Rust side tells the panel about a palette on `iamb:palette` with nothing but its session;
// the panel fetches what to show with `palette_current`. What the user types is the panel's own
// state: it travels only in the one `palette_search` call per change and is dropped when the
// palette closes (spec §9, architecture §8). Nothing here stores it.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { isCommandError, type Invoke } from "../ipc";
import { isTemplatePreview, type TemplatePreview } from "../library/template/api";

/** The Tauri event that carries a `PaletteEvent`, sent to the palette panel only. */
export const PALETTE_EVENT = "iamb:palette";

/** The Tauri event that tells the library window a request from the palette waits for it. */
export const LIBRARY_REQUEST_EVENT = "iamb:library-request";

/** The code of a pick whose focus could not be given back (`form::FOCUS_LOST`). */
export const FOCUS_LOST = "focus_lost";

/** What the palette shows when it opens (`PaletteOpen` in src-tauri/src/palette.rs). */
export interface PaletteOpen {
  session: number;
  /** Opened from Iamb's own window: a pick selects the snippet in the library. */
  ownWindow: boolean;
  /** The app the text goes into, when the platform can name it. */
  app: string | null;
  /** The label of the snippet whose form was submitted in the last 30 minutes. */
  lastForm: string | null;
}

export type PaletteEvent =
  | { type: "open"; session: number }
  | { type: "shown"; session: number }
  | { type: "closed"; session: number };

export type MatchedField = "abbreviation" | "label" | "body" | "tag";

/** One row (`PaletteRow` in src-tauri/src/palette_data.rs). */
export interface PaletteRow {
  id: string;
  abbreviation: string;
  label: string;
  /** "Replies › Returns". */
  folder: string;
  firstLine: string;
  kind: string;
  enabled: boolean;
  /** `duplicate` or a template error code (`E_…`). */
  broken: string | null;
  /** Expanding it asks for values (a `{field}` or `{if}` anywhere in it). */
  hasFields: boolean;
  needsReview: boolean;
  tags: string[];
}

export interface PaletteHit {
  row: PaletteRow;
  /** The field the ranking matched. */
  field: MatchedField | null;
  /** Matched ranges `[start, end)` in that field, in UTF-16 code units. */
  ranges: Array<[number, number]>;
}

export interface PaletteResults {
  /** How many snippets there are in all ("4 of 76"). */
  total: number;
  hits: PaletteHit[];
}

/** What the user did (`PickAction` in src-tauri/src/app/palette_panel.rs). */
export type PickAction =
  | { kind: "insert"; id: string }
  | { kind: "fill_again" }
  | { kind: "show"; id: string }
  | { kind: "new"; label: string }
  | { kind: "close" };

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const isString = (v: unknown): v is string => typeof v === "string";

export function isPaletteOpen(v: unknown): v is PaletteOpen {
  return (
    isObject(v) &&
    typeof v.session === "number" &&
    typeof v.ownWindow === "boolean" &&
    (v.app === null || isString(v.app)) &&
    (v.lastForm === null || isString(v.lastForm))
  );
}

export function isPaletteEvent(v: unknown): v is PaletteEvent {
  return (
    isObject(v) &&
    (v.type === "open" || v.type === "shown" || v.type === "closed") &&
    typeof v.session === "number"
  );
}

function isRow(v: unknown): v is PaletteRow {
  return (
    isObject(v) &&
    isString(v.id) &&
    isString(v.abbreviation) &&
    isString(v.label) &&
    isString(v.folder) &&
    isString(v.firstLine) &&
    isString(v.kind) &&
    typeof v.enabled === "boolean" &&
    (v.broken === null || isString(v.broken)) &&
    typeof v.hasFields === "boolean" &&
    typeof v.needsReview === "boolean" &&
    Array.isArray(v.tags) &&
    v.tags.every(isString)
  );
}

const isRange = (v: unknown): v is [number, number] =>
  Array.isArray(v) && v.length === 2 && v.every((n) => Number.isInteger(n) && n >= 0);

function isHit(v: unknown): v is PaletteHit {
  return (
    isObject(v) &&
    isRow(v.row) &&
    (v.field === null ||
      v.field === "abbreviation" ||
      v.field === "label" ||
      v.field === "body" ||
      v.field === "tag") &&
    Array.isArray(v.ranges) &&
    v.ranges.every(isRange)
  );
}

export function isPaletteResults(v: unknown): v is PaletteResults {
  return isObject(v) && typeof v.total === "number" && Array.isArray(v.hits) && v.hits.every(isHit);
}

export interface PaletteApi {
  /** The panel's page has loaded and listens: the panel paints once while hidden. */
  ready: () => Promise<void>;
  /** The open palette, if any. */
  current: () => Promise<PaletteOpen | null>;
  /** The panel of `session` painted at `paintedAtMs` (epoch milliseconds) after it was shown. */
  painted: (session: number, paintedAtMs: number) => Promise<void>;
  /** The hits for `query`, with their rows. */
  search: (query: string) => Promise<PaletteResults>;
  /** The snippets used most recently on this computer, newest first (nothing typed yet). */
  recent: () => Promise<PaletteResults>;
  /** What snippet `id` would type now; null when it cannot be shown. */
  preview: (id: string) => Promise<TemplatePreview | null>;
  /** What the user did. Rejects with `FOCUS_LOST` when the focus could not be given back. */
  pick: (session: number, action: PickAction) => Promise<void>;
}

const unexpected = { code: "unexpected_response", message: "the answer did not have the expected shape" };

/** The palette's commands over `invoke` (Tauri's, or a fake in tests). */
export function createPaletteApi(invoke: Invoke): PaletteApi {
  return {
    ready: () => invoke<void>("palette_ready"),
    current: async () => {
      const open = await invoke<unknown>("palette_current");
      return isPaletteOpen(open) ? open : null;
    },
    painted: (session, paintedAtMs) => invoke<void>("palette_painted", { session, paintedAtMs }),
    search: async (query) => {
      const results = await invoke<unknown>("palette_search", { query });
      if (!isPaletteResults(results)) throw unexpected;
      return results;
    },
    recent: async () => {
      const results = await invoke<unknown>("palette_recent");
      if (!isPaletteResults(results)) throw unexpected;
      return results;
    },
    preview: async (id) => {
      const preview = await invoke<unknown>("palette_preview", { id });
      return isTemplatePreview(preview) ? preview : null;
    },
    pick: (session, action) => invoke<void>("palette_pick", { session, action }),
  };
}

/** Whether `e` is the error of a pick whose focus could not be given back. */
export const isFocusLost = (e: unknown): boolean => isCommandError(e) && e.code === FOCUS_LOST;

export type ListenPalette = (handler: (event: PaletteEvent) => void) => Promise<UnlistenFn>;

/** Calls `handler` with each event the Rust side sends this panel. */
export const listenPalette: ListenPalette = (handler) =>
  getCurrentWebviewWindow().listen<unknown>(PALETTE_EVENT, (e) => {
    if (isPaletteEvent(e.payload)) handler(e.payload);
  });

export const paletteApi: PaletteApi = createPaletteApi((command, args) =>
  tauriInvoke(command, args),
);

// ---- What the palette asks of the library window ---------------------------------------------

/** What a pick asked of the library window (`LibraryRequest` in src-tauri/src/palette.rs). */
export type LibraryRequest =
  | { kind: "new"; label: string }
  | { kind: "show"; id: string }
  | { kind: "view"; view: LibraryRequestView };

/** The views the menu's "Settings…" and "Why didn't it expand?" open (`LibraryView`). */
export type LibraryRequestView = "settings" | "diagnostics";

export function isLibraryRequest(v: unknown): v is LibraryRequest {
  return (
    isObject(v) &&
    ((v.kind === "new" && isString(v.label)) ||
      (v.kind === "show" && isString(v.id)) ||
      (v.kind === "view" && (v.view === "settings" || v.view === "diagnostics")))
  );
}

/** The library window's side: take the request that waits, hear that one came, open the palette. */
export interface PaletteLink {
  /** The request a pick left for this window, once. */
  take: () => Promise<LibraryRequest | null>;
  /** Calls `handler` when a request waits; returns the function that stops listening. */
  listen: (handler: () => void) => () => void;
  /** ⌘K / Ctrl+K: opens the palette over the library window. */
  open: () => Promise<void>;
}

export function createPaletteLink(
  invoke: Invoke,
  listen: (handler: () => void) => Promise<UnlistenFn>,
): PaletteLink {
  return {
    take: async () => {
      const request = await invoke<unknown>("library_take_request");
      return isLibraryRequest(request) ? request : null;
    },
    listen: (handler) => {
      const stop = listen(handler);
      return () => void stop.then((f) => f()).catch(() => undefined);
    },
    open: () => invoke<void>("palette_open"),
  };
}

export const paletteLink: PaletteLink = createPaletteLink(
  (command, args) => tauriInvoke(command, args),
  (handler) => getCurrentWebviewWindow().listen(LIBRARY_REQUEST_EVENT, () => handler()),
);
