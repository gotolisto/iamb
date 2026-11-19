import { useEffect, useState } from "react";

import { api, onStatus, type StatusView } from "./ipc";

const initial: StatusView = { connection: "connecting", service: null };

/** The bridge's status, kept current by `iamb:status`. */
export function useStatus(): StatusView {
  const [status, setStatus] = useState<StatusView>(initial);
  useEffect(() => {
    let live = true;
    const unlisten = onStatus((view) => {
      if (live) setStatus(view);
    });
    api.status().then(
      (view) => {
        if (live) setStatus(view);
      },
      () => {},
    );
    return () => {
      live = false;
      void unlisten.then((stop) => stop());
    };
  }, []);
  return status;
}
