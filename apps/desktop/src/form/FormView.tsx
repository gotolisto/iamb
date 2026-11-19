import { useEffect, useState } from "react";

import { api, onUiEvent, type UiEvent } from "../ipc";

type FormRequest = Extract<UiEvent, { type: "form_requested" }>;

/**
 * The fill-in form panel, created hidden at start so that it can appear within 150 ms. A
 * placeholder until the form itself lands (architecture §4.5, §11, 3.5): it shows which fields
 * the last request asked for, never values.
 */
export function FormView() {
  const [request, setRequest] = useState<FormRequest | null>(null);
  useEffect(() => {
    const unlisten = onUiEvent((event) => {
      if (event.type === "form_requested") setRequest(event);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);
  return (
    <main className="window panel" onKeyDown={(e) => e.key === "Escape" && void api.dismiss()}>
      <h1 className="title">Fill-in form</h1>
      {request ? (
        <ul className="list">
          {request.fields.map((f) => (
            <li key={f.name} className="row">
              <span className="label">{f.name}</span>
              <span className="muted">{f.kind}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No form is open.</p>
      )}
      <button type="button" className="button" onClick={() => void api.dismiss()}>
        Close
      </button>
    </main>
  );
}
