// A chip's card (flow 2, step 4; design system §4.11): the settings of one macro, a one-line
// preview where the macro produces text, [Remove] and [Done ⏎]. Changes apply at once (the
// chip and the preview follow); Enter (outside a several-line field), Esc or Done close the
// card and put the caret after the chip.

import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { failureCode, type TemplateService } from "./api";
import { errorMessage, messageText } from "./errors";
import { TplIcon } from "./Icon";
import {
  chipIcon,
  FIELD_KINDS,
  fieldKind,
  KEYS,
  parseShift,
  SHIFT_UNITS,
  type FieldKind,
} from "./macros";
import { attr, macroSource, type Macro } from "./model";

export interface ChipCardProps {
  macro: Macro;
  onChange: (macro: Macro) => void;
  onRemove: () => void;
  onClose: () => void;
  service: TemplateService;
  /** The fill-in fields of the snippet (for "Shown when"). */
  fields: readonly string[];
  /** Snippets of the library, for "Another snippet". */
  snippets?: ReadonlyArray<{ abbreviation: string; label: string }> | undefined;
  /** Where the card sits, relative to the editor. */
  position?: { top: number; left: number } | undefined;
}

const TITLES: Record<Macro["name"], string> = {
  date: "Date",
  time: "Time",
  clipboard: "Clipboard",
  cursor: "Cursor position",
  snippet: "Another snippet",
  field: "Fill-in field",
  key: "Key press",
  wait: "Wait",
  if: "Optional part",
  else: "Else",
  endif: "End if",
};

/** `macro` with attributes set (a string), removed (null) or kept (absent from `changes`); the
 * written order is kept and new attributes go at the end. */
export function withAttrs(macro: Macro, changes: Record<string, string | null>): Macro {
  const attrs = macro.attrs
    .filter((a) => changes[a.name] !== null)
    .map((a) => (a.name in changes ? { name: a.name, value: changes[a.name] as string } : a));
  for (const [name, value] of Object.entries(changes)) {
    if (value !== null && !attrs.some((a) => a.name === name)) attrs.push({ name, value });
  }
  return { ...macro, attrs };
}

export function ChipCard(props: ChipCardProps) {
  const { macro, onClose, onRemove, position } = props;
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useEffect(() => {
    // Focus the first setting, or Done.
    const first = ref.current?.querySelector<HTMLElement>("input, select, textarea, button.tpl-primary");
    first?.focus();
    if (first instanceof HTMLInputElement && first.type === "text") first.select();
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLTextAreaElement) && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      onClose();
    }
  };

  return (
    <div
      ref={ref}
      className="tpl-card"
      role="dialog"
      aria-labelledby={titleId}
      style={position ? { top: position.top, left: position.left } : undefined}
      onKeyDown={onKeyDown}
    >
      <h5 id={titleId}>
        <TplIcon name={chipIcon(macro)} />
        {TITLES[macro.name]}
      </h5>
      <Settings {...props} />
      <div className="tpl-actions">
        <button type="button" className="tpl-btn tpl-quiet" onClick={onRemove}>
          Remove
        </button>
        <span className="tpl-grow" />
        <button type="button" className="tpl-btn tpl-primary" onClick={onClose} aria-keyshortcuts="Enter">
          Done <kbd className="tpl-kbd">⏎</kbd>
        </button>
      </div>
    </div>
  );
}

function Settings(props: ChipCardProps) {
  switch (props.macro.name) {
    case "date":
    case "time":
      return <DateSettings {...props} />;
    case "clipboard":
      return <ClipboardSettings {...props} />;
    case "cursor":
      return <p className="tpl-help">After the snippet is typed, the caret is put here.</p>;
    case "snippet":
      return <SnippetSettings {...props} />;
    case "field":
      return <FieldSettings {...props} />;
    case "key":
      return <KeySettings {...props} />;
    case "wait":
      return <WaitSettings {...props} />;
    case "if":
      return <IfSettings {...props} />;
    case "else":
      return <p className="tpl-help">The text up to “End if” is typed when the condition is not met.</p>;
    case "endif":
      return <p className="tpl-help">The optional part ends here.</p>;
  }
}

function Field({ label, help, error, children, id }: { label: string; help?: string; error?: string | null; children: ReactNode; id: string }) {
  return (
    <div className="tpl-field">
      <label htmlFor={id}>{label}</label>
      {children}
      {help && <p className="tpl-help">{help}</p>}
      {error && (
        <p className="tpl-err" role="alert">
          <TplIcon name="warn" />
          <span>{error}</span>
        </p>
      )}
    </div>
  );
}

