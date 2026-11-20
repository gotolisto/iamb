import { useLayoutEffect, useRef, type KeyboardEvent, type ReactNode } from "react";

import type { FieldView } from "../ipc";
import type { FormOpen } from "./api";
import {
  controlOf,
  copy,
  keysFor,
  titleOf,
  typeahead,
  type Keys,
  type Os,
  type Values,
} from "./model";

/** How long typed letters add up to one search in a list of options. */
const TYPEAHEAD_MS = 700;

export interface FormPanelProps {
  form: FormOpen;
  values: Values;
  /** The form shows remembered answers ("Your last answers. Start empty"). */
  remembered: boolean;
  /** The focus could not be given back: the red banner of flow 4. */
  focusLost: boolean;
  os: Os;
  onChange: (name: string, value: string) => void;
  onInsert: () => void;
  onCancel: () => void;
  onStartEmpty: () => void;
}

/** The mark of the design system (a breve and a macron), decorative. */
function Mark() {
  return (
    <svg className="ff-icon ff-mark" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path d="M1.75 6.25a2.9 2.9 0 0 0 5.8 0" />
      <path d="M9.75 7.9h4.75" strokeWidth="1.9" />
    </svg>
  );
}

function WarnIcon() {
  return (
    <svg className="ff-icon" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path d="M8 2.2 14.3 13H1.7z" />
      <path d="M8 6.5v3M8 11.3v.1" />
    </svg>
  );
}

/** Attributes that keep the web view from remembering or correcting what is typed (spec §9). */
const private_ = {
  autoComplete: "off",
  autoCorrect: "off",
  autoCapitalize: "off",
  spellCheck: false,
} as const;

interface FieldProps {
  field: FieldView;
  index: number;
  value: string;
  onChange: (value: string) => void;
}

function TextField({ field, index, value, onChange }: FieldProps) {
  const id = `ff-${index}`;
  return (
    <div className="ff-field">
      <label htmlFor={id}>{field.name}</label>
      <input
        id={id}
        className="ff-input"
        type="text"
        data-field={index}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        {...private_}
      />
    </div>
  );
}

function AreaField({ field, index, value, onChange }: FieldProps) {
  const id = `ff-${index}`;
  const ref = useRef<HTMLTextAreaElement>(null);
  // Three lines, growing to eight, then it scrolls (design system §4.13); the limits are the
  // element's own min-height and max-height.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    if (el.scrollHeight > 0) el.style.height = `${el.scrollHeight + 2}px`;
  }, [value]);
  return (
    <div className="ff-field">
      <label htmlFor={id}>{field.name}</label>
      <textarea
        id={id}
        ref={ref}
        className="ff-input ff-area"
        rows={3}
        data-field={index}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        {...private_}
      />
    </div>
  );
}

function RadioField({ field, index, value, onChange }: FieldProps) {
  const labelId = `ff-${index}-label`;
  const typed = useRef({ text: "", at: 0 });
  // Arrow keys move and choose (the radios' own behaviour); a letter jumps to an option.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key.length !== 1 || e.ctrlKey || e.metaKey || e.altKey || e.key === " ") return;
    const now = e.timeStamp;
    const text = now - typed.current.at < TYPEAHEAD_MS ? typed.current.text + e.key : e.key;
    typed.current = { text, at: now };
    const next = typeahead(field.options, value, text);
    if (next === undefined) return;
    e.preventDefault();
    onChange(next);
    const radio = e.currentTarget.querySelector<HTMLInputElement>(
      `input[data-option="${field.options.indexOf(next)}"]`,
    );
    radio?.focus();
  };
  return (
    <div className="ff-field">
      <span className="ff-label" id={labelId}>
        {field.name}
      </span>
      <div className="ff-options" role="radiogroup" aria-labelledby={labelId} onKeyDown={onKeyDown}>
        {field.options.map((option, i) => (
          <label key={option} className={option === value ? "ff-radio is-on" : "ff-radio"}>
            <input
              type="radio"
              name={`ff-${index}`}
              data-field={option === value ? index : undefined}
              data-option={i}
              checked={option === value}
              onChange={() => onChange(option)}
            />
            <span>{option}</span>
          </label>
        ))}
      </div>
    </div>
  );
}

function SelectField({ field, index, value, onChange }: FieldProps) {
  const id = `ff-${index}`;
  return (
    <div className="ff-field">
      <label htmlFor={id}>{field.name}</label>
      <select
        id={id}
        className="ff-input ff-select"
        data-field={index}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        {field.options.map((option) => (
          <option key={option} value={option}>
            {option}
          </option>
        ))}
      </select>
    </div>
  );
}

function ToggleField({ field, index, value, onChange }: FieldProps) {
  return (
    <label className="ff-check">
      <input
        type="checkbox"
        data-field={index}
        checked={value === "yes"}
        onChange={(e) => onChange(e.target.checked ? "yes" : "no")}
      />
      <span>{field.name}</span>
    </label>
  );
}

