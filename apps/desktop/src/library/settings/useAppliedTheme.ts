import { useEffect } from "react";

import type { SettingsApi } from "./api";
import { applyTheme, themeOf } from "./theme";

/** Shows the window in the stored appearance as soon as the service answers. */
export function useAppliedTheme(api: SettingsApi, online: boolean): void {
  useEffect(() => {
    if (!online) return;
    let live = true;
    api.getSettings().then(
      (values) => {
        if (live) applyTheme(themeOf(values["ui.theme"]));
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [api, online]);
}
