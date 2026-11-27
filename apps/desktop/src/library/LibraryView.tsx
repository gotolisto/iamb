import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo } from "react";

import { onUiEvent } from "../ipc";
import { afterNextPaint, navigationStartedAtMs, scriptStartedAtMs } from "../paint";
import { api } from "../ipc";
import { useStatus } from "../useStatus";
import { paletteLink } from "../palette/api";
import { libraryApi } from "./api";
import type { LibraryEnv } from "./env";
import { Library } from "./Library";
import { settingsApi } from "./settings";
import { useAppliedTheme } from "./settings/useAppliedTheme";
import { isMac } from "./model";
import { guardUnload } from "../reload";

const guardClose: LibraryEnv["guardClose"] = (blocked, onBlocked) => {
  let stop: (() => void) | undefined;
  let removed = false;
  // A reload the keys did not catch (src/reload.ts) asks too.
  const stopUnload = guardUnload(window, blocked);
  getCurrentWindow()
    .onCloseRequested((event) => {
      if (blocked()) {
        event.preventDefault();
        onBlocked();
      }
    })
    .then(
      (unlisten) => {
        if (removed) unlisten();
        else stop = unlisten;
      },
      () => {},
    );
  return () => {
    removed = true;
    stop?.();
    stopUnload();
  };
};

/** The tray's "Quit Iamb" while this window is open (src-tauri/src/app/tray.rs). */
const QUIT_EVENT = "iamb:quit-requested";

const onQuitRequested: LibraryEnv["onQuitRequested"] = (handler) => {
  let stop: (() => void) | undefined;
  let removed = false;
  getCurrentWindow()
    .listen(QUIT_EVENT, () => handler())
    .then(
      (unlisten) => {
        if (removed) unlisten();
        else stop = unlisten;
      },
      () => {},
    );
  return () => {
    removed = true;
    stop?.();
  };
};

const onEvent: LibraryEnv["onEvent"] = (handler) => {
  const unlisten = onUiEvent(handler);
  return () => {
    void unlisten.then((stop) => stop());
  };
};

/** The library window: reports its first paint (the cold-open measurement) and runs the library. */
export function LibraryView() {
  const status = useStatus();
  useAppliedTheme(settingsApi, status.connection === "connected");

  useEffect(() => {
    afterNextPaint((paintedAtMs) => {
      void api
        .reportFirstPaint({
          navigationAtMs: navigationStartedAtMs(),
          scriptAtMs: scriptStartedAtMs,
          paintedAtMs,
        })
        .catch(() => {});
    });
  }, []);

  const env = useMemo<LibraryEnv>(
    () => ({
      api: libraryApi,
      onEvent,
      connection: status.connection,
      paused: status.service?.paused ?? false,
      pausedKept: status.service?.pausedKept ?? false,
      guardClose,
      mac: isMac(),
      settings: settingsApi,
      palette: paletteLink,
      onQuitRequested,
      quit: () => api.quit(),
      restartService: () => api.restartService(),
    }),
    [status.connection, status.service?.paused, status.service?.pausedKept],
  );
  return <Library env={env} />;
}
