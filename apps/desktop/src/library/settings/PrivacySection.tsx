import { useState } from "react";

import { describeFailure } from "../errors";
import { ConfirmDialog } from "../Dialogs";
import { Row, Switch } from "./controls";
import type { SectionProps } from "./section";
import { useMessages } from "./useMessages";

/** The three statements of the Privacy section (specification §4.7), word for word. */
export const PRIVACY_STATEMENTS = [
  "Keys you type are held in memory for a moment and never saved or sent.",
  "How often you use a snippet is counted on this computer only.",
  "The log of why something did not expand lives in memory and is gone when Iamb restarts.",
];

/** Privacy: what Iamb keeps, and the crash-report switch. */
export const RESET_COPY = {
  label: "Usage counts",
  help: "How often and when each snippet was used, kept on this computer only. Resetting forgets it; your snippets do not change.",
  button: "Reset usage counts…",
  title: "Reset usage counts?",
  body: "Iamb will forget how often and when each snippet was used on this computer. “Recently used”, “Most used on this computer” and the unused list in Library health start again from nothing. Your snippets are not changed.",
  confirm: "Reset counts",
  done: "Usage counts were reset.",
} as const;

export function PrivacySection({ env, data, offline, mac }: SectionProps) {
  const { messages, fail } = useMessages();
  const [asking, setAsking] = useState(false);
  const [reset, setReset] = useState<"idle" | "done">("idle");
  const on = data.values?.["ui.crash_reports"] === "on";
  return (
    <>
      <ul className="st-statements">
        {PRIVACY_STATEMENTS.map((s) => (
          <li key={s}>{s}</li>
        ))}
      </ul>
      <Row
        label="Send crash reports"
        help="A report says where Iamb failed. It never contains what you typed, your snippets or your clipboard."
        message={messages.crash ?? null}
      >
        {(ids) => (
          <Switch
            on={on}
            labelledBy={ids.label}
            describedBy={ids.describedBy}
            disabled={offline}
            onChange={(next) => {
              void data.save("ui.crash_reports", next ? "on" : null).then((f) => fail("crash", f));
            }}
          />
        )}
      </Row>
      <Row label={RESET_COPY.label} help={RESET_COPY.help} message={messages.usage ?? null}>
        {() => (
          <>
            <button type="button" className="button" disabled={offline} onClick={() => setAsking(true)}>
              {RESET_COPY.button}
            </button>
            {reset === "done" && (
              <span className="help" role="status">
                {RESET_COPY.done}
              </span>
            )}
          </>
        )}
      </Row>
      {asking && (
        <ConfirmDialog
          title={RESET_COPY.title}
          body={RESET_COPY.body}
          confirm={RESET_COPY.confirm}
          mac={mac}
          onCancel={() => setAsking(false)}
          onConfirm={() => {
            setAsking(false);
            env.api.resetUsage().then(
              () => {
                setReset("done");
                fail("usage", null);
              },
              (e: unknown) => {
                setReset("idle");
                fail("usage", describeFailure(e, "reset_usage"));
              },
            );
          }}
        />
      )}
    </>
  );
}
