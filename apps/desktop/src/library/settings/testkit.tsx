// A fake service for the Settings and diagnostics tests: the settings, the exclusions and the
// diagnostics over memory, with the rules of the service that the interface relies on (a date
// format with an unknown code is refused, a chord another app holds is a conflict, injection
// settings must be JSON). Test data is invented.

import { render } from "@testing-library/react";

import type { UiEvent } from "../../ipc";
import type { LibraryEnv } from "../env";
import { Library } from "../Library";
import { Failure, createFake, envFor } from "../testkit";
import type { Diagnostic, Exclusion, SettingsApi, TestResult } from "./api";
import { unknownFormatCode } from "./model";

export function createFakeSettings(
  initial: Record<string, string> = {},
  options: { diagnostics?: Diagnostic[]; exclusions?: Exclusion[] } = {},
) {
  const values = new Map<string, string>(Object.entries(initial));
  const exclusions: Exclusion[] = [...(options.exclusions ?? [])];
  const diagnostics: Diagnostic[] = [...(options.diagnostics ?? [])];
  const puts: { key: string; value: string | null }[] = [];
  const takenChords = new Set<string>();
  const tests = new Map<string, TestResult>();
  const calls: string[] = [];
  let down: string | null = null;

  const guard = () => {
    if (down) throw new Failure(down, "the service is not connected");
  };

  const api: SettingsApi = {
    getSettings: async () => {
      calls.push("getSettings");
      guard();
      return Object.fromEntries(values);
    },
    putSetting: async (key, value) => {
      calls.push(`putSetting ${key}`);
      guard();
      if (key === "") throw new Failure("invalid", "the setting key is empty");
      if ((key === "date_format" || key === "time_format") && value !== null) {
        if (value === "" || unknownFormatCode(value) !== null) {
          throw new Failure("invalid", `invalid ${key} setting`);
        }
      }
      if (key === "palette_hotkey" && value !== null) {
        if (takenChords.has(value)) throw new Failure("conflict", "the system refused this shortcut");
        if (!/\+/.test(value)) throw new Failure("invalid", "the palette_hotkey setting is not a valid chord");
      }
      if (key === "injection_overrides" && value !== null) {
        try {
          JSON.parse(value);
        } catch {
          throw new Failure("invalid", "invalid injection_overrides setting: malformed JSON");
        }
      }
      puts.push({ key, value });
      if (value === null) values.delete(key);
      else values.set(key, value);
    },
    listExclusions: async () => {
      calls.push("listExclusions");
      guard();
      return exclusions.map((x) => ({ ...x }));
    },
    putExclusion: async (appId, displayName, excluded) => {
      calls.push(`putExclusion ${excluded}`);
      guard();
      const id = appId.trim().toLowerCase();
      if (id === "" || id.includes("/")) throw new Failure("invalid", "the app id is a path or empty");
      const at = exclusions.findIndex((x) => x.appId === id);
      if (excluded) {
        const row = { appId: id, displayName, createdAt: 1_700_000_000_000 };
        if (at >= 0) exclusions[at] = row;
        else exclusions.push(row);
        return [row];
      }
      if (at < 0) return [];
      return exclusions.splice(at, 1);
    },
    recentDiagnostics: async () => {
      calls.push("recentDiagnostics");
      guard();
      return [...diagnostics].sort((a, b) => b.at - a.at);
    },
    testAbbreviation: async (typed) => {
      calls.push("testAbbreviation");
      guard();
      return (
        tests.get(typed) ?? { outcome: "no_snippet", snippet: null, label: null, abbreviation: null, error: null }
      );
    },
  };

  return {
    api,
    values,
    exclusions,
    diagnostics,
    puts,
    calls,
    /** The system refuses this chord: another app holds it. */
    takeChord: (chord: string) => takenChords.add(chord),
    /** What `testAbbreviation(typed)` answers. */
    answer: (typed: string, result: TestResult) => tests.set(typed, result),
    setDown: (code: string | null) => {
      down = code;
    },
    /** The last stored value of a key. */
    stored: (key: string) => values.get(key),
  };
}

export type FakeSettings = ReturnType<typeof createFakeSettings>;

export function emitEvent(library: ReturnType<typeof createFake>, event: UiEvent): void {
  for (const l of [...library.listeners]) l(event);
}

export interface Rig {
  settings: FakeSettings;
  library: ReturnType<typeof createFake>;
  env: LibraryEnv;
}

export function rig(
  initial: Record<string, string> = {},
  over: Partial<LibraryEnv> = {},
  options: { diagnostics?: Diagnostic[]; exclusions?: Exclusion[] } = {},
): Rig {
  const settings = createFakeSettings(initial, options);
  const library = createFake();
  const env = envFor(library, { settings: settings.api, ...over });
  return { settings, library, env };
}

/** The whole library window over the fake, to test the routing. */
export async function renderWindow(r: Rig) {
  const view = render(<Library env={r.env} />);
  await view.findByRole("navigation", { name: "Library" });
  return view;
}

export const diagnostic = (over: Partial<Diagnostic> & { code: string }): Diagnostic => ({
  snippet: null,
  app: null,
  at: Date.UTC(2026, 10, 20, 14, 30),
  ...over,
});
