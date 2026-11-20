import { useMemo } from "react";

import { api as appApi } from "../ipc";
import { libraryApi } from "../library/api";
import { settingsApi } from "../library/settings/api";
import { onboardingApi } from "./api";
import type { OnboardingEnv } from "./env";
import { LIBRARY_KEY, LIBRARY_OPEN_KEY, isMac } from "./model";
import { Onboarding } from "./Onboarding";

/** `?mode=fix` (the menu's "Fix permissions…") shows the permissions step alone. */
export function modeOf(search: string): "setup" | "fix" {
  return new URLSearchParams(search).get("mode") === "fix" ? "fix" : "setup";
}

/** The library window opens with this snippet selected (a convenience: it opens without it). */
export function rememberSelection(storage: Pick<Storage, "getItem" | "setItem">, snippet: string): void {
  try {
    let sort = "label";
    const raw = storage.getItem(LIBRARY_KEY);
    if (raw) {
      const was = JSON.parse(raw) as { sort?: unknown };
      if (was.sort === "changed") sort = "changed";
    }
    storage.setItem(LIBRARY_KEY, JSON.stringify({ sort, view: null, snippet }));
  } catch {
    // No storage: the library opens where it was.
  }
}

/** The library window asked to open at "Why didn’t it expand?" (read once, when it is fresh). */
export function requestDiagnostics(storage: Pick<Storage, "setItem">, at: number): void {
  try {
    storage.setItem(LIBRARY_OPEN_KEY, JSON.stringify({ view: "diagnostics", at }));
  } catch {
    // No storage: the library opens where it was.
  }
}

/** The onboarding window, over the real commands. */
export function OnboardingView() {
  const env = useMemo<OnboardingEnv>(
    () => ({
      api: onboardingApi,
      library: libraryApi,
      settings: settingsApi,
      status: () => appApi.refreshStatus(),
      resume: () => appApi.setPaused(false),
      mac: isMac(),
      mode: modeOf(window.location.search),
      openWhy: async () => {
        requestDiagnostics(window.localStorage, Date.now());
        await onboardingApi.openLibrary(false);
      },
      rememberSelection: (snippet) => rememberSelection(window.localStorage, snippet),
    }),
    [],
  );
  return <Onboarding env={env} />;
}
