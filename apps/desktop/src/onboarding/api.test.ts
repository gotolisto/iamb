import { describe, expect, it } from "vitest";

import { COMMANDS } from "../ipc";
import { createOnboardingApi, isFinished, isLoginState, isPermissions } from "./api";
import fixtures from "./onboarding.fixtures.json";

describe("the payloads of the onboarding commands (written by tests/onboarding_contract.rs)", () => {
  it("pass the front end's guards", () => {
    expect(isPermissions(fixtures.permissions)).toBe(true);
    expect(isPermissions(fixtures.windows)).toBe(true);
    expect(isFinished(fixtures.finished)).toBe(true);
    expect(isFinished(fixtures.refused)).toBe(true);
    expect(fixtures.login.every(isLoginState)).toBe(true);
  });

  it("are refused when a field is wrong", () => {
    expect(isPermissions({ ...fixtures.permissions, accessibility: "maybe" })).toBe(false);
    expect(isPermissions({ accessibility: "granted" })).toBe(false);
    expect(isFinished({ paletteHotkey: 3, hotkeyRefused: false })).toBe(false);
    expect(isLoginState("sometimes")).toBe(false);
  });
});

describe("the commands", () => {
  it("are all registered on the Rust side, with the arguments the Rust side reads", () => {
    const calls: [string, Record<string, unknown> | undefined][] = [];
    const api = createOnboardingApi(async (command, args) => {
      calls.push([command, args]);
      return undefined as never;
    });
    void api.permissions();
    void api.openPermissionSettings("input_monitoring");
    void api.testField(true);
    void api.restartHelper();
    void api.finish();
    void api.loginItem(true);
    void api.loginItem();
    void api.openLibrary();
    void api.openLibrary(false);
    void api.close();
    expect(calls).toEqual([
      ["onboarding_permissions", undefined],
      ["onboarding_open_permission_settings", { kind: "input_monitoring" }],
      ["onboarding_test_field", { on: true }],
      ["onboarding_restart_helper", undefined],
      ["onboarding_finish", undefined],
      ["onboarding_login_item", { enabled: true }],
      ["onboarding_login_item", { enabled: null }],
      ["onboarding_open_library", { closeThis: true }],
      ["onboarding_open_library", { closeThis: false }],
      ["onboarding_close", undefined],
    ]);
    for (const [command] of calls) expect(COMMANDS as readonly string[]).toContain(command);
  });
});
