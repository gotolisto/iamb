// What the library needs from the app around it, so that the components run against a fake in
// tests and against the real commands in the window.

import type { ConnectionState, UiEvent } from "../ipc";
import type { PaletteLink } from "../palette/api";
import type { LibraryApi } from "./api";
import type { SettingsApi } from "./settings/api";
import type { TemplateService } from "./template";

export interface LibraryEnv {
  api: LibraryApi;
  /** Calls `handler` with each service event until the returned function is called. */
  onEvent: (handler: (event: UiEvent) => void) => () => void;
  /** The bridge's connection to the service. */
  connection: ConnectionState;
  /** Global pause is on. */
  paused: boolean;
  /**
   * The pause was kept from before the service started: it outlives a quit, a logout and a
   * restart of the machine, so the user may not remember it.
   */
  pausedKept?: boolean;
  /**
   * Asks to be told when the window is about to close. `blocked()` says whether there is
   * something to ask first; if so the window stays open and `onBlocked` is called. The returned
   * function removes the guard.
   */
  guardClose?: ((blocked: () => boolean, onBlocked: () => void) => () => void) | undefined;
  /** The platform's keyboard: macOS writes ⌘ and symbols. */
  mac?: boolean;
  /** Writes text to the clipboard ("Copy content"). */
  copyText?: (text: string) => Promise<void>;
  /** The commands of Settings and "Why didn't it expand?" (task 3.9). */
  settings?: SettingsApi | undefined;
  /** The template commands (`ParseTemplate`, `RenderPreview`); the app's own when absent. */
  template?: TemplateService | undefined;
  /** The search palette: requests it leaves for this window, and ⌘K (task 3.6). */
  palette?: PaletteLink | undefined;
  /**
   * Calls `handler` when "Quit Iamb" was chosen in the tray menu while this window is open: the
   * window asks about unsaved edits first, as its close does, then calls `quit`. Returns the
   * function that stops listening.
   */
  onQuitRequested?: ((handler: () => void) => () => void) | undefined;
  /** Ends Iamb (after `onQuitRequested`). */
  quit?: (() => Promise<void>) | undefined;
  /** "Start it again" in the "Iamb is not running" banner: starts the background service again. */
  restartService?: (() => Promise<void>) | undefined;
}