// ---- One-line preview of the macro ("Today this gives") -----------------------------------

function useMacroPreview(service: TemplateService, macro: Macro): { text: string | null; error: string | null } {
  const [result, setResult] = useState<{ text: string | null; error: string | null }>({ text: null, error: null });
  const source = macroSource(macro);
  useEffect(() => {
    let live = true;
    const timer = setTimeout(() => {
      service.preview(source).then(
        (p) => {
          if (!live) return;
          if (p.error) setResult({ text: null, error: messageText(errorMessage(p.error, macro)) });
          else setResult({ text: p.parts.map((x) => (x.part === "text" ? x.text : "")).join(""), error: null });
        },
        (e: unknown) => {
          if (live) setResult({ text: null, error: failureCode(e) === "disconnected" ? "Iamb is not running." : null });
        },
      );
    }, 150);
    return () => {
      live = false;
      clearTimeout(timer);
    };
    // `macro` is described by its source.
  }, [service, source]);
  return result;
}

// ---- Date and time --------------------------------------------------------------------------

const DATE_FORMATS = ["%A %-d %B", "%-d %B %Y", "%d/%m/%Y", "%m/%d/%Y", "%Y-%m-%d", "%B %-d, %Y", "%a %-d %b", "%-d %b"];
const TIME_FORMATS = ["%H:%M", "%-I:%M %p", "%H:%M:%S", "%-H.%M"];
const DATE_UNITS = ["d", "bd", "w", "mo", "y"];
const TIME_UNITS = ["min", "h", "d", "bd", "w", "mo", "y"];

/** Today's date (or the time now) written in each format, from one preview of the service. */
function useSamples(service: TemplateService, name: "date" | "time", formats: readonly string[]): string[] | null {
  const [samples, setSamples] = useState<string[] | null>(null);
  useEffect(() => {
    let live = true;
    const source = [{ name, attrs: [] }, ...formats.map((f) => ({ name, attrs: [{ name: "fmt", value: f }] }))]
      .map((m) => macroSource(m))
      .join("\n");
    service.preview(source).then(
      (p) => {
        if (!live || p.error) return;
        const text = p.parts.map((x) => (x.part === "text" ? x.text : "")).join("");
        const lines = text.split("\n");
        if (lines.length === formats.length + 1) setSamples(lines);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [service, name, formats]);
  return samples;
}

function DateSettings({ macro, onChange, service }: ChipCardProps) {
  const name = macro.name === "time" ? "time" : "date";
  const formats = name === "date" ? DATE_FORMATS : TIME_FORMATS;
  const units = name === "date" ? DATE_UNITS : TIME_UNITS;
  const samples = useSamples(service, name, formats);
  const fmt = attr(macro, "fmt");
  const shift = attr(macro, "shift") ?? "";
  const terms = shift ? parseShift(shift) : [];
  const single = terms && terms.length <= 1 && (terms.length === 0 || units.includes(terms[0]?.unit ?? ""));
  const [custom, setCustom] = useState(fmt !== undefined && !formats.includes(fmt));
  const [count, setCount] = useState(terms && terms[0] ? `${terms[0].sign}${terms[0].count}` : "");
  const [unit, setUnit] = useState(terms && terms[0] ? terms[0].unit : name === "date" ? "d" : "h");
  const preview = useMacroPreview(service, macro);
  const ids = { fmt: useId(), custom: useId(), count: useId(), unit: useId(), shift: useId() };

  const setShift = (c: string, u: string) => {
    setCount(c);
    setUnit(u);
    const m = /^([+-]?)(\d{1,4})$/.exec(c.trim());
    if (!m || Number(m[2]) === 0) {
      onChange(withAttrs(macro, { shift: c.trim() === "" || (m && Number(m[2]) === 0) ? null : c.trim() + u }));
      return;
    }
    onChange(withAttrs(macro, { shift: `${m[1] || "+"}${Number(m[2])}${u}` }));
  };

  const selectValue = custom ? "custom" : (fmt ?? "");
  const errorAt = preview.error;
  return (
    <>
      <Field id={ids.fmt} label="Format">
        <select
          id={ids.fmt}
          className="tpl-input"
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "custom") {
              setCustom(true);
              return;
            }
            setCustom(false);
            onChange(withAttrs(macro, { fmt: v === "" ? null : v }));
          }}
        >
          <option value="">{samples ? `${samples[0]} (as in Settings)` : "As in Settings"}</option>
          {formats.map((f, i) => (
            <option key={f} value={f}>
              {samples?.[i + 1] ?? f}
            </option>
          ))}
          <option value="custom">Custom…</option>
        </select>
      </Field>
      {custom && (
        <Field id={ids.custom} label="Format code" help="For example %d/%m/%Y. Codes start with %.">
          <input
            id={ids.custom}
            className="tpl-input tpl-mono"
            type="text"
            value={fmt ?? ""}
            onChange={(e) => onChange(withAttrs(macro, { fmt: e.target.value === "" ? null : e.target.value }))}
          />
        </Field>
      )}
      {single ? (
        <div className="tpl-field">
          <label htmlFor={ids.count}>Shift</label>
          <div className="tpl-shift">
            <input
              id={ids.count}
              className="tpl-input"
              type="text"
              inputMode="numeric"
              placeholder="+0"
              value={count}
              onChange={(e) => setShift(e.target.value, unit)}
            />
            <select
              id={ids.unit}
              aria-label="Unit"
              className="tpl-input"
              value={unit}
              onChange={(e) => setShift(count, e.target.value)}
            >
              {units.map((u) => (
                <option key={u} value={u}>
                  {(SHIFT_UNITS[u] as [string, string])[1]}
                </option>
              ))}
            </select>
          </div>
          {units.includes("bd") && <p className="tpl-help">Working days are Monday to Friday. Public holidays are not known.</p>}
        </div>
      ) : (
        <Field id={ids.shift} label="Shift" help="For example +1mo -1d: a month later, then a day earlier.">
          <input
            id={ids.shift}
            className="tpl-input tpl-mono"
            type="text"
            value={shift}
            onChange={(e) => onChange(withAttrs(macro, { shift: e.target.value === "" ? null : e.target.value }))}
          />
        </Field>
      )}
      <div className="tpl-preview tpl-preview-small">
        <span className="tpl-ovl">{name === "date" ? "Today this gives" : "Now this gives"}</span>
        <div className="tpl-out">{preview.text ?? " "}</div>
      </div>
      {errorAt && (
        <p className="tpl-err" role="alert">
          <TplIcon name="warn" />
          <span>{errorAt}</span>
        </p>
      )}
    </>
  );
}

