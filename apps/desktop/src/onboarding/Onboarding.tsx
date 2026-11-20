import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { StatusView } from "../ipc";
import type { Permissions } from "./api";
import type { OnboardingEnv } from "./env";
import { type Example, pickExample, saveStarter, setExpandsAtOnce } from "./example";
import {
  CANNOT_EXPAND_HEADING,
  HEADINGS,
  LISTENER_GRACE_MS,
  NOTHING_HAPPENED_MS,
  PERMISSION_POLL_MS,
  type StepId,
  anyMissing,
  asksForPermissions,
  progressLine,
  stepsFor,
} from "./model";
import { DoneStep, PermissionsStep, SnippetStep, TryStep, WelcomeStep } from "./steps";
import "./onboarding.css";

/** What the window learned when it opened. */
interface Boot {
  connected: boolean;
  permissions: Permissions | null;
  example: Example | null;
}

export function Onboarding({ env }: { env: OnboardingEnv }) {
  const { api, library, mac, mode } = env;
  const pollMs = env.timing?.pollMs ?? PERMISSION_POLL_MS;
  const nothingMs = env.timing?.nothingMs ?? NOTHING_HAPPENED_MS;
  const graceMs = env.timing?.graceMs ?? LISTENER_GRACE_MS;

  const [boot, setBoot] = useState<Boot | null>(null);
  const [step, setStep] = useState<StepId>(mode === "fix" ? "permissions" : "welcome");
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [service, setService] = useState<StatusView | null>(null);
  const [skipped, setSkipped] = useState(false);
  const [example, setExample] = useState<Example | null>(null);
  const [restartError, setRestartError] = useState(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // ---- What the window learns when it opens ------------------------------------------------
  const learn = useCallback(async (): Promise<Boot> => {
    const view = await env.status().catch(() => null);
    const connected = view?.connection === "connected";
    if (alive.current) setService(view);
    if (!connected) return { connected: false, permissions: null, example: null };
    const [p, found] = await Promise.all([
      api.permissions().catch(() => null),
      pickExample(library).catch(() => null),
    ]);
    if (alive.current) setPermissions(p);
    return { connected: true, permissions: p, example: found };
  }, [api, env, library]);

  useEffect(() => {
    let live = true;
    void learn().then((b) => {
      if (!live) return;
      setBoot(b);
      if (b.example) setExample(b.example);
    });
    return () => {
      live = false;
    };
  }, [learn]);

  // Until the service answers, the welcome step looks again every second: "the step returns to
  // normal when `GetStatus` answers".
  const down = boot !== null && !boot.connected;
  useEffect(() => {
    if (!down) return;
    const timer = setInterval(() => {
      void learn().then((b) => {
        if (b.connected && alive.current) {
          setBoot(b);
          if (b.example) setExample(b.example);
          setRestartError(false);
        }
      });
    }, pollMs);
    return () => clearInterval(timer);
  }, [down, learn, pollMs]);

  // The permissions step is part of this run when the system asks for permissions and one was
  // missing when the window opened; it stays part of it once it is.
  const showsPermissions = boot?.permissions != null && asksForPermissions(boot.permissions) && anyMissing(boot.permissions);
  const [keepsPermissions, setKeepsPermissions] = useState(false);
  useEffect(() => {
    if (showsPermissions) setKeepsPermissions(true);
  }, [showsPermissions]);
  // The first snippet is skipped when the library had a usable snippet when the window opened.
  const skipsSnippet = boot === null ? null : boot.example !== null;
  const steps = useMemo(
    () =>
      mode === "fix"
        ? (["permissions"] as StepId[])
        : stepsFor({ permissions: showsPermissions || keepsPermissions, snippet: skipsSnippet !== true }),
    [mode, showsPermissions, keepsPermissions, skipsSnippet],
  );
  const index = Math.max(0, steps.indexOf(step));
  const go = useCallback(
    (to: StepId) => {
      setStep(to);
    },
    [],
  );
  const next = () => go(steps[Math.min(index + 1, steps.length - 1)] ?? "done");
  const back = () => go(steps[Math.max(index - 1, 0)] ?? "welcome");

  // ---- The permissions: read every second while the step is shown ---------------------------
  const watching = step === "permissions";
  useEffect(() => {
    if (!watching) return;
    let live = true;
    const read = async () => {
      const [p, s] = await Promise.all([api.permissions().catch(() => null), env.status().catch(() => null)]);
      if (!live) return;
      if (p) setPermissions(p);
      setService(s);
    };
    void read();
    const timer = setInterval(() => void read(), pollMs);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [watching, api, env, pollMs]);

  const restart = async () => {
    setRestartError(false);
    try {
      await api.restartHelper();
    } catch {
      setRestartError(true);
    }
  };

  const title = step === "try" && skipped && permissions && anyMissing(permissions) ? CANNOT_EXPAND_HEADING : HEADINGS[step];
  const progress = mode === "fix" ? null : progressLine(index, steps.length);
  const spoken = progress ? `${progress}. ` : "";

  useEffect(() => {
    document.title = "Welcome to Iamb";
  }, []);

  const heading = (
    <h1 className="ob-title" id="ob-title">
      <span className="ob-sr">{spoken}</span>
      {title}
    </h1>
  );

  return (
    <div className="ob-root">
      <main className="ob" aria-labelledby="ob-title" data-step={step}>
        {progress ? (
          <div className="ob-progress">
            <span className="ob-bar" aria-hidden="true">
              {steps.map((id, i) => (
                <i key={id} className={i <= index ? "done" : ""} />
              ))}
            </span>
            <span aria-hidden="true">{progress}</span>
          </div>
        ) : null}
        {step === "welcome" ? (
          <WelcomeStep
            heading={heading}
            ready={boot !== null}
            serviceDown={down}
            restartError={restartError}
            onRestart={() => void restart()}
            onStart={next}
          />
        ) : null}
        {step === "permissions" ? (
          <PermissionsStep
            heading={heading}
            permissions={permissions}
            service={service}
            fix={mode === "fix"}
            graceMs={graceMs}
            restartError={restartError}
            onOpen={(kind) => void api.openPermissionSettings(kind).catch(() => undefined)}
            onRestart={() => void restart()}
            onBack={mode === "fix" ? null : back}
            onSkip={
              mode === "fix"
                ? null
                : () => {
                    setSkipped(true);
                    next();
                  }
            }
            onContinue={mode === "fix" ? () => void api.close().catch(() => undefined) : next}
          />
        ) : null}
        {step === "snippet" ? (
          <SnippetStep
            heading={heading}
            example={example}
            save={async (abbreviation, content) => {
              const saved = await saveStarter(library, abbreviation, content, example);
              setExample(saved);
            }}
            onBack={back}
            onSaved={next}
          />
        ) : null}
        {step === "try" ? (
          <TryStep
            heading={heading}
            example={example}
            blocked={skipped && permissions != null && anyMissing(permissions)}
            mac={mac}
            env={env}
            nothingMs={nothingMs}
            pollMs={pollMs}
            onExpandsAtOnce={async (immediate) => {
              if (!example) return;
              setExample(await setExpandsAtOnce(library, example, immediate));
            }}
            onFixPermissions={() => go("permissions")}
            onBack={back}
            onContinue={next}
          />
        ) : null}
        {step === "done" ? <DoneStep heading={heading} env={env} snippet={example?.id ?? null} /> : null}
      </main>
    </div>
  );
}

