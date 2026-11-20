import { type ReactNode, useId } from "react";

/** A setting: its name and help on the left, its control on the right, a message below. */
export function Row({
  label,
  help,
  message,
  children,
}: {
  label: string;
  help?: ReactNode;
  /** An error or warning about this setting. */
  message?: { kind: "error" | "warning"; text: ReactNode } | null;
  children: (ids: { label: string; describedBy: string }) => ReactNode;
}) {
  const id = useId();
  const labelId = `${id}-label`;
  const describedBy = [help ? `${id}-help` : null, message ? `${id}-message` : null]
    .filter(Boolean)
    .join(" ");
  return (
    <div className="st-row">
      <div className="st-text">
        <span className="label" id={labelId}>
          {label}
        </span>
        {help && (
          <span className="help" id={`${id}-help`}>
            {help}
          </span>
        )}
      </div>
      <div className="st-control">{children({ label: labelId, describedBy })}</div>
      {message && (
        <p
          className={`field-message ${message.kind} st-message`}
          id={`${id}-message`}
          role={message.kind === "error" ? "alert" : undefined}
        >
          {message.text}
        </p>
      )}
    </div>
  );
}

/** A switch that takes effect at once (design system §4.3). */
export function Switch({
  on,
  labelledBy,
  describedBy,
  disabled,
  onChange,
}: {
  on: boolean;
  labelledBy: string;
  describedBy?: string;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-labelledby={labelledBy}
      {...(describedBy ? { "aria-describedby": describedBy } : {})}
      className={on ? "switch on" : "switch"}
      disabled={disabled}
      onClick={() => onChange(!on)}
    >
      <span className="switch-track" aria-hidden="true">
        <span className="switch-thumb" />
      </span>
      <span>{on ? "On" : "Off"}</span>
    </button>
  );
}

/** A group of radio buttons; the arrow keys move and select. */
export function Choice<T extends string>({
  label,
  value,
  options,
  describedBy,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string; help?: string }[];
  describedBy?: string;
  onChange: (value: T) => void;
}) {
  const name = useId();
  return (
    <div
      className="radios"
      role="radiogroup"
      aria-labelledby={label}
      {...(describedBy ? { "aria-describedby": describedBy } : {})}
    >
      {options.map((o) => (
        <label key={o.value} className={value === o.value ? "radio checked" : "radio"}>
          <input type="radio" name={name} checked={value === o.value} onChange={() => onChange(o.value)} />
          <span>
            {o.label}
            {o.help && <span className="help block">{o.help}</span>}
          </span>
        </label>
      ))}
    </div>
  );
}

/** A chord as key badges. */
export function Badges({ parts, spoken }: { parts: string[]; spoken?: string }) {
  return (
    <span className="st-badges" {...(spoken ? { role: "img", "aria-label": spoken } : {})}>
      {parts.map((p, i) => (
        <kbd key={`${p}-${i}`} className="kbd" aria-hidden={spoken ? true : undefined}>
          {p}
        </kbd>
      ))}
    </span>
  );
}
