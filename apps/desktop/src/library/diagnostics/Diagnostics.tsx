import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { LibraryEnv } from "../env";
import { describeFailure } from "../errors";
import { isMac, rowTitle, templateErrorSentence } from "../model";
import type { Diagnostic, TestResult } from "../settings/api";
import type { SectionId } from "../settings/section";
import { type ReasonAction, render } from "./reasons";
import "./diagnostics.css";

/** The log keeps at most this many entries (the service's ring). */
export const MAX_ENTRIES = 200;

const keyOf = (d: Diagnostic) => `${d.at}|${d.code}|${d.snippet ?? ""}|${d.app ?? ""}`;

/** Newest first, each entry once, at most [`MAX_ENTRIES`]. */
export function mergeEntries(...lists: Diagnostic[][]): Diagnostic[] {
  const seen = new Set<string>();
  const all: Diagnostic[] = [];
  for (const list of lists) {
    for (const d of list) {
      const k = keyOf(d);
      if (!seen.has(k)) {
        seen.add(k);
        all.push(d);
      }
    }
  }
  return all.sort((a, b) => b.at - a.at).slice(0, MAX_ENTRIES);
}

function clock(at: number): string {
  try {
    return new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return "";
  }
}

/** The sentence for the outcome of "Test an abbreviation" (specification §4.8). */
function Outcome({
  result,
  typed,
  enterOrTab,
  onOpen,
}: {
  result: TestResult;
  typed: string;
  enterOrTab: boolean;
  onOpen: (id: string) => void;
}) {
  const name = result.label || result.abbreviation || "A snippet";
  const abbr = result.abbreviation ?? "";
  switch (result.outcome) {
    case "expands": {
      const last = typed.slice(-1);
      const tail = last === " " ? " and the space" : /[\p{L}\p{N}]/u.test(last) || last === "" ? "" : " and the punctuation";
      return (
        <p>
          <strong>Expands.</strong> “{name}” would replace <span className="mono">{abbr}</span>
          {tail}.
        </p>
      );
    }
    case "not_yet":
      return (
        <p>
          <strong>Not yet.</strong> “{name}” waits for a space or punctuation after <span className="mono">{abbr}</span>.
          {enterOrTab ? "" : " Enter and Tab do not count."}
        </p>
      );
    case "broken":
      return (
        <>
          <p>
            <strong>Broken.</strong> “{name}” has an error: {templateErrorSentence(result.error ?? "")}{" "}
            {result.error && <span className="code-small">{result.error}</span>}
          </p>
          {result.snippet && (
            <button type="button" className="button" onClick={() => onOpen(result.snippet ?? "")}>
              Open snippet
            </button>
          )}
        </>
      );
    default:
      return (
        <p>
          <strong>No snippet.</strong> Nothing has the abbreviation <span className="mono">{typed.trim()}</span>.
        </p>
      );
  }
}

/** "Test an abbreviation": says what typing the field's content in an app would do. Typed text is never stored. */
function Tester({
  env,
  api,
  enterOrTab,
  onOpenSnippet,
}: {
  env: LibraryEnv;
  api: NonNullable<LibraryEnv["settings"]>;
  enterOrTab: boolean;
  onOpenSnippet: (id: string) => void;
}) {
  const [typed, setTyped] = useState("");
  const [result, setResult] = useState<{ typed: string; result: TestResult } | { error: string } | null>(null);
  const seq = useRef(0);
  const online = env.connection === "connected";

  useEffect(() => {
    const mine = ++seq.current;
    if (typed === "") {
      setResult(null);
      return;
    }
    const timer = window.setTimeout(() => {
      api.testAbbreviation(typed).then(
        (r) => {
          if (mine === seq.current) setResult({ typed, result: r });
        },
        (e: unknown) => {
          if (mine === seq.current) setResult({ error: describeFailure(e, "dry_run").text });
        },
      );
    }, 150);
    return () => window.clearTimeout(timer);
  }, [typed, api]);

  return (
    <section className="dx-test" aria-labelledby="dx-test-label">
      <label id="dx-test-label" htmlFor="dx-test-input" className="label">
        Test an abbreviation
      </label>
      <input
        id="dx-test-input"
        className="input mono"
        value={typed}
        spellCheck={false}
        autoComplete="off"
        autoCapitalize="off"
        disabled={!online}
        aria-describedby="dx-test-help"
        onChange={(e) => setTyped(e.target.value)}
      />
      <p className="help" id="dx-test-help">
        Say what would happen if you typed this in an app. Nothing is typed, and what you write here is not kept.
      </p>
      <div className="dx-result" role="status">
        {result && "error" in result ? (
          <p className="field-message error">{result.error}</p>
        ) : result ? (
          <Outcome result={result.result} typed={result.typed} enterOrTab={enterOrTab} onOpen={onOpenSnippet} />
        ) : null}
      </div>
    </section>
  );
}

/**
 * "Why didn't it expand?" (specification §4.8): test an abbreviation, then the log of what the
 * service held back, newest first, kept in the service's memory only.
 */
export function Diagnostics({
  env,
  onOpenSnippet,
  onOpenSettings,
}: {
  env: LibraryEnv;
  /** Shows a snippet in the library. */
  onOpenSnippet: (id: string) => void;
  /** Opens Settings at a section. */
  onOpenSettings: (section: SectionId) => void;
}) {
  const api = env.settings;
  if (!api) return <p className="help">This view is not available yet.</p>;
  return <Log env={env} api={api} onOpenSnippet={onOpenSnippet} onOpenSettings={onOpenSettings} />;
}

