import { useCallback, useEffect, useState } from "react";

import { listenNotice, noticeApi, type ListenNotice, type NoticeApi } from "./api";
import { noticeText, type Notice } from "./model";
import "./notice.css";

export interface NoticeViewProps {
  api?: NoticeApi;
  listen?: ListenNotice;
}

/**
 * The notice over other apps (spec §4.11, design system §4.9): a refused palette pick or form
 * Insert, in a corner of the screen. The window never takes the keyboard; the Rust side shows
 * it, hides it after 6 s, and says when a newer one is there.
 */
export function NoticeView({ api = noticeApi, listen = listenNotice }: NoticeViewProps) {
  const [notice, setNotice] = useState<Notice | null>(null);
  const load = useCallback(() => {
    api.current().then(setNotice, () => undefined);
  }, [api]);
  useEffect(() => {
    let live = true;
    let stop: (() => void) | undefined;
    void listen(load)
      .then((unlisten) => {
        if (live) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined);
    // The window is made for its first notice: it may be there before this page listened.
    load();
    return () => {
      live = false;
      stop?.();
    };
  }, [listen, load]);
  if (!notice) return <div className="nt-root" />;
  const { title, sentence } = noticeText(notice);
  return (
    <div className="nt-root">
      <div key={notice.id} className="nt" role="alert">
        <p className="nt-title">{title}</p>
        <p className="nt-sentence">{sentence}</p>
      </div>
    </div>
  );
}
