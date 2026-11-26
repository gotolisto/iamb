import { describe, expect, it } from "vitest";

import { COMMANDS } from "../../ipc";
import {
  SETTING_KEYS,
  createSettingsApi,
  isDiagnostic,
  isExclusion,
  isSettingsMap,
  isTestResult,
} from "./api";
import fixtures from "./settings.fixtures.json";

describe("the payloads of the settings commands (written by tests/settings_contract.rs)", () => {
  it("pass the front end's guards", () => {
    expect(isSettingsMap(fixtures.settings)).toBe(true);
    expect(fixtures.exclusions.every(isExclusion)).toBe(true);
    expect(fixtures.diagnostics.every(isDiagnostic)).toBe(true);
    expect(isTestResult(fixtures.test)).toBe(true);
  });

  it("are refused when a field is wrong", () => {
    expect(isSettingsMap({ locale: 3 })).toBe(false);
    expect(isExclusion({ ...fixtures.exclusions[0], createdAt: "now" })).toBe(false);
    expect(isDiagnostic({ ...fixtures.diagnostics[0], at: null })).toBe(false);
    expect(isDiagnostic({ ...fixtures.diagnostics[0], code: 4 })).toBe(false);
    expect(isTestResult({ ...fixtures.test, outcome: "maybe" })).toBe(false);
  });
});

describe("the commands", () => {
  it("are all registered on the Rust side", () => {
    const calls: [string, Record<string, unknown> | undefined][] = [];
    const api = createSettingsApi(async (command, args) => {
      calls.push([command, args]);
      return undefined as never;
    });
    void api.getSettings();
    void api.putSetting("locale", "de");
    void api.putSetting("locale", null);
    void api.listExclusions();
    void api.putExclusion("mybank.exe", "My bank", true);
    void api.recentDiagnostics();
    void api.testAbbreviation(";rty");
    // These two check the answer's shape; the stub's empty answer is refused, which is expected
    // here (only the command and its arguments are under test).
    void api.loginItem(false).catch(() => undefined);
    void api.about().catch(() => undefined);
    void api.openLogFolder();
    expect(calls.map(([c]) => c)).toEqual([
      "settings_get",
      "settings_put",
      "settings_put",
      "settings_list_exclusions",
      "settings_put_exclusion",
      "diagnostics_recent",
      "diagnostics_test",
      "settings_login_item",
      "settings_about",
      "settings_open_log_folder",
    ]);
    for (const [command] of calls) expect(COMMANDS as readonly string[]).toContain(command);
    expect(calls[1]?.[1]).toEqual({ key: "locale", value: "de" });
    // Removing a setting sends no value.
    expect(calls[2]?.[1]).toEqual({ key: "locale", value: null });
    expect(calls[4]?.[1]).toEqual({ appId: "mybank.exe", displayName: "My bank", excluded: true });
    expect(calls[7]?.[1]).toEqual({ enabled: false });
  });

  it("name every key once", () => {
    expect(new Set(SETTING_KEYS).size).toBe(SETTING_KEYS.length);
    expect(SETTING_KEYS.filter((k) => k.startsWith("ui."))).toHaveLength(6);
  });
});
