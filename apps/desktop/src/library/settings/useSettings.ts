import { useCallback, useEffect, useRef, useState } from "react";

import type { LibraryEnv } from "../env";
import { type Failure, describeFailure } from "../errors";
import type { Exclusion, SettingsApi } from "./api";

export interface SettingsData {
  /** Null until the first read. */
  values: Record<string, string> | null;
  exclusions: Exclusion[];
  /** The first read failed (the service is not answering). */
  error: string | null;
  reload: () => Promise<void>;
  /**
   * Writes a setting (null removes it). Resolves to null when the service took it, else what
   * went wrong (its `text` is the sentence for the control to show); the stored value is then
   * unchanged.
   */
  save: (key: string, value: string | null) => Promise<Failure | null>;
  /** Adds or removes an exclusion; resolves to null or what went wrong. */
  setExcluded: (appId: string, displayName: string, excluded: boolean) => Promise<Failure | null>;
}

/** Reads the settings and the exclusions, keeps them current, and writes them (task 3.9). */
export function useSettings(env: LibraryEnv, api: SettingsApi): SettingsData {
  const [values, setValues] = useState<Record<string, string> | null>(null);
  const [exclusions, setExclusions] = useState<Exclusion[]>([]);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const reload = useCallback(async () => {
    try {
      const [v, x] = await Promise.all([api.getSettings(), api.listExclusions()]);
      if (!alive.current) return;
      setValues(v);
      setExclusions(x);
      setError(null);
    } catch (e) {
      if (alive.current) setError(describeFailure(e, "get_settings").text);
    }
  }, [api]);

  const online = env.connection === "connected";
  useEffect(() => {
    if (online) void reload();
  }, [online, reload]);

  // Settings changed elsewhere (another window, the menu): read them again.
  useEffect(
    () =>
      env.onEvent((event) => {
        if (event.type === "settings_changed") void reload();
      }),
    [env, reload],
  );

  const save = useCallback(
    async (key: string, value: string | null) => {
      try {
        await api.putSetting(key, value);
      } catch (e) {
        return describeFailure(e, "put_setting");
      }
      if (alive.current) {
        setValues((old) => {
          const next = { ...(old ?? {}) };
          if (value === null) delete next[key];
          else next[key] = value;
          return next;
        });
      }
      return null;
    },
    [api],
  );

  const setExcluded = useCallback(
    async (appId: string, displayName: string, excluded: boolean) => {
      try {
        const rows = await api.putExclusion(appId, displayName, excluded);
        if (alive.current) {
          setExclusions((old) => {
            const gone = new Set(rows.map((r) => r.appId));
            const kept = old.filter((x) => !gone.has(x.appId));
            return excluded ? [...kept, ...rows] : kept;
          });
        }
        return null;
      } catch (e) {
        return describeFailure(e, "put_exclusion");
      }
    },
    [api],
  );

  return { values, exclusions, error, reload, save, setExcluded };
}
