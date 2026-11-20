// The editor's live preview (UX specification §4.3; flow 2, step 5): what the snippet would
// type, rendered by the service with the parser that expands (`RenderPreview`), 150 ms after
// the last change (§10). Blanks without a value are grey name tags, key presses key badges,
// waits "· 0.5 s ·", the cursor position a caret; below, the fill-in fields the form would ask
// for. A template with an error previews as "Preview unavailable" and the reason.
//
// Privacy: the test values (`values`) are sent only in the one request; the preview never reads
// the clipboard (the service shows the word "Clipboard").

import { forwardRef, useEffect, useRef, useState, type ReactNode } from "react";

import type { FieldView } from "../../ipc";
import { failureCode, type PreviewPart, type TemplatePreview as Preview, type TemplateService } from "./api";
import { errorMessage, messageText } from "./errors";
import { FIELD_KINDS, keyLabel, seconds } from "./macros";

/** The budget of §10: the preview follows the last key by 150 ms. */
export const PREVIEW_DELAY_MS = 150;

export interface TemplatePreviewProps {
  source: string;
  service: TemplateService;
  /** Test values from "Try the form"; absent fields show as name tags. */
  values?: Record<string, string> | undefined;
  /** Opens the fill-in form to try it; the button shows only with this and fields to ask. */
  onTryForm?: ((fields: FieldView[]) => void) | undefined;
  delayMs?: number;
  /** ⌘⇧P / Ctrl+Shift+P inside the preview: back to the text. */
  onBack?: () => void;
}

type State =
  | { kind: "waiting" }
  | { kind: "ready"; preview: Preview }
  | { kind: "failed"; code: string };

function unavailableReason(code: string): string {
  if (code === "disconnected" || code === "timeout") return "Iamb is not running.";
  if (code === "unavailable") return "Iamb is busy. Try again in a moment.";
  return "Something went wrong inside Iamb. Restarting it usually helps.";
}

/** The values as a stable key for the effect (never logged or stored). */
function valuesKey(values: Record<string, string> | undefined): string {
  return values ? JSON.stringify(Object.entries(values).sort(([a], [b]) => (a < b ? -1 : 1))) : "";
}

