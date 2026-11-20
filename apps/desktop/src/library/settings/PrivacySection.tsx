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
export function PrivacySection({ data, offline }: SectionProps) {
  const { messages, fail } = useMessages();
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
    </>
  );
}
