// The soft two-note chime after each finished expansion (`expansion_finished`, task 3.x).
//
// The service says only that an expansion went through (no snippet, no text). One window plays
// the sound, the always-created `form` window, because every window hears every event and the
// chime must not double. The setting `ui.expansion_sound` is on unless it holds `off`.

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { UiEvent } from "../ipc";

/** The stored setting: absent means on, `off` means off (like `ui.tray_icon`). */
export const SOUND_KEY = "ui.expansion_sound";

/** Whether the stored settings ask for the sound. */
export function soundEnabled(values: Record<string, string> | undefined): boolean {
  return values?.[SOUND_KEY] !== "off";
}

export interface SoundDeps {
  /** Subscribes to the bridge's events (`onUiEvent`). */
  listen: (handler: (event: UiEvent) => void) => Promise<UnlistenFn>;
  /** Every setting, by key (`settingsApi.getSettings`). */
  getSettings: () => Promise<Record<string, string>>;
  /** Plays the chime from its start. Must not throw. */
  play: () => void;
}

/**
 * Plays the chime on each `expansion_finished` while the setting allows it. The setting is read
 * at the start and again on every `settings_changed`; until a read has worked, the sound is on
 * (the default), and the next event tries the read again before it decides.
 *
 * Resolves to a function that stops listening.
 */
export async function startExpansionSound(deps: SoundDeps): Promise<() => void> {
  let enabled = true;
  let known = false;
  const refresh = async (): Promise<void> => {
    try {
      enabled = soundEnabled(await deps.getSettings());
      known = true;
    } catch {
      // The service is not there (yet): keep what we have and ask again at the next event.
    }
  };
  await refresh();
  return deps.listen((event) => {
    if (event.type === "settings_changed") {
      void refresh();
    } else if (event.type === "expansion_finished") {
      if (known) {
        if (enabled) deps.play();
      } else {
        void refresh().then(() => {
          if (enabled) deps.play();
        });
      }
    }
  });
}

/** A player for the bundled chime: one element, rewound for each play. */
export function chimePlayer(url: string, create: (url: string) => HTMLAudioElement = (u) => new Audio(u)): () => void {
  let audio: HTMLAudioElement | null = null;
  return () => {
    try {
      audio ??= create(url);
      audio.currentTime = 0;
      void audio.play().catch(() => {});
    } catch {
      // No audio output or a refused play: the chime is a courtesy, never an error.
    }
  };
}
