import { type ReactNode, useEffect, useId, useRef } from "react";

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

interface DialogProps {
  title: string;
  children?: ReactNode;
  /** The buttons, in the order of the platform. The one with `data-primary` is Enter's. */
  actions: ReactNode;
  /** Esc, and the cancel button's work. */
  onCancel: () => void;
  /** `alertdialog` for a destructive confirmation. */
  role?: "dialog" | "alertdialog";
  /** Wider than the default 480 px (the review dialog). */
  wide?: boolean;
}

/**
 * A dialog of the design system (§4.8): focus moves in and is trapped, Esc cancels, Enter is the
 * primary button, and focus returns to where it came from. The element with `data-autofocus`
 * (else the first control) takes the focus first.
 */
export function Dialog({ title, children, actions, onCancel, role = "dialog", wide }: DialogProps) {
  const titleId = useId();
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const el = root.current;
    const first =
      el?.querySelector<HTMLElement>("[data-autofocus]") ??
      el?.querySelector<HTMLElement>(FOCUSABLE) ??
      null;
    first?.focus();
    // The dialog is modal: focus that lands outside it (a click that was still being handled
    // when it opened) comes back.
    const pullBack = (e: FocusEvent) => {
      if (el && e.target instanceof Node && !el.contains(e.target)) {
        (el.querySelector<HTMLElement>("[data-autofocus]") ?? el.querySelector<HTMLElement>(FOCUSABLE))?.focus();
      }
    };
    document.addEventListener("focusin", pullBack);
    return () => {
      document.removeEventListener("focusin", pullBack);
      // Focus is never lost: back to where it came from, if that is still there.
      if (before?.isConnected) before.focus();
    };
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
      return;
    }
    if (e.key === "Tab" && root.current) {
      const items = [...root.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
      const firstEl = items[0];
      const lastEl = items[items.length - 1];
      if (!firstEl || !lastEl) return;
      if (e.shiftKey && document.activeElement === firstEl) {
        e.preventDefault();
        lastEl.focus();
      } else if (!e.shiftKey && document.activeElement === lastEl) {
        e.preventDefault();
        firstEl.focus();
      }
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
      const target = e.target as HTMLElement;
      // A button, a select or a text area does its own Enter.
      if (target.tagName === "BUTTON" || target.tagName === "TEXTAREA" || target.tagName === "SELECT") {
        return;
      }
      const primary = root.current?.querySelector<HTMLButtonElement>("button[data-primary]");
      if (primary && !primary.disabled) {
        e.preventDefault();
        primary.click();
      }
    }
  };

  return (
    <div className="scrim">
      {/* The scrim's click does nothing: a dialog is left by its buttons or Esc. */}
      <div
        ref={root}
        className={wide ? "dialog dialog-wide" : "dialog"}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onKeyDown}
      >
        <h2 id={titleId} className="dialog-title">
          {title}
        </h2>
        <div className="dialog-body">{children}</div>
        <div className="dialog-actions">{actions}</div>
      </div>
    </div>
  );
}
