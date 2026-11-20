import { useCallback, useEffect, useRef, useState } from "react";

import type { Health } from "../api";
import type { LibraryEnv } from "../env";

/**
 * The library's health, read again after every library change and whenever `refresh` changes
 * (opening the view: usage changes without a library event). The sidebar's badge reads it too.
 */
export function useHealth(env: LibraryEnv, changes: number, refresh: unknown) {
  const { api, connection } = env;
  const [health, setHealth] = useState<Health | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  const load = useCallback(() => {
    const mine = ++seq.current;
    api.health().then(
      (h) => {
        if (mine !== seq.current) return;
        setHealth(h);
        setError(null);
      },
      () => {
        if (mine !== seq.current) return;
        setError("Library health could not be read.");
      },
    );
  }, [api]);

  useEffect(() => {
    if (connection !== "connected") return undefined;
    // A burst of changes is read once, after the list has settled.
    const timer = setTimeout(load, 60);
    return () => clearTimeout(timer);
  }, [connection, changes, refresh, load]);

  return { health, error, reload: load };
}
