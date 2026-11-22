import { type FormEvent, type KeyboardEvent, type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import type { StatusView } from "../ipc";
import { isCommandError } from "../ipc";
import { Icon } from "../library/Icon";
import { chordBadges, chordSpoken, recordChord, refusedText } from "../library/settings/model";
import type { Finished, LoginState, PermissionKind, Permissions } from "./api";
import type { Example } from "./example";
import {
  type Cause,
  ENTER_HINT,
  PERMISSION_ROWS,
  STARTER_ABBREVIATION,
  WORKED_HEADING,
  anyMissing,
  conflictText,
  didExpand,
  endsWithAbbreviation,
  firstCause,
  firstMissing,
  holdsUnexpanded,
  iconName,
  snippetProblem,
  whereItLives,
} from "./model";
import { type OnboardingEnv, failureText } from "./env";

/** Attributes that keep the web view from remembering or correcting what is typed (specification §9). */
const PRIVATE = {
  autoComplete: "off",
  autoCorrect: "off",
  autoCapitalize: "off",
  spellCheck: false,
} as const;

function Code({ children }: { children: ReactNode }) {
  return <span className="ob-code">{children}</span>;
}

function Enter() {
  return <kbd aria-hidden="true">⏎</kbd>;
}

// ---- 1. Welcome -----------------------------------------------------------------------------

export function WelcomeStep(props: {
  heading: ReactNode;
  ready: boolean;
  serviceDown: boolean;
  restartError: boolean;
  onRestart: () => void;
  onStart: () => void;
}) {
  const { heading, ready, serviceDown, restartError, onRestart, onStart } = props;
  // The keyboard goes to the main button as soon as it can be pressed.
  const main = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (ready || serviceDown) main.current?.focus();
  }, [ready, serviceDown]);
  return (
    <>
      <div className="ob-stack">
        {heading}
        <p className="ob-lead">
          Save the text you type again and again as a snippet. Type its abbreviation in any app and Iamb replaces it.
        </p>
      </div>
      <div className="ob-demo" role="img" aria-label="Example: typing ;em gives ines@brindlewood.example">
        <Code>;em</Code>
        <Icon name="right" />
        <span>ines@brindlewood.example</span>
      </div>
      <ul className="ob-points">
        <li>
          <Icon name="shield" />
          <div>
            <b>What you type stays on this computer</b>
            <span>
              Iamb holds your last few keys in memory to spot an abbreviation, then forgets them. It never saves or
              sends what you type.
            </span>
          </div>
        </li>
        <li>
          <Icon name="lock" />
          <div>
            <b>It stays out of password fields</b>
            <span>And out of any app you tell it to leave alone.</span>
          </div>
        </li>
      </ul>
      {serviceDown ? (
        <div className="ob-banner bad" role="alert">
          <Icon name="warn" />
          <span className="ob-txt">
            Iamb’s background service is not running.
            {restartError ? " Iamb could not start it from here. Quit Iamb from its menu and open it again." : ""}
          </span>
        </div>
      ) : null}
      <div className="ob-actions">
        <span className="ob-grow" />
        {serviceDown ? (
          <button type="button" className="ob-btn ob-primary" onClick={onRestart} ref={main}>
            Start it again
          </button>
        ) : (
          <button type="button" className="ob-btn ob-primary" onClick={onStart} disabled={!ready} ref={main}>
            Get started <Enter />
          </button>
        )}
      </div>
    </>
  );
}

// ---- 2. Permissions (macOS) -----------------------------------------------------------------

