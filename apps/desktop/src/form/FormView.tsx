import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { afterNextPaint } from "../paint";
import { formApi, isFocusLost, listenForm, type FormApi, type FormOpen, type ListenForm } from "./api";
import { FormPanel, focusFirstField, naturalHeight } from "./FormPanel";
import { currentOs, defaults, initialValues, remembered, submission, type Os, type Values } from "./model";
import "./form.css";

export interface FormViewProps {
  api?: FormApi;
  listen?: ListenForm;
  os?: Os;
  /** Reports a paint (tests replace the browser's frame clock). */
  onNextPaint?: (report: (paintedAtMs: number) => void) => void;
}

interface Shown {
  form: FormOpen;
  values: Values;
  remembered: boolean;
  focusLost: boolean;
}

/**
 * The fill-in form panel, created hidden at start so that it can appear within 150 ms
 * (architecture §4.5, ADR-0017; spec §4.4; flow 4).
 *
 * The Rust side (src-tauri/src/form.rs) says a form is open; this view fetches it, renders it
 * while the panel is still hidden, reports its height, and, once the panel is shown, puts the
 * keyboard in the first field and reports the paint. Insert and Cancel go back to Rust, which
 * hides the panel and hands the focus back before anything reaches the service.
 *
 * The answers live in this component's state only. They are dropped (and the fields unmounted)
 * when the form closes: on Insert, on Cancel, and when Rust says the form is over.
 */
export function FormView({
  api = formApi,
  listen = listenForm,
  os = currentOs(),
  onNextPaint = afterNextPaint,
}: FormViewProps) {
  const [shown, setShown] = useState<Shown | null>(null);
  /** The form as last rendered, for an event handler set up once. */
  const latest = useRef<Shown | null>(null);
  latest.current = shown;
  const root = useRef<HTMLDivElement>(null);
  const session = useRef<number | null>(null);
  /** A `shown` event that came before the form was rendered. */
  const pendingShown = useRef<number | null>(null);
  const busy = useRef(false);
  /** Bumped to move the keyboard to the first field after the next render. */
  const [focusRequest, setFocusRequest] = useState(0);
  useEffect(() => {
    if (focusRequest > 0) focusFirstField(root.current);
  }, [focusRequest]);

  const forget = useCallback(() => {
    session.current = null;
    pendingShown.current = null;
    busy.current = false;
    setShown(null);
  }, []);

  const panelShown = useCallback(
    (s: number) => {
      focusFirstField(root.current);
      onNextPaint((paintedAtMs) => {
        void api.painted(s, paintedAtMs).catch(() => undefined);
      });
    },
    [api, onNextPaint],
  );

  const load = useCallback(async () => {
    let form: FormOpen | null;
    try {
      form = await api.current();
    } catch {
      return;
    }
    if (!form || form.session === session.current) return;
    session.current = form.session;
    busy.current = false;
    setShown({
      form,
      values: initialValues(form),
      remembered: remembered(form),
      focusLost: false,
    });
  }, [api]);

  useEffect(() => {
    let live = true;
    let stop: (() => void) | undefined;
    void listen((event) => {
      switch (event.type) {
        case "open":
          void load();
          break;
        case "shown":
          if (event.session === session.current) panelShown(event.session);
          else pendingShown.current = event.session;
          break;
        case "target_gone": {
          // The window the text was for is gone: the answers go to Rust once, to be kept for
          // "Fill in … again", and the panel forgets them.
          const cur = latest.current;
          if (event.session !== session.current || !cur) break;
          const values = submission(cur.form.fields, cur.values);
          forget();
          void api.abandon(event.session, values).catch(() => undefined);
          break;
        }
        case "closed":
          if (event.session === session.current) forget();
          break;
      }
    })
      .then((unlisten) => {
        if (live) {
          stop = unlisten;
          // Listening: the Rust side may now have the hidden panel paint once.
          void api.ready().catch(() => undefined);
        } else unlisten();
      })
      .catch(() => undefined);
    // A form that opened before this page had loaded.
    void load();
    return () => {
      live = false;
      stop?.();
    };
  }, [api, listen, load, panelShown, forget]);

  // Once a new form is rendered (still hidden): its height places the panel.
  const renderedSession = shown?.form.session ?? null;
  useLayoutEffect(() => {
    if (renderedSession === null) return;
    void api.laidOut(renderedSession, naturalHeight(root.current)).catch(() => undefined);
    if (pendingShown.current === renderedSession) {
      pendingShown.current = null;
      panelShown(renderedSession);
    }
  }, [renderedSession, api, panelShown]);

  if (!shown) {
    // Hidden and empty between forms.
    return <div ref={root} className="ff-root" />;
  }

  const { form } = shown;
  const insert = () => {
    if (busy.current) return;
    busy.current = true;
    const s = form.session;
    setShown((cur) => (cur ? { ...cur, focusLost: false } : cur));
    api.submit(s, submission(form.fields, shown.values)).then(
      () => {
        if (session.current === s) forget();
      },
      (e: unknown) => {
        if (session.current !== s) return;
        if (isFocusLost(e)) {
          busy.current = false;
          setShown((cur) => (cur ? { ...cur, focusLost: true } : cur));
        } else {
          forget();
        }
      },
    );
  };
  const cancel = () => {
    const s = form.session;
    forget();
    void api.cancel(s).catch(() => undefined);
  };
  const startEmpty = () => {
    setShown((cur) =>
      cur ? { ...cur, values: defaults(cur.form.fields), remembered: false } : cur,
    );
    setFocusRequest((n) => n + 1);
  };

  return (
    <div ref={root} className="ff-root">
      <FormPanel
        key={form.session}
        form={form}
        values={shown.values}
        remembered={shown.remembered}
        focusLost={shown.focusLost}
        os={os}
        onChange={(name, value) =>
          setShown((cur) => (cur ? { ...cur, values: { ...cur.values, [name]: value } } : cur))
        }
        onInsert={insert}
        onCancel={cancel}
        onStartEmpty={startEmpty}
      />
    </div>
  );
}
