// The typed commands of Settings and "Why didn't it expand?": wrappers over the `settings_*` and
// `diagnostics_*` Tauri commands of src-tauri/src/app/settings_commands.rs, and the guards that
// check what comes back.
//
// The types mirror src-tauri/src/settings.rs by hand. `settings.fixtures.json` holds one sample
// of each payload, written by `tests/settings_contract.rs`; `api.test.ts` runs them through the
// guards, so a change on one side that the other does not follow breaks a test.
//
// Nothing here logs. A diagnostic carries a code, a snippet id, an app and a time; what is typed
// into "Test an abbreviation" travels in one command and is not kept (specification §9).

import { type Invoke, invoke } from "../../ipc";

/** Every setting the service reads (architecture §7), by key. */
export const SERVICE_KEYS = [
  "delimiters",
  "date_format",
  "time_format",
  "locale",
  "palette_hotkey",
  "reopen_form_hotkey",
  "pause_hotkey",
  "injection_overrides",
] as const;

/** The interface's own settings, under the prefix `ui.` (specification §11.1). */
export const UI_KEYS = [
  "ui.onboarding_done",
  "ui.theme",
  "ui.tray_icon",
  "ui.new_trigger",
  "ui.new_case_mode",
  "ui.crash_reports",
] as const;

export const SETTING_KEYS = [...SERVICE_KEYS, ...UI_KEYS] as const;
export type SettingKey = (typeof SETTING_KEYS)[number];

export interface Exclusion {
  appId: string;
  displayName: string;
  createdAt: number;
}

export interface Diagnostic {
  code: string;
  snippet: string | null;
  app: string | null;
  /** UTC milliseconds. */
  at: number;
}

export type TestOutcome = "expands" | "not_yet" | "no_snippet" | "broken";

export interface TestResult {
  outcome: TestOutcome;
  snippet: string | null;
  label: string | null;
  abbreviation: string | null;
  /** The template error code, for `broken`. */
  error: string | null;
}

/** "Start Iamb when I log in" as the system has it (`LoginView` in src-tauri/src/onboarding.rs). */
export type LoginState = "enabled" | "disabled" | "requires_approval" | "unavailable";

/** Settings → About: this build's version and the folder of the log files. */
export interface About {
  version: string;
  logFolder: string;
}

export function isAbout(v: unknown): v is About {
  return typeof v === "object" && v !== null && typeof (v as About).version === "string" && typeof (v as About).logFolder === "string";
}

export function isLoginState(v: unknown): v is LoginState {
  return v === "enabled" || v === "disabled" || v === "requires_approval" || v === "unavailable";
}

/** The commands, over `invoke` (Tauri's, or a fake in tests). */
export function createSettingsApi(invoke: Invoke) {
  return {
    /** Every setting, by key. */
    getSettings: () => invoke<Record<string, string>>("settings_get"),
    /** Sets a setting, or removes it (`null`). Refused with `invalid` or, for a chord, `conflict`. */
    putSetting: (key: string, value: string | null) => invoke<void>("settings_put", { key, value }),
    listExclusions: () => invoke<Exclusion[]>("settings_list_exclusions"),
    /** Adds or removes; answers the row written (as the service spells the id) or the rows removed. */
    putExclusion: (appId: string, displayName: string, excluded: boolean) =>
      invoke<Exclusion[]>("settings_put_exclusion", { appId, displayName, excluded }),
    /** The diagnostics ring, newest first. */
    recentDiagnostics: () => invoke<Diagnostic[]>("diagnostics_recent"),
    /** What typing `typed` would do; nothing is typed or stored. */
    testAbbreviation: (typed: string) => invoke<TestResult>("diagnostics_test", { typed }),
    /** The version and the log folder (Settings → About). */
    about: async () => {
      const about = await invoke<unknown>("settings_about");
      if (!isAbout(about)) throw { code: "unexpected_response", message: "not the about view" };
      return about;
    },
    /** Shows the log folder in Finder or File Explorer. */
    openLogFolder: () => invoke<void>("settings_open_log_folder"),
    /** Registers (true) or removes (false) the login item; no argument only reads it. */
    loginItem: async (enabled?: boolean) => {
      const state = await invoke<unknown>("settings_login_item", { enabled: enabled ?? null });
      if (!isLoginState(state)) throw { code: "unexpected_response", message: "not a login item state" };
      return state;
    },
  };
}

export type SettingsApi = ReturnType<typeof createSettingsApi>;

/** The commands of the running app. */
export const settingsApi: SettingsApi = createSettingsApi(invoke);

// ---- Guards ---------------------------------------------------------------------------------

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const isNum = (v: unknown): v is number => typeof v === "number";
const isStrOrNull = (v: unknown): v is string | null => v === null || isStr(v);

export function isSettingsMap(v: unknown): v is Record<string, string> {
  return isObject(v) && Object.values(v).every(isStr);
}

export function isExclusion(v: unknown): v is Exclusion {
  return isObject(v) && isStr(v.appId) && isStr(v.displayName) && isNum(v.createdAt);
}

export function isDiagnostic(v: unknown): v is Diagnostic {
  return isObject(v) && isStr(v.code) && isStrOrNull(v.snippet) && isStrOrNull(v.app) && isNum(v.at);
}

export function isTestResult(v: unknown): v is TestResult {
  return (
    isObject(v) &&
    (v.outcome === "expands" ||
      v.outcome === "not_yet" ||
      v.outcome === "no_snippet" ||
      v.outcome === "broken") &&
    isStrOrNull(v.snippet) &&
    isStrOrNull(v.label) &&
    isStrOrNull(v.abbreviation) &&
    isStrOrNull(v.error)
  );
}
