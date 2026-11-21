// The front end's only way to the service: typed wrappers over the Tauri commands and events of
// `iamb-desktop` (src-tauri/src/app/commands.rs, src-tauri/src/contract.rs). The front end never
// speaks IPC itself (architecture §6.2, ADR-0017).
//
// The types mirror the Rust ones by hand. `src-tauri/tests/front_end_contract.rs` checks that
// every command and event name here exists on the Rust side and writes the sample payloads in
// `contract.fixtures.json`, which `ipc.test.ts` runs through the guards below: a change on one
// side that the other does not follow breaks a test.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ConnectionState = "connecting" | "connected" | "disconnected";

export interface ServiceView {
  version: string;
  paused: boolean;
  suspended: boolean;
  snippets: number;
  revision: number;
  listener: boolean;
  memoryLocked: boolean;
}

export interface StatusView {
  connection: ConnectionState;
  service: ServiceView | null;
}

export interface SnippetSummary {
  id: string;
  folderId: string;
  abbreviation: string;
  label: string;
  kind: "plain" | "rich";
  enabled: boolean;
}

export interface SnippetDetail extends SnippetSummary {
  body: string;
  trigger: "immediate" | "delimiter";
  rev: number;
}

export interface FieldView {
  name: string;
  kind: "text" | "area" | "choice" | "toggle";
  options: string[];
  default: string;
}

/** The rows a library change touched (`ChangedView` in src-tauri/src/contract.rs). */
export interface ChangedRows {
  snippets: string[];
  folders: string[];
  tags: string[];
}

export type UiEvent =
  | { type: "library_changed"; revision: number; changed: ChangedRows | null }
  | { type: "form_requested"; session: number; snippet: string; fields: FieldView[] }
  | { type: "palette_requested" }
  | {
      type: "diagnostic_added";
      code: string;
      snippet: string | null;
      app: string | null;
      /** UTC milliseconds. */
      at: number;
    }
  | { type: "settings_changed" };

/** The cold-open marks of a window's first paint, in epoch milliseconds (src-tauri/src/timing.rs). */
export interface PaintMarks {
  /** The page's navigation began. */
  navigationAtMs: number;
  /** Its script began to run. */
  scriptAtMs: number;
  /** It painted for the first time. */
  paintedAtMs: number;
}

export interface CommandError {
  code: string;
  message: string;
}

/** The Tauri event that carries a `StatusView`. */
export const STATUS_EVENT = "iamb:status";
/** The Tauri event that carries a `UiEvent`. */
export const UI_EVENT = "iamb:event";

/** Every command `iamb-desktop` registers, by name. */
export const COMMANDS = [
  "status",
  "refresh_status",
  "set_paused",
  "list_snippets",
  "get_snippet",
  "report_first_paint",
  "dismiss",
  "app_quit",
  "library_list_folders",
  "library_list_tags",
  "library_list_summaries",
  "library_get_snippet",
  "library_save_snippet",
  "library_delete_snippet",
  "library_set_enabled",
  "library_save_folder",
  "library_delete_folder",
  "library_save_tag",
  "library_delete_tag",
  "library_check_abbreviation",
  "library_health",
  "library_reset_usage",
  "template_parse",
  "template_preview",
  "template_add_attachment",
  "template_try_form",
  "template_open_link",
  "form_ready",
  "form_current",
  "form_laid_out",
  "form_painted",
  "form_submit",
  "form_cancel",
  "form_abandon",
  "settings_get",
  "settings_put",
  "settings_list_exclusions",
  "settings_put_exclusion",
  "diagnostics_recent",
  "diagnostics_test",
  "onboarding_permissions",
  "onboarding_open_permission_settings",
  "onboarding_test_field",
  "onboarding_restart_helper",
  "onboarding_finish",
  "onboarding_login_item",
  "onboarding_open_library",
  "onboarding_close",
  "app_restart_service",
  "palette_ready",
  "palette_current",
  "palette_painted",
  "palette_search",
  "palette_recent",
  "palette_preview",
  "palette_pick",
  "palette_open",
  "library_take_request",
  "notice_current",
] as const;