export const TemplatePreview = forwardRef<HTMLElement, TemplatePreviewProps>(function TemplatePreview(
  { source, service, values, onTryForm, delayMs = PREVIEW_DELAY_MS, onBack },
  ref,
) {
  const [state, setState] = useState<State>({ kind: "waiting" });
  const seq = useRef(0);
  const key = valuesKey(values);
  const valuesRef = useRef(values);
  valuesRef.current = values;

  useEffect(() => {
    const mine = ++seq.current;
    const timer = setTimeout(() => {
      service.preview(source, valuesRef.current).then(
        (preview) => {
          // An older answer that arrives after a newer request is dropped.
          if (mine === seq.current) setState({ kind: "ready", preview });
        },
        (e: unknown) => {
          if (mine === seq.current) setState({ kind: "failed", code: failureCode(e) });
        },
      );
    }, delayMs);
    return () => clearTimeout(timer);
  }, [source, key, service, delayMs]);

  const preview = state.kind === "ready" ? state.preview : null;
  const fields = preview?.fields ?? [];
  const count = fields.length;

  let body: ReactNode;
  if (state.kind === "failed") {
    body = (
      <p className="tpl-unavailable">
        <strong>Preview unavailable</strong> {unavailableReason(state.code)}
      </p>
    );
  } else if (preview?.error) {
    body = (
      <p className="tpl-unavailable">
        <strong>Preview unavailable</strong> {messageText(errorMessage(preview.error, null))}
      </p>
    );
  } else if (preview) {
    body =
      preview.parts.length === 0 ? (
        <p className="tpl-out tpl-muted">Nothing to type yet.</p>
      ) : (
        <div className="tpl-out">{renderParts(preview)}</div>
      );
  } else {
    body = <div className="tpl-out" />;
  }

  return (
    <section
      ref={ref}
      className="tpl-preview"
      aria-label="Preview"
      tabIndex={-1}
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "p") {
          e.preventDefault();
          onBack?.();
        }
      }}
    >
      <div className="tpl-preview-head">
        <span className="tpl-ovl">Preview</span>
        {count > 0 && (
          <span className="tpl-count">{count === 1 ? "Asks for 1 value" : `Asks for ${count} values`}</span>
        )}
        {onTryForm && count > 0 && (
          <button type="button" className="tpl-btn" onClick={() => onTryForm(fields)}>
            Try the form
          </button>
        )}
      </div>
      {body}
      {count > 0 && !preview?.error && (
        <ul className="tpl-fields" aria-label="Fill-in fields">
          {fields.map((f) => (
            <li key={f.name}>
              <span className="tpl-field-name">{f.name}</span>
              <span className="tpl-muted">{fieldSummary(f, values)}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
});

/** "One line", "Choice: Road, Gravel · starts with Road", "Toggle · starts checked", "· your
 * test value" — what the form would ask, and the value the preview used. */
function fieldSummary(f: FieldView, values: Record<string, string> | undefined): string {
  const kind = FIELD_KINDS.find((k) => k.kind === f.kind)?.label ?? f.kind;
  const bits = [f.kind === "choice" && f.options.length ? `${kind}: ${f.options.join(", ")}` : kind];
  if (values && f.name in values) bits.push("your test value");
  else if (f.kind === "toggle") bits.push(f.default === "yes" ? "starts checked" : "starts unchecked");
  else if (f.default) bits.push(`starts with “${f.default}”`);
  else if (f.kind === "text" || f.kind === "area") bits.push("shown by its name");
  return bits.join(" · ");
}

const caret = (key: string) => <span key={key} className="tpl-caret" role="img" aria-label="Cursor position" />;

function renderParts(p: Preview): ReactNode[] {
  const out: ReactNode[] = [];
  const cursor = p.cursor;
  p.parts.forEach((part, i) => {
    if (cursor && cursor.part === i && part.part !== "text") out.push(caret(`c${i}`));
    out.push(renderPart(part, i, cursor && cursor.part === i ? cursor.offset : null));
  });
  if (cursor && cursor.part >= p.parts.length) out.push(caret("c-end"));
  return out;
}

function renderPart(part: PreviewPart, i: number, cursorAt: number | null): ReactNode {
  if (part.part === "key") {
    const label = keyLabel(part.key, part.shift, part.times);
    return (
      <kbd key={i} className="tpl-kbd" aria-label={`Key press, ${keyLabel(part.key, part.shift, part.times, true)}`}>
        {label}
      </kbd>
    );
  }
  if (part.part === "wait") {
    return (
      <span key={i} className="tpl-wait">
        {` · ${seconds(part.ms)} · `}
      </span>
    );
  }
  // Text, cut at the blanks, the clipboard ranges and the cursor.
  type Mark = { start: number; end: number; node: (text: string, k: string) => ReactNode };
  const marks: Mark[] = [
    ...part.blanks.map<Mark>((b) => ({
      start: b.start,
      end: b.end,
      node: (text, k) =>
        b.sample ? (
          <span key={k} className="tpl-tag" title="Fill-in field">
            {text}
          </span>
        ) : (
          <span key={k} className="tpl-value">
            {text}
          </span>
        ),
    })),
    ...part.clipboard.map<Mark>(([start, end]) => ({
      start,
      end,
      node: (text, k) => (
        <span key={k} className="tpl-tag">
          {text}
        </span>
      ),
    })),
  ].sort((a, b) => a.start - b.start);
  const nodes: ReactNode[] = [];
  let at = 0;
  const text = part.text;
  let pending = cursorAt;
  const plain = (from: number, to: number) => {
    if (pending !== null && pending >= from && pending <= to) {
      if (pending > from) nodes.push(text.slice(from, pending));
      nodes.push(caret(`c${i}`));
      if (to > pending) nodes.push(text.slice(pending, to));
      pending = null;
      return;
    }
    if (to > from) nodes.push(text.slice(from, to));
  };
  for (const [n, m] of marks.entries()) {
    if (m.start < at) continue;
    plain(at, m.start);
    nodes.push(m.node(text.slice(m.start, m.end), `m${i}-${n}`));
    at = m.end;
  }
  plain(at, text.length);
  if (pending !== null) nodes.push(caret(`c${i}`));
  return <span key={i}>{nodes}</span>;
}
