// A fake service for the onboarding tests: the permissions, the status, the test field, the
// login item and the end of onboarding, over memory, around the library's fake (`createFake`).
// Test data is invented; nothing here is a user's text.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect } from "vitest";

import type { ServiceView, StatusView } from "../ipc";
import { type Fake, Failure, createFake } from "../library/testkit";
import type { Finished, Grant, LoginState, OnboardingApi, PermissionKind, Permissions } from "./api";
import type { OnboardingEnv } from "./env";
import { Onboarding } from "./Onboarding";

export { Failure, createFake };

export const SAMPLE_EMAIL = "ines@brindlewood.example";

export interface RigOptions {
  mac?: boolean;
  mode?: "setup" | "fix";
  permissions?: Permissions;
  service?: Partial<ServiceView> | null;
  library?: Fake;
  /** Starts as the service saw it: the hotkey the system refuses (another app owns it). */
  hotkeyRefused?: boolean;
}

const GRANTED: Permissions = { accessibility: "granted", inputMonitoring: "granted" };
export const NOT_REQUIRED: Permissions = { accessibility: "not_required", inputMonitoring: "not_required" };
export const NOTHING_GRANTED: Permissions = { accessibility: "denied", inputMonitoring: "denied" };

export function rig(options: RigOptions = {}) {
  const mac = options.mac ?? false;
  const library = options.library ?? createFake();
  let permissions: Permissions = options.permissions ?? (mac ? NOTHING_GRANTED : NOT_REQUIRED);
  let service: ServiceView | null =
    options.service === null
      ? null
      : {
          version: "0.0.0",
          paused: false,
          suspended: false,
          snippets: 0,
          revision: 1,
          listener: true,
          memoryLocked: true,
          ...options.service,
        };
  const calls: string[] = [];
  const opened: PermissionKind[] = [];
  const testField: boolean[] = [];
  const logins: (boolean | undefined)[] = [];
  const settings: { key: string; value: string | null }[] = [];
  const selections: string[] = [];
  let finished = 0;
  let closed = 0;
  let librariesOpened: boolean[] = [];
  let whyOpened = 0;
  let resumed = 0;
  let restarts = 0;
  let failRestart = false;
  let failDeclare = false;
  let failLogin = false;
  let hotkeyRefused = options.hotkeyRefused ?? false;
  let takenChords = new Set<string>();
  let stored: string | null = null;

  const view = (): StatusView =>
    service ? { connection: "connected", service } : { connection: "disconnected", service: null };

  const api: OnboardingApi = {
    permissions: async () => {
      calls.push("permissions");
      if (!service) throw new Failure("disconnected", "not connected");
      return permissions;
    },
    openPermissionSettings: async (kind) => {
      calls.push("openPermissionSettings");
      opened.push(kind);
    },
    testField: async (on) => {
      calls.push(`testField ${on}`);
      if (on && failDeclare) throw new Failure("unavailable", "this system cannot name the focused window");
      testField.push(on);
    },
    restartHelper: async () => {
      calls.push("restartHelper");
      restarts += 1;
      if (failRestart) throw new Failure("unavailable", "this build cannot restart the background service");
      service = service ?? defaultService();
    },
    finish: async (): Promise<Finished> => {
      calls.push("finish");
      finished += 1;
      if (!service) throw new Failure("disconnected", "not connected");
      return hotkeyRefused ? { paletteHotkey: null, hotkeyRefused: true } : { paletteHotkey: stored ?? "Ctrl+Shift+Space", hotkeyRefused: false };
    },
    loginItem: async (enabled): Promise<LoginState> => {
      calls.push(`loginItem ${String(enabled)}`);
      logins.push(enabled);
      if (failLogin) throw new Failure("unavailable", "the login item");
      return enabled === false ? "disabled" : "enabled";
    },
    openLibrary: async (closeThis = true) => {
      calls.push("openLibrary");
      librariesOpened.push(closeThis);
    },
    close: async () => {
      calls.push("close");
      closed += 1;
    },
  };

  const defaultService = (): ServiceView => ({
    version: "0.0.0",
    paused: false,
    suspended: false,
    snippets: 0,
    revision: 1,
    listener: true,
    memoryLocked: true,
  });

  const env: OnboardingEnv = {
    api,
    library: library.api,
    settings: {
      putSetting: async (key, value) => {
        calls.push(`putSetting ${key}`);
        if (key === "palette_hotkey" && value !== null && takenChords.has(value)) {
          throw new Failure("conflict", "the system refused this shortcut");
        }
        settings.push({ key, value });
        if (key === "palette_hotkey") {
          stored = value;
          hotkeyRefused = false;
        }
      },
    },
    status: async () => view(),
    resume: async () => {
      calls.push("resume");
      resumed += 1;
      if (service) service = { ...service, paused: false };
    },
    mac,
    mode: options.mode ?? "setup",
    openWhy: async () => {
      calls.push("openWhy");
      whyOpened += 1;
    },
    rememberSelection: (id) => {
      selections.push(id);
    },
    timing: { pollMs: 15, nothingMs: 40, graceMs: 40 },
  };

  return {
    env,
    api,
    library,
    calls,
    opened,
    testField,
    logins,
    settings,
    selections,
    get finished() {
      return finished;
    },
    get closed() {
      return closed;
    },
    get librariesOpened() {
      return librariesOpened;
    },
    get whyOpened() {
      return whyOpened;
    },
    get resumed() {
      return resumed;
    },
    get restarts() {
      return restarts;
    },
    setPermissions(p: Permissions) {
      permissions = p;
    },
    grant(kind: PermissionKind) {
      const g: Grant = "granted";
      permissions = kind === "input_monitoring" ? { ...permissions, inputMonitoring: g } : { ...permissions, accessibility: g };
    },
    grantAll() {
      permissions = GRANTED;
    },
    setService(over: Partial<ServiceView> | null) {
      service = over === null ? null : { ...(service ?? defaultService()), ...over };
    },
    failRestart(v = true) {
      failRestart = v;
    },
    failDeclare(v = true) {
      failDeclare = v;
    },
    failLogin(v = true) {
      failLogin = v;
    },
    refuseChord(chord: string) {
      takenChords = new Set([...takenChords, chord]);
    },
    resetCounters() {
      librariesOpened = [];
    },
  };
}

export type Rig = ReturnType<typeof rig>;

export async function renderOnboarding(r: Rig = rig()) {
  const view = render(<Onboarding env={r.env} />);
  await screen.findByRole("heading", { level: 1 });
  return { r, ...view };
}

/** The window's heading, without the screen-reader prefix. */
export const heading = () => screen.getByRole("heading", { level: 1 }).textContent ?? "";

export const field = () => screen.getByLabelText("Test field") as HTMLTextAreaElement;

/** What the service does to the test field: replaces the typed abbreviation with `text`. */
export function serviceTypes(text: string) {
  act(() => {
    fireEvent.change(field(), { target: { value: text } });
  });
}

/** What the person types into the test field (the caret goes to the end). */
export function personTypes(text: string) {
  const el = field();
  act(() => {
    fireEvent.change(el, { target: { value: text } });
    el.setSelectionRange(text.length, text.length);
  });
}

/** Waits for a step by its heading. */
export async function onStep(title: string | RegExp) {
  await waitFor(() => expect(heading()).toMatch(title));
}