export function PermissionsStep(props: {
  heading: ReactNode;
  permissions: Permissions | null;
  service: StatusView | null;
  fix: boolean;
  graceMs: number;
  restartError: boolean;
  onOpen: (kind: PermissionKind) => void;
  onRestart: () => void;
  onBack: (() => void) | null;
  onSkip: (() => void) | null;
  onContinue: () => void;
}) {
  const { heading, permissions, service, fix, graceMs, restartError, onOpen, onRestart, onBack, onSkip, onContinue } = props;
  const granted = permissions !== null && !anyMissing(permissions);
  const missing = permissions ? firstMissing(permissions) : "input_monitoring";
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const proceed = useRef<HTMLButtonElement>(null);

  // The keyboard goes to the first missing permission's button, and to Continue when both arrive.
  const focused = useRef<"none" | "missing" | "continue">("none");
  useEffect(() => {
    if (granted && focused.current !== "continue") {
      focused.current = "continue";
      proceed.current?.focus();
    } else if (!granted && permissions && missing && focused.current === "none") {
      focused.current = "missing";
      buttons.current.get(missing)?.focus();
    }
  }, [granted, permissions, missing]);

  // Both grants present but the listener still off: the helper has to restart (flow 1, errors).
  const [stuck, setStuck] = useState(false);
  const listening = service?.service?.listener ?? true;
  useEffect(() => {
    if (!granted || listening) {
      setStuck(false);
      return;
    }
    const timer = setTimeout(() => setStuck(true), graceMs);
    return () => clearTimeout(timer);
  }, [granted, listening, graceMs]);

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    const t = e.target as HTMLElement;
    if (e.key === "Enter" && missing && (t === e.currentTarget || t.tagName === "BODY" || t.tagName === "MAIN")) {
      onOpen(missing);
    }
  };

  return (
    // The Enter key opens the settings page of the first permission still missing.
    <section className="ob-step" onKeyDown={onKeyDown}>
      <div className="ob-stack">
        {heading}
        <p className="ob-lead">
          {granted
            ? "Both are on. You can change them at any time in System Settings."
            : "macOS asks before any app may do this. Turn on Iamb in two lists. This window updates by itself."}
        </p>
      </div>
      <div className="ob-stack">
        {PERMISSION_ROWS.map((row) => {
          const state = permissions ? (row.kind === "input_monitoring" ? permissions.inputMonitoring : permissions.accessibility) : "denied";
          const on = state !== "denied";
          const first = missing === row.kind;
          return (
            <div key={row.kind} className={on ? "ob-perm ok" : "ob-perm"}>
              <span className="ob-ic">
                <Icon name={on ? "check" : row.kind === "input_monitoring" ? "eye" : "key"} />
              </span>
              <div className="ob-t">
                <b>{row.name}</b>
                <span>{row.why}</span>
              </div>
              <div className="ob-do">
                <span className="ob-state" role="status">
                  <span className="ob-sr">{row.name}: </span>
                  {on ? "Granted" : "Not granted yet"}
                </span>
                {on ? null : (
                  <button
                    type="button"
                    className={first ? "ob-btn ob-primary" : "ob-btn"}
                    ref={(el) => {
                      if (el) buttons.current.set(row.kind, el);
                      else buttons.current.delete(row.kind);
                    }}
                    onClick={() => onOpen(row.kind)}
                  >
                    Open settings {first ? <Enter /> : null}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {granted ? null : (
        <p className="ob-help">
          In System Settings, turn on the switch next to <b>Iamb</b>. macOS may ask for your password or fingerprint.
        </p>
      )}
      {stuck ? (
        <div className="ob-banner bad" role="alert">
          <Icon name="warn" />
          <span className="ob-txt">
            Iamb needs to restart its helper to use the new permissions.
            {restartError ? " Iamb could not restart it from here. Quit Iamb from its menu and open it again." : ""}
          </span>
          <button type="button" className="ob-btn" onClick={onRestart}>
            Restart
          </button>
        </div>
      ) : null}
      <div className="ob-actions">
        {onBack ? (
          <button type="button" className="ob-btn ghost" onClick={onBack}>
            Back
          </button>
        ) : null}
        <span className="ob-grow" />
        {onSkip ? (
          <button type="button" className="ob-link" onClick={onSkip}>
            Skip for now
          </button>
        ) : null}
        <button type="button" className="ob-btn ob-primary" ref={proceed} disabled={!granted} onClick={onContinue}>
          {fix ? "Done" : "Continue"} {granted ? <Enter /> : null}
        </button>
      </div>
    </section>
  );
}

// ---- 3. First snippet -----------------------------------------------------------------------

export function SnippetStep(props: {
  heading: ReactNode;
  example: Example | null;
  save: (abbreviation: string, content: string) => Promise<void>;
  onBack: () => void;
  onSaved: () => void;
}) {
  const { heading, example, save, onBack, onSaved } = props;
  const [abbreviation, setAbbreviation] = useState(example?.abbreviation ?? STARTER_ABBREVIATION);
  const [content, setContent] = useState(example?.content ?? "");
  const [problem, setProblem] = useState<{ field: "abbreviation" | "content"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    const found = snippetProblem({ abbreviation, content });
    setProblem(found);
    if (found) return;
    setBusy(true);
    try {
      await save(abbreviation, content);
      if (alive.current) onSaved();
    } catch (err) {
      if (!alive.current) return;
      if (isCommandError(err) && err.code === "conflict") {
        setProblem({ field: "abbreviation", text: conflictText(abbreviation) });
      } else {
        setProblem({ field: "content", text: failureText(err, "save_snippet") });
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  const bad = (field: "abbreviation" | "content") => problem?.field === field;
  return (
    <form className="ob-step" onSubmit={(e) => void submit(e)} noValidate>
      <div className="ob-stack">
        {heading}
        <p className="ob-lead">Your email address is a good start. Change both sides to anything you like.</p>
      </div>
      <div className="ob-pair">
        <div className="ob-field">
          <label htmlFor="ob-abbr">When I type</label>
          <input
            id="ob-abbr"
            className="ob-input mono"
            value={abbreviation}
            onChange={(e) => setAbbreviation(e.target.value)}
            aria-invalid={bad("abbreviation") || undefined}
            aria-describedby={bad("abbreviation") ? "ob-problem" : undefined}
            {...PRIVATE}
          />
        </div>
        <span className="ob-arrow" aria-hidden="true">
          <Icon name="right" />
        </span>
        <div className="ob-field">
          <label htmlFor="ob-content">Iamb types</label>
          <input
            id="ob-content"
            className="ob-input"
            value={content}
            placeholder="your email address"
            onChange={(e) => setContent(e.target.value)}
            aria-invalid={bad("content") || undefined}
            aria-describedby={bad("content") ? "ob-problem" : undefined}
            autoFocus
            {...PRIVATE}
          />
        </div>
      </div>
      {problem ? (
        <p className="ob-problem" id="ob-problem" role="alert">
          {problem.text}
        </p>
      ) : null}
      <p className="ob-tip">
        Tip: begin abbreviations with a character you rarely type before a word, such as <Code>;</Code>, so one never
        fires by accident.
      </p>
      <div className="ob-actions">
        <button type="button" className="ob-btn ghost" onClick={onBack}>
          Back
        </button>
        <span className="ob-grow" />
        <button type="submit" className="ob-btn ob-primary" disabled={busy}>
          Save and try it <Enter />
        </button>
      </div>
    </form>
  );
}

// ---- 4. Try it ------------------------------------------------------------------------------

/** How long the window keeps asking the service to take the test field again after a restart. */
const REDECLARE_TRIES = 20;

export function TryStep(props: {
  heading: ReactNode;
  example: Example | null;
  /** Permissions were skipped and are still missing: nothing can expand yet. */
  blocked: boolean;
  mac: boolean;
  env: OnboardingEnv;
  nothingMs: number;
  pollMs: number;
  onExpandsAtOnce: (immediate: boolean) => Promise<void>;
  onFixPermissions: () => void;
  onBack: () => void;
  onContinue: () => void;
}) {
  const { heading, example, blocked, mac, env, nothingMs, pollMs, onExpandsAtOnce, onFixPermissions, onBack, onContinue } = props;
  const { api } = env;
  const abbreviation = example?.abbreviation ?? STARTER_ABBREVIATION;
  const immediate = example?.immediate ?? false;
  const [value, setValue] = useState("");
  const [worked, setWorked] = useState(false);
  const [hint, setHint] = useState<string | null>(null);
  const [cause, setCause] = useState<Cause | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const composing = useRef(false);
  const tabKept = useRef(false);
  const field = useRef<HTMLTextAreaElement>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // ---- Declare the field to the service, and withdraw it when the step is left ---------------
  const declared = useRef(false);
  const diagnose = useCallback(async () => {
    const [view, permissions] = await Promise.all([env.status().catch(() => null), api.permissions().catch(() => null)]);
    if (!alive.current) return;
    const found = firstCause({
      connected: view?.connection === "connected",
      service: view?.service ? { listener: view.service.listener, paused: view.service.paused } : null,
      permissions,
      composing: composing.current,
      mac,
    });
    setCause(found);
  }, [api, env, mac]);

  const declare = useCallback(async () => {
    try {
      await api.testField(true);
      declared.current = true;
      return true;
    } catch {
      return false;
    }
  }, [api]);

  // The service takes the window that has the keyboard as the field, so the field is declared
  // when it has the keyboard (its focus), not when the step mounts, when another Iamb window
  // may hold it; and again every poll while it has the focus, so that a service that restarted
  // (it forgets the field) takes it back without the person doing anything.
  const focused = useRef(false);
  const onFocus = () => {
    focused.current = true;
    void declare().then((ok) => {
      if (!ok && alive.current) void diagnose();
    });
  };
  useEffect(() => {
    if (blocked) return;
    const timer = setInterval(() => {
      if (focused.current) void declare();
    }, pollMs);
    return () => {
      clearInterval(timer);
      focused.current = false;
      if (declared.current) {
        declared.current = false;
        void api.testField(false).catch(() => undefined);
      }
    };
  }, [blocked, declare, api, pollMs]);

  // ---- Did it expand? Did nothing happen? ----------------------------------------------------
  useEffect(() => {
    if (worked || blocked) return;
    if (example && didExpand(value, abbreviation, example.content)) {
      setWorked(true);
      setCause(null);
      setHint(null);
      return;
    }
    if (!holdsUnexpanded(value, abbreviation, immediate)) return;
    const timer = setTimeout(() => void diagnose(), nothingMs);
    return () => clearTimeout(timer);
  }, [value, worked, blocked, example, abbreviation, immediate, nothingMs, diagnose]);

  // The banner's button takes the keyboard when the banner appears; Continue when it worked.
  const bannerButton = useRef<HTMLButtonElement>(null);
  const proceed = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (cause && cause.action) bannerButton.current?.focus();
  }, [cause]);
  useEffect(() => {
    if (worked) proceed.current?.focus();
  }, [worked]);

  const change = (text: string) => {
    setValue(text);
    setCause(null);
    if (!endsWithAbbreviation(text, abbreviation)) setHint(null);
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const before = e.currentTarget.value.slice(0, e.currentTarget.selectionStart);
    if (e.key === "Enter" && endsWithAbbreviation(before, abbreviation)) {
      setHint("Enter");
    } else if (e.key === "Tab" && !e.shiftKey && endsWithAbbreviation(before, abbreviation) && !tabKept.current) {
      // The field keeps Tab for itself once, to say why it does not expand; then it lets go.
      e.preventDefault();
      tabKept.current = true;
      setHint("Tab");
    }
  };

  const again = () => {
    setValue("");
    setWorked(false);
    setCause(null);
    setHint(null);
    queueMicrotask(() => field.current?.focus());
  };

  const expandAtOnce = async (on: boolean) => {
    setProblem(null);
    try {
      await onExpandsAtOnce(on);
      setValue("");
      setHint(null);
      setCause(null);
      field.current?.focus();
    } catch (err) {
      setProblem(failureText(err, "save_snippet"));
    }
  };

  /** Asks the supervisor to restart the worker, then asks the service to take the field again. */
  const act = async (c: Cause) => {
    setProblem(null);
    if (c.kind === "no_keyboard") {
      try {
        await api.restartHelper();
      } catch {
        setProblem("Iamb could not restart its helper from here. Quit Iamb from its menu and open it again.");
        return;
      }
      for (let i = 0; i < REDECLARE_TRIES && alive.current; i += 1) {
        if (await declare()) break;
        await new Promise((r) => setTimeout(r, pollMs));
      }
      setCause(null);
      setValue("");
    } else if (c.kind === "paused") {
      await env.resume().catch(() => undefined);
      setCause(null);
    } else if (c.kind === "permission") {
      onFixPermissions();
    } else if (c.kind === "other") {
      await env.openWhy().catch(() => undefined);
    }
  };

  if (blocked) {
    return (
      <section className="ob-step">
        <div className="ob-stack">
          {heading}
          <p className="ob-lead">Turn on both permissions and Iamb can expand your snippet. Until then it cannot type for you.</p>
        </div>
        <div className="ob-actions">
          <button type="button" className="ob-btn ghost" onClick={onBack}>
            Back
          </button>
          <span className="ob-grow" />
          <button type="button" className="ob-link" onClick={onContinue}>
            Skip this step
          </button>
          <button type="button" className="ob-btn ob-primary" onClick={onFixPermissions} autoFocus>
            Fix permissions
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="ob-step">
      <div className="ob-stack">
        {worked ? (
          <h1 className="ob-title" id="ob-title">
            {WORKED_HEADING}
          </h1>
        ) : (
          heading
        )}
        <p className="ob-lead">
          {worked ? (
            "It works the same way in your mail, your browser and everywhere else you type."
          ) : immediate ? (
            <>
              Type <Code>{abbreviation}</Code>.
            </>
          ) : (
            <>
              Type <Code>{abbreviation}</Code> and then a <b>space</b>.
            </>
          )}
        </p>
      </div>
      <div className="ob-field">
        <label className="ob-sr" htmlFor="ob-test">
          Test field
        </label>
        <textarea
          id="ob-test"
          ref={field}
          className="ob-input ob-test"
          rows={3}
          value={value}
          readOnly={worked}
          onChange={(e) => change(e.target.value)}
          onKeyDown={onKey}
          onFocus={onFocus}
          onBlur={() => {
            focused.current = false;
          }}
          onCompositionStart={() => {
            composing.current = true;
          }}
          aria-describedby="ob-test-help"
          autoFocus
          {...PRIVATE}
        />
        {worked ? null : (
          <p className="ob-help" id="ob-test-help">
            Iamb expands when you finish the word with a space or with punctuation such as <Code>.</Code> <Code>,</Code>{" "}
            <Code>?</Code> <Code>!</Code> Enter and Tab do not expand a snippet.
          </p>
        )}
      </div>
      {worked ? (
        <div className="ob-banner ok" role="status">
          <Icon name="check" />
          <span className="ob-txt">
            Iamb replaced <b>{abbreviation}</b> with your snippet.
          </span>
        </div>
      ) : (
        <label className="ob-switch">
          <input
            type="checkbox"
            role="switch"
            checked={immediate}
            onChange={(e) => void expandAtOnce(e.target.checked)}
          />
          <span className="ob-track" aria-hidden="true" />
          <span>
            Expand the moment I finish typing <Code>{abbreviation}</Code>, without a space
          </span>
        </label>
      )}
      {!worked && hint ? (
        <div className="ob-banner warn" role="status">
          <Icon name="info" />
          <span className="ob-txt">
            <b>{hint} does not expand a snippet.</b>
            {ENTER_HINT.rest}
          </span>
          <button type="button" className="ob-btn" onClick={() => void expandAtOnce(true)}>
            {ENTER_HINT.action}
          </button>
        </div>
      ) : null}
      {!worked && cause ? (
        <div className="ob-banner bad" role="alert">
          <Icon name="warn" />
          <span className="ob-txt">
            <b>{cause.lead}</b>
            {cause.rest}
          </span>
          {cause.action ? (
            <button type="button" className="ob-btn" ref={bannerButton} onClick={() => void act(cause)}>
              {cause.action}
            </button>
          ) : null}
        </div>
      ) : null}
      {problem ? (
        <p className="ob-problem" role="alert">
          {problem}
        </p>
      ) : null}
      <div className="ob-actions">
        {worked ? (
          <button type="button" className="ob-btn ghost" onClick={again}>
            Try again
          </button>
        ) : (
          <button type="button" className="ob-btn ghost" onClick={onBack}>
            Back
          </button>
        )}
        <span className="ob-grow" />
        {worked ? (
          <button type="button" className="ob-btn ob-primary" ref={proceed} onClick={onContinue}>
            Continue <Enter />
          </button>
        ) : cause ? (
          <button type="button" className="ob-link" onClick={onContinue}>
            Skip this step
          </button>
        ) : (
          <button type="button" className="ob-link" onClick={() => void diagnose()}>
            It does not work
          </button>
        )}
      </div>
    </section>
  );
}

// ---- 5. Done --------------------------------------------------------------------------------

export function DoneStep(props: { heading: ReactNode; env: OnboardingEnv; snippet: string | null }) {
  const { heading, env, snippet } = props;
  const { api, mac } = env;
  const [finished, setFinished] = useState<Finished | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [login, setLogin] = useState<LoginState | null>(null);
  const [startAtLogin, setStartAtLogin] = useState(true);
  const [loginProblem, setLoginProblem] = useState(false);
  const [chordProblem, setChordProblem] = useState<string | null>(null);
  const [listening, setListening] = useState(false);

  // Reaching this step is the end: the test field goes, the shortcut and `ui.onboarding_done` are
  // stored, and Iamb starts at login (the checkbox below undoes it).
  useEffect(() => {
    let live = true;
    void api.finish().then(
      (f) => live && setFinished(f),
      (e) => live && setProblem(failureText(e, "onboarding_finish")),
    );
    void api.loginItem(true).then(
      (s) => live && setLogin(s),
      () => live && setLoginProblem(true),
    );
    return () => {
      live = false;
    };
  }, [api]);

  const close = useCallback(() => void api.close().catch(() => undefined), [api]);
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape" && !listening) close();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [close, listening]);

  const openLibrary = async () => {
    if (snippet) env.rememberSelection(snippet);
    await api.openLibrary().catch((e) => setProblem(failureText(e, "onboarding_open_library")));
  };

  const toggleLogin = async (on: boolean) => {
    setStartAtLogin(on);
    setLoginProblem(false);
    try {
      setLogin(await api.loginItem(on));
    } catch {
      setLoginProblem(true);
    }
  };

  const chord = finished?.paletteHotkey ?? "Ctrl+Shift+Space";
  const recordKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    if (e.key === "Tab" && plain) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      e.currentTarget.blur();
      return;
    }
    const result = recordChord(e.nativeEvent, mac);
    if (result.kind === "chord") {
      setChordProblem(null);
      void env.settings.putSetting("palette_hotkey", result.chord).then(
        () => setFinished({ paletteHotkey: result.chord, hotkeyRefused: false }),
        (err) =>
          setChordProblem(
            isCommandError(err) && err.code === "conflict"
              ? "Another app already uses this shortcut."
              : "Iamb could not use that shortcut. Try another.",
          ),
      );
    } else if (result.kind === "refused") {
      setChordProblem(
        result.reason === "reserved" || result.reason === "types"
          ? `${refusedText(result.reason, mac)}.`
          : "Choose a shortcut with Ctrl, Alt or ⌘ and one other key.",
      );
    }
  };

  return (
    <section className="ob-step">
      <div className="ob-stack">
        {heading}
        <p className="ob-lead">{whereItLives(mac)} Close this window whenever you like: it keeps working.</p>
      </div>
      <ul className="ob-points">
        {finished?.hotkeyRefused ? (
          <li>
            <Icon name="search" />
            <div>
              <b id="ob-chord-label">Choose a shortcut for search</b>
              <span>Search your snippets from any app and insert one with Enter.</span>
              <button
                type="button"
                className="ob-input ob-recorder"
                aria-labelledby="ob-chord-label"
                onFocus={() => setListening(true)}
                onBlur={() => setListening(false)}
                onKeyDown={recordKey}
              >
                {listening ? "Press the keys" : "Not set"}
              </button>
              {chordProblem ? <span role="alert">{chordProblem}</span> : null}
            </div>
          </li>
        ) : (
          <li>
            <Icon name="search" />
            <div>
              <b>
                Forgot an abbreviation? Press{" "}
                <span role="img" aria-label={chordSpoken(chord, mac)}>
                  {chordBadges(chord, mac).map((p, i) => (
                    <kbd key={`${p}-${i}`} aria-hidden="true">
                      {p}
                    </kbd>
                  ))}
                </span>
              </b>
              <span>
                Search your snippets from any app and insert one with <kbd>Enter</kbd>.
              </span>
            </div>
          </li>
        )}
        <li>
          <Icon name="pause" />
          <div>
            <b>Need it quiet for a while?</b>
            <span>Pause Iamb from its {iconName(mac)}.</span>
          </div>
        </li>
      </ul>
      <label className="ob-check">
        <input type="checkbox" checked={startAtLogin} onChange={(e) => void toggleLogin(e.target.checked)} />
        <span>Start Iamb when I log in</span>
      </label>
      {login === "requires_approval" && startAtLogin ? (
        <p className="ob-help">macOS asks you to approve this once, in System Settings, under Login Items.</p>
      ) : null}
      {loginProblem ? (
        <p className="ob-problem" role="alert">
          Iamb could not change this. It is not a problem: you can start it from the {mac ? "Applications folder" : "Start menu"}.
        </p>
      ) : null}
      {problem ? (
        <p className="ob-problem" role="alert">
          {problem}
        </p>
      ) : null}
      <div className="ob-actions">
        <button type="button" className="ob-btn" onClick={close}>
          Close
        </button>
        <span className="ob-grow" />
        <button type="button" className="ob-btn ob-primary" onClick={() => void openLibrary()} autoFocus>
          Open my library <Enter />
        </button>
      </div>
    </section>
  );
}