function Log({
  env,
  api,
  onOpenSnippet,
  onOpenSettings,
}: {
  env: LibraryEnv;
  api: NonNullable<LibraryEnv["settings"]>;
  onOpenSnippet: (id: string) => void;
  onOpenSettings: (section: SectionId) => void;
}) {
  const mac = env.mac ?? isMac();
  const online = env.connection === "connected";
  const [entries, setEntries] = useState<Diagnostic[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [technical, setTechnical] = useState(false);
  const [labels, setLabels] = useState<Record<string, string | null>>({});
  const [enterOrTab, setEnterOrTab] = useState(false);
  const asked = useRef(new Set<string>());
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const load = useCallback(async () => {
    try {
      const fetched = await api.recentDiagnostics();
      if (!alive.current) return;
      // Entries heard live while the list was on its way stay.
      setEntries((live) => mergeEntries(live, fetched));
      setError(null);
    } catch (e) {
      if (alive.current) setError(describeFailure(e, "recent_diagnostics").text);
    }
    try {
      const values = await api.getSettings();
      if (alive.current) setEnterOrTab(/[\n\t]/.test(values.delimiters ?? ""));
    } catch {
      // The sentence of "Not yet" then keeps its default words.
    }
  }, [api]);

  useEffect(() => {
    if (online) void load();
  }, [online, load]);

  useEffect(
    () =>
      env.onEvent((event) => {
        if (event.type === "diagnostic_added") {
          const { code, snippet, app, at } = event;
          setEntries((old) => mergeEntries([{ code, snippet, app, detail: event.detail ?? null, at }], old));
        } else if (event.type === "settings_changed") {
          void api.getSettings().then(
            (v) => alive.current && setEnterOrTab(/[\n\t]/.test(v.delimiters ?? "")),
            () => {},
          );
        }
      }),
    [env, api],
  );

  // A snippet's label for the sentence: library content, looked up once per snippet.
  useEffect(() => {
    for (const e of entries) {
      const id = e.snippet;
      if (!id || asked.current.has(id)) continue;
      asked.current.add(id);
      env.api.getSnippet(id).then(
        (s) => alive.current && setLabels((old) => ({ ...old, [id]: rowTitle({ label: s.label, firstLine: s.body, abbreviation: s.abbreviation }) })),
        () => alive.current && setLabels((old) => ({ ...old, [id]: null })),
      );
    }
  }, [entries, env.api]);

  const rows = useMemo(
    () =>
      entries
        .map((d) => ({
          d,
          r: render(d.code, { label: d.snippet ? (labels[d.snippet] ?? null) : null, app: d.app, mac, detail: d.detail ?? null }),
        }))
        .filter((x) => technical || !x.r.technical),
    [entries, labels, mac, technical],
  );

  const act = (action: ReasonAction, d: Diagnostic) => {
    if (action === "open_snippet" && d.snippet) onOpenSnippet(d.snippet);
    else if (action === "manage_apps") onOpenSettings("apps");
    else if (action === "typing_settings") onOpenSettings("typing");
  };
  const actionLabel = (a: ReasonAction) => (a === "open_snippet" ? "Open snippet" : a === "manage_apps" ? "Manage apps" : "Why?");

  return (
    <div className="dx">
      <Tester env={env} api={api} enterOrTab={enterOrTab} onOpenSnippet={onOpenSnippet} />
      <div className="dx-head">
        <p className="help">Kept in memory only. Iamb never records what you typed.</p>
        <label className="check">
          <input type="checkbox" checked={technical} onChange={(e) => setTechnical(e.target.checked)} />
          <span>Show technical entries</span>
        </label>
      </div>
      {online && env.paused && (
        <div className="banner warning" role="status">
          <p>
            {env.pausedKept
              ? "Iamb is still paused: it has been since before it last started. Typed abbreviations do not expand, and nothing is listed here for them."
              : "Iamb is paused. Typed abbreviations do not expand, and nothing is listed here for them."}
          </p>
        </div>
      )}
      {error && (
        <div className="banner danger" role="alert">
          <p>{error}</p>
          <button type="button" className="button" onClick={() => void load()}>
            Try again
          </button>
        </div>
      )}
      {rows.length === 0 ? (
        !error && <p className="dx-empty">Nothing has been held back since Iamb started.</p>
      ) : (
        <ol className="dx-log" aria-label="Entries, newest first">
          {rows.map(({ d, r }) => {
            const showApp = d.app && !r.title.includes(d.app);
            return (
              <li key={keyOf(d)} className="dx-entry">
                <time dateTime={new Date(d.at).toISOString()}>{clock(d.at)}</time>
                <div className="dx-what">
                  <b>{r.title}</b>
                  {(r.detail || r.code || showApp) && (
                    <span>
                      {r.detail}
                      {r.code && (
                        <>
                          {r.detail ? " " : ""}
                          <span className="code-small">{r.code}</span>
                        </>
                      )}
                      {showApp && (
                        <>
                          {r.detail || r.code ? " · " : ""}
                          {d.app}
                        </>
                      )}
                    </span>
                  )}
                  {r.reason && <span className="code-small dx-reason">{r.reason}</span>}
                </div>
                {r.action && (r.action !== "open_snippet" || d.snippet) ? (
                  <button type="button" className="button" onClick={() => act(r.action as ReasonAction, d)}>
                    {actionLabel(r.action)}
                  </button>
                ) : (
                  <span />
                )}
              </li>
            );
          })}
        </ol>
      )}
      <p className="dx-foot">
        Typed an abbreviation and pressed Enter or Tab? Those keys do not expand a snippet.{" "}
        <button type="button" className="link" onClick={() => onOpenSettings("typing")}>
          Why?
        </button>
      </p>
    </div>
  );
}
