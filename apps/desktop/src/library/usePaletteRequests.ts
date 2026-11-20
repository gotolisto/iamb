import { useEffect, useRef } from "react";

import type { LibraryRequest, PaletteLink } from "../palette/api";

/** What the library does for a request the palette left (task 3.6, flow 5). */
export interface PaletteRequestHandlers {
  /** "New snippet labelled “…”": a draft whose label is the text. */
  onNew: (label: string) => void;
  /** "Edit", "review", "open", or a pick made from this window: show the snippet. */
  onShow: (id: string) => void;
}

/**
 * Takes the request a pick in the search palette left for this window, when the window starts
 * and each time the palette says one waits. The label of a new snippet is the user's query: it is
 * handed to the draft and held nowhere else.
 */
export function usePaletteRequests(
  link: PaletteLink | undefined,
  ready: boolean,
  handlers: PaletteRequestHandlers,
): void {
  const latest = useRef(handlers);
  latest.current = handlers;
  const readyRef = useRef(ready);
  readyRef.current = ready;
  const waiting = useRef<LibraryRequest | null>(null);

  const run = useRef((request: LibraryRequest) => {
    if (request.kind === "new") latest.current.onNew(request.label);
    else latest.current.onShow(request.id);
  });

  useEffect(() => {
    if (!link) return;
    let live = true;
    const take = () => {
      void link
        .take()
        .then((request) => {
          if (!live || !request) return;
          // A draft needs the folders, which load with the window: wait for them.
          if (readyRef.current) run.current(request);
          else waiting.current = request;
        })
        .catch(() => undefined);
    };
    take();
    const stop = link.listen(take);
    return () => {
      live = false;
      stop();
    };
  }, [link]);

  useEffect(() => {
    if (ready && waiting.current) {
      const request = waiting.current;
      waiting.current = null;
      run.current(request);
    }
  }, [ready]);
}
