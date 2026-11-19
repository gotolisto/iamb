import { useEffect, useState } from "react";

import { api, isCommandError, onUiEvent, type SnippetSummary } from "../ipc";
import { afterNextPaint, navigationStartedAtMs, scriptStartedAtMs } from "../paint";
import { statusText } from "../status";
import { useStatus } from "../useStatus";

/** The library window. A placeholder until the library itself lands (architecture §11, 3.3). */
export function LibraryView() {
  const status = useStatus();
  const [snippets, setSnippets] = useState<SnippetSummary[]>([]);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const connected = status.connection === "connected";

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

  useEffect(() => {
    const unlisten = onUiEvent((event) => {
      if (event.type === "library_changed") setRevision(event.revision);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    if (!connected) return;
    let live = true;
    api.listSnippets().then(
      (list) => {
        if (live) setSnippets(list);
      },
      (e: unknown) => {
        if (live) setError(isCommandError(e) ? e.message : "The snippets could not be loaded.");
      },
    );
    return () => {
      live = false;
    };
  }, [connected, revision]);

  const togglePause = () => {
    void api.setPaused(!status.service?.paused).catch((e: unknown) => {
      setError(isCommandError(e) ? e.message : "Pause could not be changed.");
    });
  };

  return (
    <main className="window library">
      <header className="bar">
        <h1 className="title">Iamb</h1>
        <p className="status" data-state={status.connection}>
          {statusText(status)}
        </p>
        <button type="button" className="button" disabled={!connected} onClick={togglePause}>
          {status.service?.paused ? "Resume expansion" : "Pause expansion"}
        </button>
      </header>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <section className="content" aria-label="Snippets">
        {connected ? (
          <ul className="list">
            {snippets.map((s) => (
              <li key={s.id} className="row">
                <code className="abbr">{s.abbreviation}</code>
                <span className="label">{s.label}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="muted">Your snippets appear here while the Iamb service runs.</p>
        )}
      </section>
    </main>
  );
}