function Field(props: FieldProps) {
  switch (controlOf(props.field)) {
    case "textarea":
      return <AreaField {...props} />;
    case "radios":
      return <RadioField {...props} />;
    case "select":
      return <SelectField {...props} />;
    case "checkbox":
      return <ToggleField {...props} />;
    default:
      return <TextField {...props} />;
  }
}

/** Moves the keyboard to the first field, its content selected (spec §4.4). */
export function focusFirstField(root: ParentNode | null): void {
  const first = root?.querySelector<HTMLElement>('[data-field="0"]');
  if (!first) return;
  first.focus();
  if (first instanceof HTMLInputElement && first.type === "text") first.select();
  if (first instanceof HTMLTextAreaElement) first.select();
}

function Buttons({
  os,
  keys,
  onCancel,
  onInsert,
}: {
  os: Os;
  keys: Keys;
  onCancel: () => void;
  onInsert: () => void;
}) {
  const cancel = (
    <button
      key="cancel"
      type="button"
      className="ff-btn"
      aria-keyshortcuts={keys.aria.cancel}
      onClick={onCancel}
    >
      {copy.cancel} <kbd aria-hidden="true">{keys.cancel}</kbd>
    </button>
  );
  const insert = (
    <button
      key="insert"
      type="button"
      className="ff-btn ff-primary"
      aria-keyshortcuts={keys.aria.insert}
      onClick={onInsert}
    >
      {copy.insert} <kbd aria-hidden="true">{keys.insert}</kbd>
    </button>
  );
  // The main action is rightmost on macOS and leftmost on Windows (spec §7).
  return <div className="ff-actions">{os === "mac" ? [cancel, insert] : [insert, cancel]}</div>;
}

/** The fill-in form panel, drawn from a `FormOpen` (spec §4.4, design system §4.13, §4.14). */
export function FormPanel(props: FormPanelProps) {
  const { form, values, remembered, focusLost, os, onChange, onInsert, onCancel, onStartEmpty } =
    props;
  const keys = keysFor(os);

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    const mod = os === "mac" ? e.metaKey : e.ctrlKey;
    const target = e.target as HTMLElement;
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter" && mod) {
      e.preventDefault();
      onInsert();
    } else if (e.key === "Enter" && !e.shiftKey && !e.altKey && !mod && !e.ctrlKey && !e.metaKey) {
      // Enter inserts from every field but a several-line one; a button does its own thing.
      if (target instanceof HTMLTextAreaElement || target instanceof HTMLButtonElement) return;
      e.preventDefault();
      onInsert();
    } else if (e.key === "Backspace" && mod && remembered) {
      // Only while the form shows remembered answers: otherwise the key keeps its usual
      // meaning in a text field (delete the previous word).
      e.preventDefault();
      onStartEmpty();
    }
  };

  let hint: ReactNode = null;
  if (focusLost) {
    hint = copy.stillHere;
  } else if (remembered) {
    hint = (
      <>
        {copy.lastAnswers}{" "}
        <button
          type="button"
          className="ff-link"
          aria-keyshortcuts={keys.aria.startEmpty}
          title={keys.startEmpty}
          onClick={onStartEmpty}
        >
          {copy.startEmpty}
        </button>
      </>
    );
  } else if (form.app) {
    hint = copy.goesInto(form.app);
  }

  return (
    // A `div`, not a `form`: `dialog` is not a role a form element may have. Enter and the Insert
    // button call `onInsert` themselves.
    <div
      className="ff"
      role="dialog"
      aria-labelledby="ff-title"
      aria-describedby="ff-description"
      data-os={os}
      onKeyDown={onKeyDown}
    >
      <header className="ff-hd">
        <Mark />
        <h1 id="ff-title" className="ff-title">
          {titleOf(form)}
        </h1>
        {form.label && form.abbreviation ? (
          <span className="ff-abbr">{form.abbreviation}</span>
        ) : null}
        <span id="ff-description" className="ff-sr">
          {copy.description(form.fields.length)}
        </span>
      </header>
      <div className="ff-bd">
        {focusLost ? (
          <div className="ff-banner" role="alert">
            <WarnIcon />
            <span>
              <b>{copy.couldNotGetBack(form.app)}</b> {copy.nothingTyped}
            </span>
          </div>
        ) : null}
        {form.fields.map((field, index) => (
          <Field
            key={field.name}
            field={field}
            index={index}
            value={values[field.name] ?? ""}
            onChange={(value) => onChange(field.name, value)}
          />
        ))}
      </div>
      <footer className="ff-ft">
        <span className="ff-hint">{hint}</span>
        <Buttons os={os} keys={keys} onCancel={onCancel} onInsert={onInsert} />
      </footer>
    </div>
  );
}

/** The natural height of a rendered panel: header, all of the body, footer (CSS pixels). */
export function naturalHeight(root: ParentNode | null): number {
  if (!root) return 0;
  const part = (selector: string) => root.querySelector<HTMLElement>(selector);
  const hd = part(".ff-hd")?.offsetHeight ?? 0;
  const bd = part(".ff-bd")?.scrollHeight ?? 0;
  const ft = part(".ff-ft")?.offsetHeight ?? 0;
  return hd + bd + ft;
}
