import { useCallback, useState } from "react";

import type { Failure } from "../errors";

export type Message = { kind: "error" | "warning"; text: string };

/** The message under each setting, by name; setting one clears the others of the same name. */
export function useMessages() {
  const [messages, setMessages] = useState<Record<string, Message | undefined>>({});
  const set = useCallback((name: string, message: Message | null) => {
    setMessages((old) => {
      const next = { ...old };
      if (message) next[name] = message;
      else delete next[name];
      return next;
    });
  }, []);
  /** Shows a failure as an error under `name`; null clears it. */
  const fail = useCallback(
    (name: string, failure: Failure | null, text?: string) => {
      set(name, failure ? { kind: "error", text: text ?? failure.text } : null);
    },
    [set],
  );
  return { messages, set, fail };
}
