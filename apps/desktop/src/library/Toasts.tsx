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
export function Toast({ toast, onDismiss }: { toast: ToastState | null; onDismiss: (id: number) => void }) {
  const [held, setHeld] = useState(false);
  const id = toast?.id;
  const hasAction = toast?.action !== undefined;
  useEffect(() => {
    if (id === undefined || held) return;
    const timer = setTimeout(() => onDismiss(id), hasAction ? 10_000 : 4_000);
    return () => clearTimeout(timer);
  }, [id, held, hasAction, onDismiss]);
  const root = useRef<HTMLDivElement>(null);
  if (!toast) return null;
  return (
    <div className="toast-region">
      <div
        ref={root}
        className="toast"
        role="status"
        onMouseEnter={() => setHeld(true)}
        onMouseLeave={() => setHeld(false)}
        onFocus={() => setHeld(true)}
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
              onDismiss(toast.id);
              run?.();
            }}
          >
            {toast.action.label}
          </button>
        )}
      </div>
    </div>
  );
}
