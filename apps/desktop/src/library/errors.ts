// What a failed command says in the editor and in the window (specification §6.3). The details
// copied for a bug report are the code and the request's name, never the request's content.

import { isCommandError } from "../ipc";

export type Failure =
  | { kind: "gone"; text: string; detail: string }
  | { kind: "busy"; text: string; detail: string }
  | { kind: "offline"; text: string; detail: string }
  | { kind: "invalid"; text: string; detail: string }
  | { kind: "conflict"; text: string; detail: string }
  | { kind: "internal"; text: string; detail: string };

export function describeFailure(e: unknown, request: string): Failure {
  const code = isCommandError(e) ? e.code : "internal";
  const message = isCommandError(e) ? e.message : "";
  const detail = `${code} · ${request}`;
  switch (code) {
    case "not_found":
      return {
        kind: "gone",
        text: "This snippet no longer exists. It may have been deleted on another device.",
        detail,
      };
    case "conflict":
      return { kind: "conflict", text: "That abbreviation belongs to another snippet.", detail };
    case "invalid":
      return {
        kind: "invalid",
        text: message ? `Iamb could not accept that value. ${message}` : "Iamb could not accept that value.",
        detail,
      };
    case "unavailable":
    case "timeout":
      return { kind: "busy", text: "Iamb is busy. Try again in a moment.", detail };
    case "disconnected":
      return {
        kind: "offline",
        text: "Iamb is not running. Your changes stay here until it is back.",
        detail,
      };
    default:
      return {
        kind: "internal",
        text: "Something went wrong inside Iamb. Restarting it usually helps.",
        detail,
      };
  }
}
