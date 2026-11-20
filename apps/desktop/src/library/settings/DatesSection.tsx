import { useId, useMemo, useState } from "react";

import { Row } from "./controls";
import {
  DATE_PRESETS,
  DEFAULT_DATE_FORMAT,
  DEFAULT_TIME_FORMAT,
  LANGUAGES,
  type Language,
  TIME_PRESETS,
  formatExample,
  languageOf,
  unknownFormatCode,
} from "./model";
import type { SectionProps } from "./section";
import { useMessages } from "./useMessages";

const CUSTOM = "custom";

const CODE_HELP: [string, string][] = [
  ["%Y %y", "Year, four digits or two"],
  ["%m %-m", "Month number, padded or not"],
  ["%B %b", "Month name, full or short"],
  ["%d %-d", "Day of the month, padded or not"],
  ["%A %a", "Weekday name, full or short"],
  ["%H %-H", "Hour 00 to 23, padded or not"],
  ["%I %-I", "Hour 01 to 12, padded or not"],
  ["%M %S", "Minute, second"],
  ["%p", "AM or PM"],
  ["%j", "Day of the year"],
  ["%%", "A percent sign"],
];

/** A list of formats shown as the current date or time, and a field of your own. */
function FormatRow({
  label,
  storedKey,
  value,
  presets,
  fallback,
  language,
  now,
  disabled,
  message,
  onSave,
}: {
  label: string;
  storedKey: string;
  value: string;
  presets: string[];
  fallback: string;
  language: Language;
  now: Date;
  disabled: boolean;
  message: { kind: "error" | "warning"; text: string } | null;
  onSave: (format: string) => void;
}) {
  const inList = presets.includes(value);
  const [custom, setCustom] = useState(inList ? null : value);
  const [draft, setDraft] = useState(value);
  const customOn = custom !== null;
  const bad = unknownFormatCode(draft);
  const selectId = useId();
  const commit = () => {
    if (bad === null && draft !== "" && draft !== value) onSave(draft);
  };
  return (
    <Row label={label} message={customOn && (bad !== null || draft === "") ? { kind: "error", text: bad !== null ? `Iamb does not know “${bad}”. See the list of date codes.` : "Write a format, for example " + fallback + "." } : message}>
      {(ids) => (
        <div className="st-stack">
          <select
            id={selectId}
            className="input st-select"
            aria-labelledby={ids.label}
            data-setting={storedKey}
            disabled={disabled}
            value={customOn ? CUSTOM : value}
            onChange={(e) => {
              if (e.target.value === CUSTOM) {
                setCustom(value);
                setDraft(value);
              } else {
                setCustom(null);
                setDraft(e.target.value);
                onSave(e.target.value);
              }
            }}
          >
            {presets.map((p) => (
              <option key={p} value={p}>
                {formatExample(p, now, language)}
              </option>
            ))}
            <option value={CUSTOM}>Custom…</option>
          </select>
          {customOn && (
            <div className="st-custom">
              <input
                className={bad !== null ? "input mono invalid" : "input mono"}
                aria-label={`${label}, custom format`}
                spellCheck={false}
                autoComplete="off"
                disabled={disabled}
                value={draft}
                aria-invalid={bad !== null}
                onChange={(e) => setDraft(e.target.value)}
                onBlur={commit}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    commit();
                  }
                }}
              />
              <span className="help" aria-live="polite">
                {bad === null && draft !== "" ? `Looks like ${formatExample(draft, now, language)}` : ""}
              </span>
            </div>
          )}
        </div>
      )}
    </Row>
  );
}

/** Dates: the date and time formats and the language of month and day names (§4.7). */
export function DatesSection({ data, offline }: SectionProps) {
  const { messages, fail } = useMessages();
  const values = data.values ?? {};
  const language = languageOf(values.locale);
  const now = useMemo(() => new Date(), []);
  return (
    <>
      <FormatRow
        label="Date format"
        storedKey="date_format"
        value={values.date_format ?? DEFAULT_DATE_FORMAT}
        presets={DATE_PRESETS}
        fallback={DEFAULT_DATE_FORMAT}
        language={language}
        now={now}
        disabled={offline}
        message={messages.date ?? null}
        onSave={(f) => void data.save("date_format", f).then((r) => fail("date", r))}
      />
      <FormatRow
        label="Time format"
        storedKey="time_format"
        value={values.time_format ?? DEFAULT_TIME_FORMAT}
        presets={TIME_PRESETS}
        fallback={DEFAULT_TIME_FORMAT}
        language={language}
        now={now}
        disabled={offline}
        message={messages.time ?? null}
        onSave={(f) => void data.save("time_format", f).then((r) => fail("time", r))}
      />
      <Row label="Month and day names in" message={messages.locale ?? null}>
        {(ids) => (
          <select
            className="input st-select"
            aria-labelledby={ids.label}
            disabled={offline}
            value={language}
            onChange={(e) => void data.save("locale", e.target.value).then((r) => fail("locale", r))}
          >
            {LANGUAGES.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </select>
        )}
      </Row>
      <details className="st-codes">
        <summary>Date codes</summary>
        <dl>
          {CODE_HELP.map(([code, meaning]) => (
            <div key={code}>
              <dt className="mono">{code}</dt>
              <dd>{meaning}</dd>
            </div>
          ))}
        </dl>
      </details>
    </>
  );
}
