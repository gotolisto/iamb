// The typed commands of the onboarding window: wrappers over the `onboarding_*` Tauri commands of
// src-tauri/src/app/onboarding_commands.rs, and the guards that check what comes back.
//
// The types mirror src-tauri/src/onboarding.rs by hand. `onboarding.fixtures.json` holds one
// sample of each payload, written by `tests/onboarding_contract.rs`; `api.test.ts` runs them
// through the guards, so a change on one side that the other does not follow breaks a test.
//
// What is typed in the test field never travels through these commands: the window keeps it in
// its own state (architecture §8 rule 3).

import { type Invoke, invoke } from "../ipc";

export type Grant = "granted" | "denied" | "not_required";

export interface Permissions {
  accessibility: Grant;
  inputMonitoring: Grant;
}

/** The permissions the system asks the user to grant, by the name the service uses. */
export type PermissionKind = "input_monitoring" | "accessibility";

export interface Finished {
  /** The search shortcut that is stored; absent when the system refused the default. */
  paletteHotkey: string | null;
  /** The system refused the default shortcut: the window asks for another. */
  hotkeyRefused: boolean;
}

export type LoginState = "enabled" | "disabled" | "requires_approval" | "unavailable";

/** The commands, over `invoke` (Tauri's, or a fake in tests). */
export function createOnboardingApi(invoke: Invoke) {
  return {
    /** The worker's permissions: the worker holds the grants (ADR-0021). */
    permissions: () => invoke<Permissions>("onboarding_permissions"),
    /** Opens the system settings page of a permission. */
    openPermissionSettings: (kind: PermissionKind) =>
      invoke<void>("onboarding_open_permission_settings", { kind }),
    /** Declares this window as the test field, or withdraws it. */
    testField: (on: boolean) => invoke<void>("onboarding_test_field", { on }),
    /** Asks the supervisor to stop and start the worker. */
    restartHelper: () => invoke<void>("onboarding_restart_helper"),
    /** The last step was reached: stores the default shortcut and `ui.onboarding_done`. */
    finish: () => invoke<Finished>("onboarding_finish"),
    /** Registers or removes the login item; no argument only reads it. */
    loginItem: (enabled?: boolean) =>
      invoke<LoginState>("onboarding_login_item", { enabled: enabled ?? null }),
    /** Opens the library window; this one closes too unless `closeThis` is false. */
    openLibrary: (closeThis = true) => invoke<void>("onboarding_open_library", { closeThis }),
    /** Closes this window; Iamb keeps running. */
    close: () => invoke<void>("onboarding_close"),
  };
}

export type OnboardingApi = ReturnType<typeof createOnboardingApi>;

/** The commands of the running app. */
export const onboardingApi: OnboardingApi = createOnboardingApi(invoke);

// ---- Guards ---------------------------------------------------------------------------------

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isGrant = (v: unknown): v is Grant => v === "granted" || v === "denied" || v === "not_required";

export function isPermissions(v: unknown): v is Permissions {
  return isObject(v) && isGrant(v.accessibility) && isGrant(v.inputMonitoring);
}

export function isFinished(v: unknown): v is Finished {
  return (
    isObject(v) &&
    (v.paletteHotkey === null || typeof v.paletteHotkey === "string") &&
    typeof v.hotkeyRefused === "boolean"
  );
}

export function isLoginState(v: unknown): v is LoginState {
  return v === "enabled" || v === "disabled" || v === "requires_approval" || v === "unavailable";
}
