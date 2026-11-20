import { useState } from "react";

import { Buttons } from "../Dialogs";
import { Dialog } from "../Dialog";
import type { CaseMode, Trigger } from "../api";
import { Choice, Row } from "./controls";
import {
  type DelimiterGroup,
  type Mark,
  delimiterChoices,
  delimitersToStore,
  withGroup,
} from "./model";
import type { SectionProps } from "./section";
import { useMessages } from "./useMessages";

const TRIGGERS: { value: Trigger; label: string }[] = [
  { value: "delimiter", label: "After a space or punctuation" },
  { value: "immediate", label: "Immediately" },
];

const CASES: { value: CaseMode; label: string }[] = [
  { value: "exact", label: "Exactly as written" },
  { value: "insensitive", label: "Ignore" },
  { value: "adaptive", label: "Ignore, and follow mine" },
];

function Check({
  label,
  mark,
  disabled,
  describedBy,
  onChange,
}: {
  label: string;
  mark: Mark;
  disabled?: boolean;
  describedBy?: string;
  onChange: (on: boolean) => void;
}) {
  return (
    <label className="check">
      <input
        type="checkbox"
        checked={mark === "on"}
        disabled={disabled}
        ref={(el) => {
          if (el) el.indeterminate = mark === "mixed";
        }}
        {...(describedBy ? { "aria-describedby": describedBy } : {})}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}

/** Typing: what ends a word (ADR-0026) and what new snippets start as (specification §4.7). */
export function TypingSection({ data, mac, offline }: SectionProps) {
  const { messages, fail } = useMessages();
  const [ask, setAsk] = useState<"Enter" | "Tab" | null>(null);
  const values = data.values ?? {};
  const stored = values.delimiters;
  const choices = delimiterChoices(stored);

  const write = (group: DelimiterGroup, on: boolean) => {
    const next = withGroup(stored, group, on);
    void data.save("delimiters", delimitersToStore(next)).then((f) => fail("delimiters", f));
  };

  const long = choices.enter || choices.tab;
  const which = choices.enter && choices.tab ? "Enter and Tab also expand." : choices.enter ? "Enter also expands." : "Tab also expands.";

  return (
    <>
      <Row
        label="A snippet expands after"
        message={messages.delimiters ?? null}
      >
        {() => (
          <fieldset className="st-checks">
            <legend className="visually-hidden">A snippet expands after</legend>
            <Check label="A space" mark="on" disabled onChange={() => {}} />
            <Check
              label="Punctuation . , ; : ! ?"
              mark={choices.punctuation}
              disabled={offline}
              onChange={(on) => write("punctuation", on)}
            />
            <Check
              label={"Closing brackets and quotes ) ] } \" '"}
              mark={choices.closers}
              disabled={offline}
              onChange={(on) => write("closers", on)}
            />
            <Check
              label="Enter"
              mark={choices.enter ? "on" : "off"}
              disabled={offline}
              onChange={(on) => (on ? setAsk("Enter") : write("enter", false))}
            />
            <Check
              label="Tab"
              mark={choices.tab ? "on" : "off"}
              disabled={offline}
              onChange={(on) => (on ? setAsk("Tab") : write("tab", false))}
            />
            {long && (
              <p className="field-message warning st-warning">
                {which} Snippets may land in the wrong place in chats, forms and dialogs.
              </p>
            )}
          </fieldset>
        )}
      </Row>
      <Row label="New snippets expand" message={messages.trigger ?? null}>
        {(ids) => (
          <Choice
            label={ids.label}
            value={values["ui.new_trigger"] === "immediate" ? "immediate" : "delimiter"}
            options={TRIGGERS}
            onChange={(v) => {
              void data.save("ui.new_trigger", v === "delimiter" ? null : v).then((f) => fail("trigger", f));
            }}
          />
        )}
      </Row>
      <Row label="Capital letters in new snippets" message={messages.case ?? null}>
        {(ids) => (
          <Choice
            label={ids.label}
            value={
              values["ui.new_case_mode"] === "exact" || values["ui.new_case_mode"] === "adaptive"
                ? values["ui.new_case_mode"]
                : "insensitive"
            }
            options={CASES}
            onChange={(v) => {
              void data.save("ui.new_case_mode", v === "insensitive" ? null : v).then((f) => fail("case", f));
            }}
          />
        )}
      </Row>
      {ask && (
        <Dialog
          title={`Expand on ${ask}?`}
          onCancel={() => setAsk(null)}
          actions={
            <Buttons
              mac={mac}
              primary={
                <button
                  type="button"
                  className="button primary"
                  data-primary
                  onClick={() => {
                    const group = ask === "Enter" ? "enter" : "tab";
                    setAsk(null);
                    write(group, true);
                  }}
                >
                  Turn on anyway
                </button>
              }
              cancel={
                <button type="button" className="button" data-autofocus onClick={() => setAsk(null)}>
                  Cancel
                </button>
              }
            />
          }
        >
          <p>
              {ask === "Enter"
                ? "Apps act on Enter before Iamb can type: a chat sends the message, a dialog closes, a form moves on."
                : "Apps act on Tab before Iamb can type: Tab moves to the next field of a form."}{" "}
              With this on, a snippet can be typed into whatever has the focus afterwards, and Iamb deletes characters there
              first. Turn it on only if you use Iamb in plain documents.
          </p>
        </Dialog>
      )}
    </>
  );
}
