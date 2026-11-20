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

const guardClose: LibraryEnv["guardClose"] = (blocked, onBlocked) => {
  let stop: (() => void) | undefined;
  let removed = false;
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
      guardClose,
      mac: isMac(),
      settings: settingsApi,
      palette: paletteLink,
    }),
    [status.connection, status.service?.paused],
  );
  return <Library env={env} />;
}