// ---- Clipboard ------------------------------------------------------------------------------

function ClipboardSettings({ macro, onChange }: ChipCardProps) {
  const id = useId();
  return (
    <>
      <p className="tpl-help">Types the text you last copied. The preview shows the word Clipboard instead.</p>
      <label className="tpl-check" htmlFor={id}>
        <input
          id={id}
          type="checkbox"
          checked={attr(macro, "trim") === "yes"}
          onChange={(e) => onChange(withAttrs(macro, { trim: e.target.checked ? "yes" : null }))}
        />
        Remove spaces and line breaks at both ends
      </label>
    </>
  );
}

// ---- Another snippet ------------------------------------------------------------------------

function SnippetSettings({ macro, onChange, snippets }: ChipCardProps) {
  const id = useId();
  const list = useId();
  return (
    <Field id={id} label="Abbreviation" help="That snippet’s text is typed in its place.">
      <input
        id={id}
        className="tpl-input tpl-mono"
        type="text"
        list={snippets?.length ? list : undefined}
        value={attr(macro, "abbr") ?? ""}
        onChange={(e) => onChange(withAttrs(macro, { abbr: e.target.value }))}
      />
      {snippets && snippets.length > 0 && (
        <datalist id={list}>
          {snippets.map((s) => (
            <option key={s.abbreviation} value={s.abbreviation}>
              {s.label}
            </option>
          ))}
        </datalist>
      )}
    </Field>
  );
}

// ---- Fill-in field --------------------------------------------------------------------------

