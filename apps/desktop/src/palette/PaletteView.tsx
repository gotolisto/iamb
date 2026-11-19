import { useEffect, useRef } from "react";

import { api } from "../ipc";

/**
 * The search palette, created hidden at start so that it can appear within 100 ms. A
 * placeholder until search lands (architecture §11, 3.6).
 */
export function PaletteView() {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const focus = () => input.current?.focus();
    window.addEventListener("focus", focus);
    return () => window.removeEventListener("focus", focus);
  }, []);
  return (
    <main className="window panel" onKeyDown={(e) => e.key === "Escape" && void api.dismiss()}>
      <label className="title" htmlFor="palette-query">
        Search snippets
      </label>
      <input
        id="palette-query"
        ref={input}
        className="input"
        type="search"
        placeholder="Search is not available yet"
        disabled
      />
      <p className="muted">Press Escape to close.</p>
    </main>
  );
}
