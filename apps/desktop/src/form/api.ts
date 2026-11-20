// The fill-in form panel's commands and event (src-tauri/src/app/form_panel.rs, src-tauri/src/form.rs).
//
// The Rust side tells the panel about a form on `iamb:form` with nothing but its session; the
// panel fetches the form, values included, with `form_current`, which answers the form window
// only. The values live in the panel's state until the form closes (spec §9).

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { isCommandError, type FieldView, type Invoke } from "../ipc";

/** The Tauri event that carries a `FormEvent`, sent to the form panel only. */
export const FORM_EVENT = "iamb:form";

/** The code of an Insert whose focus could not be given back (`form::FOCUS_LOST`). */
export const FOCUS_LOST = "focus_lost";

/** A form to fill in (`FormOpen` in src-tauri/src/form.rs). */
export interface FormOpen {
  session: number;
  snippet: string;
  /** The snippet's label; empty when it has none or could not be read. */
  label: string;
  /** Its abbreviation; empty when it could not be read. */
  abbreviation: string;
  /** The app the text goes into, when the platform can name it. */
  app: string | null;
  fields: FieldView[];
  /** The values last submitted for this snippet (user-entered text), or null. */
  lastValues: Record<string, string> | null;
}

export type FormEvent =
  | { type: "open"; session: number }
  | { type: "shown"; session: number }
  | { type: "closed"; session: number };

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

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

function isValues(v: unknown): v is Record<string, string> {
  return isObject(v) && Object.values(v).every((s) => typeof s === "string");
}

export function isFormOpen(v: unknown): v is FormOpen {
  return (
    isObject(v) &&
    typeof v.session === "number" &&
    typeof v.snippet === "string" &&
    typeof v.label === "string" &&
    typeof v.abbreviation === "string" &&
    (v.app === null || typeof v.app === "string") &&
    Array.isArray(v.fields) &&
    v.fields.every(isField) &&
    (v.lastValues === null || isValues(v.lastValues))
  );
}

export function isFormEvent(v: unknown): v is FormEvent {
  return (
    isObject(v) &&
    (v.type === "open" || v.type === "shown" || v.type === "closed") &&
    typeof v.session === "number"
  );
}

export interface FormApi {
  /** The panel's page has loaded and listens: the panel paints once while hidden. */
  ready: () => Promise<void>;
  /** The open form, if any. */
  current: () => Promise<FormOpen | null>;
  /** The form of `session` is rendered; its content is `height` CSS pixels high. */
  laidOut: (session: number, height: number) => Promise<void>;
  /** The panel of `session` painted at `paintedAtMs` (epoch milliseconds) after it was shown. */
  painted: (session: number, paintedAtMs: number) => Promise<void>;
  /** Insert. Rejects with `FOCUS_LOST` when the focus could not be given back. */
  submit: (session: number, values: Record<string, string>) => Promise<void>;
  /** Cancel. */
  cancel: (session: number) => Promise<void>;
}

/** The form's commands over `invoke` (Tauri's, or a fake in tests). */
export function createFormApi(invoke: Invoke): FormApi {
  return {
    ready: () => invoke<void>("form_ready"),
    current: async () => {
      const form = await invoke<unknown>("form_current");
      return isFormOpen(form) ? form : null;
    },
    laidOut: (session, height) => invoke<void>("form_laid_out", { session, height }),
    painted: (session, paintedAtMs) => invoke<void>("form_painted", { session, paintedAtMs }),
    submit: (session, values) => invoke<void>("form_submit", { session, values }),
    cancel: (session) => invoke<void>("form_cancel", { session }),
  };
}

/** Whether `e` is the error of an Insert whose focus could not be given back. */
export const isFocusLost = (e: unknown): boolean => isCommandError(e) && e.code === FOCUS_LOST;

export type ListenForm = (handler: (event: FormEvent) => void) => Promise<UnlistenFn>;

/** Calls `handler` with each event the Rust side sends this panel. */
export const listenForm: ListenForm = (handler) =>
  getCurrentWebviewWindow().listen<unknown>(FORM_EVENT, (e) => {
    if (isFormEvent(e.payload)) handler(e.payload);
  });

export const formApi: FormApi = createFormApi((command, args) => tauriInvoke(command, args));