function FieldSettings({ macro, onChange }: ChipCardProps) {
  const kind = fieldKind(macro);
  const ids = { name: useId(), kind: useId(), def: useId(), options: useId(), on: useId(), off: useId() };
  const options = (attr(macro, "options") ?? "").split("|").filter(Boolean);
  const [optionsText, setOptionsText] = useState(options.join("\n"));
  const name = attr(macro, "name") ?? "";

  const setKind = (k: FieldKind) => {
    const changes: Record<string, string | null> = { type: k === "text" ? (attr(macro, "type") === "text" ? "text" : null) : k };
    if (k !== "choice") changes.options = null;
    if (k !== "toggle") {
      changes.on = null;
      changes.off = null;
    }
    const def = attr(macro, "default");
    if (k === "toggle") changes.default = def === "yes" ? "yes" : null;
    else if (k === "choice") {
      const opts = optionsText.split("\n").map((o) => o.trim()).filter(Boolean);
      changes.options = opts.length ? opts.join("|") : "Option 1|Option 2";
      if (!opts.length) setOptionsText("Option 1\nOption 2");
      changes.default = def && opts.includes(def) ? def : null;
    } else if (def === "yes" || def === "no") changes.default = null;
    if (k === "text" && def?.includes("\n")) changes.default = def.replace(/\n/g, " ");
    onChange(withAttrs(macro, changes));
  };

  const setOptions = (text: string) => {
    setOptionsText(text);
    const opts = text.split("\n").map((o) => o.trim()).filter(Boolean);
    const def = attr(macro, "default");
    onChange(withAttrs(macro, { options: opts.join("|"), default: def && opts.includes(def) ? def : null }));
  };

  const def = attr(macro, "default") ?? "";
  const nameError =
    name.length === 0 ? "A fill-in field needs a name." : [...name].length > 64 ? "A name can be at most 64 characters." : null;
  const choiceOptions = optionsText.split("\n").map((o) => o.trim()).filter(Boolean);
  const optionsError = choiceOptions.some((o) => o.includes("|"))
    ? "An option cannot contain |."
    : new Set(choiceOptions).size !== choiceOptions.length
      ? "Each option can be listed once."
      : null;

  return (
    <>
      <Field id={ids.name} label="Name" help="Shown as the label in the form" error={nameError}>
        <input
          id={ids.name}
          className="tpl-input"
          type="text"
          value={name}
          onChange={(e) => onChange(withAttrs(macro, { name: e.target.value.replace(/[\r\n]/g, " ") }))}
        />
      </Field>
      <fieldset className="tpl-field tpl-kinds">
        <legend>Kind</legend>
        {FIELD_KINDS.map((k) => (
          <label key={k.kind} className="tpl-radio">
            <input type="radio" name={ids.kind} checked={kind === k.kind} onChange={() => setKind(k.kind)} />
            {k.label}
          </label>
        ))}
      </fieldset>
      {kind === "choice" && (
        <Field id={ids.options} label="Options" help="One per line." error={optionsError}>
          <textarea id={ids.options} className="tpl-input" rows={3} value={optionsText} onChange={(e) => setOptions(e.target.value)} />
        </Field>
      )}
      {kind === "text" && (
        <Field id={ids.def} label="Starts with">
          <input
            id={ids.def}
            className="tpl-input"
            type="text"
            value={def}
            onChange={(e) => onChange(withAttrs(macro, { default: e.target.value === "" ? null : e.target.value }))}
          />
        </Field>
      )}
      {kind === "area" && (
        <Field id={ids.def} label="Starts with">
          <textarea
            id={ids.def}
            className="tpl-input"
            rows={2}
            value={def}
            onChange={(e) => onChange(withAttrs(macro, { default: e.target.value === "" ? null : e.target.value }))}
          />
        </Field>
      )}
      {kind === "choice" && choiceOptions.length > 0 && (
        <Field id={ids.def} label="Starts with">
          <select
            id={ids.def}
            className="tpl-input"
            value={def && choiceOptions.includes(def) ? def : (choiceOptions[0] ?? "")}
            onChange={(e) =>
              onChange(withAttrs(macro, { default: e.target.value === choiceOptions[0] ? null : e.target.value }))
            }
          >
            {choiceOptions.map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        </Field>
      )}
      {kind === "toggle" && (
        <>
          <label className="tpl-check" htmlFor={ids.def}>
            <input
              id={ids.def}
              type="checkbox"
              checked={def === "yes"}
              onChange={(e) => onChange(withAttrs(macro, { default: e.target.checked ? "yes" : null }))}
            />
            Starts checked
          </label>
          <Field id={ids.on} label="Types when on">
            <input
              id={ids.on}
              className="tpl-input"
              type="text"
              value={attr(macro, "on") ?? ""}
              onChange={(e) => onChange(withAttrs(macro, { on: e.target.value === "" ? null : e.target.value }))}
            />
          </Field>
          <Field id={ids.off} label="Types when off">
            <input
              id={ids.off}
              className="tpl-input"
              type="text"
              value={attr(macro, "off") ?? ""}
              onChange={(e) => onChange(withAttrs(macro, { off: e.target.value === "" ? null : e.target.value }))}
            />
          </Field>
        </>
      )}
    </>
  );
}

// ---- Optional part --------------------------------------------------------------------------

function IfSettings({ macro, onChange, fields }: ChipCardProps) {
  const ids = { field: useId(), list: useId(), cond: useId(), value: useId() };
  const field = attr(macro, "field") ?? "";
  const is = attr(macro, "is");
  const not = attr(macro, "not");
  const cond = is !== undefined ? "is" : not !== undefined ? "not" : "on";
  const value = is ?? not ?? "";
  const setCond = (c: "on" | "is" | "not", v: string) =>
    onChange(withAttrs(macro, { is: c === "is" ? v : null, not: c === "not" ? v : null }));
  return (
    <>
      <Field id={ids.field} label="Shown when" help="A fill-in field of this snippet. A new name adds a toggle to the form.">
        <input
          id={ids.field}
          className="tpl-input"
          type="text"
          list={ids.list}
          value={field}
          onChange={(e) => onChange(withAttrs(macro, { field: e.target.value }))}
        />
        <datalist id={ids.list}>
          {fields.map((f) => (
            <option key={f} value={f} />
          ))}
        </datalist>
      </Field>
      <fieldset className="tpl-field tpl-kinds">
        <legend className="tpl-sr">Condition</legend>
        <label className="tpl-radio">
          <input type="radio" name={ids.cond} checked={cond === "on"} onChange={() => setCond("on", value)} />
          is on
        </label>
        <label className="tpl-radio">
          <input type="radio" name={ids.cond} checked={cond === "is"} onChange={() => setCond("is", value)} />
          is
        </label>
        <label className="tpl-radio">
          <input type="radio" name={ids.cond} checked={cond === "not"} onChange={() => setCond("not", value)} />
          is not
        </label>
      </fieldset>
      {cond !== "on" && (
        <Field id={ids.value} label="Value">
          <input
            id={ids.value}
            className="tpl-input"
            type="text"
            value={value}
            onChange={(e) => setCond(cond, e.target.value)}
          />
        </Field>
      )}
      <p className="tpl-help">“Is on” means a toggle is checked, or a field is not empty.</p>
    </>
  );
}

// ---- Key press ------------------------------------------------------------------------------

function KeySettings({ macro, onChange }: ChipCardProps) {
  const ids = { key: useId(), shift: useId(), times: useId() };
  const [times, setTimes] = useState(attr(macro, "times") ?? "1");
  const n = Number(times);
  const timesError = /^\d+$/.test(times.trim()) && n >= 1 && n <= 50 ? null : "Press it 1 to 50 times.";
  return (
    <>
      <Field id={ids.key} label="Key">
        <select
          id={ids.key}
          className="tpl-input"
          value={attr(macro, "name") ?? "enter"}
          onChange={(e) => onChange(withAttrs(macro, { name: e.target.value }))}
        >
          {KEYS.map((k) => (
            <option key={k.name} value={k.name}>
              {k.label === k.spoken ? k.label : `${k.label} ${k.spoken}`}
            </option>
          ))}
        </select>
      </Field>
      <label className="tpl-check" htmlFor={ids.shift}>
        <input
          id={ids.shift}
          type="checkbox"
          checked={attr(macro, "shift") === "yes"}
          onChange={(e) => onChange(withAttrs(macro, { shift: e.target.checked ? "yes" : null }))}
        />
        Hold Shift
      </label>
      <Field id={ids.times} label="Times" error={timesError}>
        <input
          id={ids.times}
          className="tpl-input"
          type="text"
          inputMode="numeric"
          value={times}
          onChange={(e) => {
            setTimes(e.target.value);
            const v = Number(e.target.value);
            if (/^\d+$/.test(e.target.value.trim()) && v >= 1 && v <= 50) {
              onChange(withAttrs(macro, { times: v === 1 ? null : String(v) }));
            }
          }}
        />
      </Field>
    </>
  );
}

// ---- Wait -----------------------------------------------------------------------------------

function WaitSettings({ macro, onChange }: ChipCardProps) {
  const id = useId();
  const ms = Number(attr(macro, "ms") ?? "500");
  const [text, setText] = useState(Number.isFinite(ms) ? String(ms / 1000) : "");
  const secs = Number(text.replace(",", "."));
  const valid = text.trim() !== "" && Number.isFinite(secs) && secs >= 0.001 && secs <= 10;
  return (
    <Field
      id={id}
      label="Seconds"
      help="A pause before the next step. All waits together can be at most 30 seconds."
      error={valid ? null : "Wait at least 0.001 and at most 10 seconds."}
    >
      <input
        id={id}
        className="tpl-input"
        type="text"
        inputMode="decimal"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          const v = Number(e.target.value.replace(",", "."));
          if (e.target.value.trim() !== "" && Number.isFinite(v) && v >= 0.001 && v <= 10) {
            onChange(withAttrs(macro, { ms: String(Math.round(v * 1000)) }));
          }
        }}
      />
    </Field>
  );
}
