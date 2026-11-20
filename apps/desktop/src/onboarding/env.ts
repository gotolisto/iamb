// What the onboarding window needs from the app around it, so that it runs against a fake in tests
// and against the real commands in the window (`OnboardingView.tsx`).

import type { StatusView } from "../ipc";
import { isCommandError } from "../ipc";
import type { LibraryApi } from "../library/api";
import { describeFailure } from "../library/errors";
import type { SettingsApi } from "../library/settings/api";
import type { OnboardingApi } from "./api";

/** What the window needs from the app around it, so that it runs against a fake in tests. */
export interface OnboardingEnv {
  api: OnboardingApi;
  library: Pick<LibraryApi, "listFolders" | "listSummaries" | "getSnippet" | "saveSnippet">;
  settings: Pick<SettingsApi, "putSetting">;
  /** The bridge's state, read afresh. */
  status: () => Promise<StatusView>;
  /** Resumes expansion ("Iamb is paused. [Resume]"). */
  resume: () => Promise<unknown>;
  /** The platform's keyboard: macOS writes ⌘ and symbols, and has the permissions step. */
  mac: boolean;
  /** `setup`: every step; `fix`: the permissions step alone, with "Done". */
  mode: "setup" | "fix";
  /** Asks the library window to open at "Why didn’t it expand?". */
  openWhy: () => Promise<void>;
  /** The library window's remembered selection, written before it opens (a convenience). */
  rememberSelection: (snippet: string) => void;
  /** Timings, replaced in tests. */
  timing?: Partial<{ pollMs: number; nothingMs: number; graceMs: number }>;
}

/** What a failed command says in the window (the library's sentences). */
export function failureText(e: unknown, request: string): string {
  return describeFailure(isCommandError(e) ? e : { code: "internal", message: "" }, request).text;
}
