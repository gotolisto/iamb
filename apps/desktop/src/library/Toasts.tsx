import { useEffect, useRef, useState } from "react";

export interface ToastState {
  id: number;
  parts: { text: string; strong?: boolean }[];
  action?: { label: string; run: () => void };
}

/**
 * One toast at the bottom centre (design system §4.9): it leaves after 4 s, after 10 s when it
 * has an action, and stays while it is hovered or holds the focus. It carries nothing that is not
 * also in the list.
 */
export function Toast({
  toast,
  onDismiss,
  fallback,
}: {
  toast: ToastState | null;
  onDismiss: (id: number) => void;
  /** Takes the focus when the toast leaves holding it and where it came from is gone. */
  fallback?: () => void;
}) {
  const [held, setHeld] = useState(false);
  const id = toast?.id;
  const hasAction = toast?.action !== undefined;
  useEffect(() => {
    if (id === undefined || held) return;
    const timer = setTimeout(() => onDismiss(id), hasAction ? 10_000 : 4_000);
    return () => clearTimeout(timer);
  }, [id, held, hasAction, onDismiss]);
  const root = useRef<HTMLDivElement>(null);
  // Where the keyboard was before it came to the toast: when the toast goes while it holds the
  // focus (Undo was pressed), the focus goes back there and is not lost (spec §5.6).
  const from = useRef<HTMLElement | null>(null);
  const leave = (toastId: number) => {
    const held = root.current?.contains(document.activeElement) ?? false;
    onDismiss(toastId);
    if (held) {
      requestAnimationFrame(() => {
        // Where it was, or, when that is gone too, where the window puts the focus.
        if (from.current?.isConnected) from.current.focus();
        else fallback?.();
        from.current = null;
      });
    }
  };
  // The live region is always in the page, empty until a toast comes: a screen reader announces
  // text that is added to a region it already knows, and not always a region that arrives with
  // its text (4.1.3). `data-pane` makes the toast a stop of F6 while it has an action (§5.6).
  return (
    <div
      className="toast-region"
      role="status"
      data-pane={toast?.action ? "toast" : undefined}
    >
      {toast && (
        <div
          ref={root}
          className="toast"
          onMouseEnter={() => setHeld(true)}
          onMouseLeave={() => setHeld(false)}
          onFocus={(e) => {
            if (!from.current && e.relatedTarget instanceof HTMLElement) from.current = e.relatedTarget;
            setHeld(true);
          }}
          onBlur={() => setHeld(false)}
        >
          <span>
            {toast.parts.map((p, i) =>
              p.strong ? (
                <strong key={i} className="mono">
                  {p.text}
                </strong>
              ) : (
                <span key={i}>{p.text}</span>
              ),
            )}
          </span>
          {toast.action && (
            <button
              type="button"
              className="toast-action"
              onClick={() => {
                const run = toast.action?.run;
                leave(toast.id);
                run?.();
              }}
            >
              {toast.action.label}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