export type CommandName = (typeof COMMANDS)[number];

export type Invoke = <T>(command: CommandName, args?: Record<string, unknown>) => Promise<T>;

/** The commands, over `invoke` (Tauri's, or a fake in tests). */
export function createApi(invoke: Invoke) {
  return {
    status: () => invoke<StatusView>("status"),
    refreshStatus: () => invoke<StatusView>("refresh_status"),
    setPaused: (paused: boolean) => invoke<StatusView>("set_paused", { paused }),
    listSnippets: (folder?: string) =>
      invoke<SnippetSummary[]>("list_snippets", { folder: folder ?? null }),
    getSnippet: (id: string) => invoke<SnippetDetail>("get_snippet", { id }),
    reportFirstPaint: (marks: PaintMarks) => invoke<void>("report_first_paint", { ...marks }),
    dismiss: () => invoke<void>("dismiss"),
    /** Ends Iamb: "Quit Iamb", once the library window asked about unsaved edits. */
    quit: () => invoke<void>("app_quit"),
    /** "Start it again": asks the supervisor for the background service, started afresh. */
    restartService: () => invoke<void>("app_restart_service"),
  };
}

export type Api = ReturnType<typeof createApi>;

/** The commands of the running app. */
export const invoke: Invoke = (command, args) => tauriInvoke(command, args);
export const api: Api = createApi(invoke);

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export function isCommandError(v: unknown): v is CommandError {
  return isObject(v) && typeof v.code === "string" && typeof v.message === "string";
}

function isServiceView(v: unknown): v is ServiceView {
  return (
    isObject(v) &&
    typeof v.version === "string" &&
    typeof v.paused === "boolean" &&
    typeof v.suspended === "boolean" &&
    typeof v.snippets === "number" &&
    typeof v.revision === "number" &&
    typeof v.listener === "boolean" &&
    typeof v.memoryLocked === "boolean"
  );
}

export function isStatusView(v: unknown): v is StatusView {
  return (
    isObject(v) &&
    (v.connection === "connecting" ||
      v.connection === "connected" ||
      v.connection === "disconnected") &&
    (v.service === null || isServiceView(v.service))
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

const isStrings = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((s) => typeof s === "string");

function isChangedRows(v: unknown): v is ChangedRows {
  return isObject(v) && isStrings(v.snippets) && isStrings(v.folders) && isStrings(v.tags);
}

export function isUiEvent(v: unknown): v is UiEvent {
  if (!isObject(v)) return false;
  switch (v.type) {
    case "library_changed":
      return typeof v.revision === "number" && (v.changed === null || isChangedRows(v.changed));
    case "form_requested":
      return (
        typeof v.session === "number" &&
        typeof v.snippet === "string" &&
        Array.isArray(v.fields) &&
        v.fields.every(isField)
      );
    case "palette_requested":
    case "settings_changed":
      return true;
    case "diagnostic_added":
      return (
        typeof v.code === "string" &&
        (v.snippet === null || typeof v.snippet === "string") &&
        (v.app === null || typeof v.app === "string") &&
        typeof v.at === "number"
      );
    default:
      return false;
  }
}

/** Calls `handler` with each status the bridge reports. */
export function onStatus(handler: (view: StatusView) => void): Promise<UnlistenFn> {
  return listen<unknown>(STATUS_EVENT, (e) => {
    if (isStatusView(e.payload)) handler(e.payload);
  });
}

/** Calls `handler` with each service event the bridge forwards. */
export function onUiEvent(handler: (event: UiEvent) => void): Promise<UnlistenFn> {
  return listen<unknown>(UI_EVENT, (e) => {
    if (isUiEvent(e.payload)) handler(e.payload);
  });
}
