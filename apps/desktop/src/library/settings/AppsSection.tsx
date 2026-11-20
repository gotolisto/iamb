import { useState } from "react";

import { Icon } from "../Icon";
import { Row } from "./controls";
import {
  type AppOverride,
  type LineBreak,
  type Method,
  type OverridePatch,
  readOverrides,
  sameApp,
  withOverride,
  withoutOverride,
} from "./model";
import type { SectionProps } from "./section";
import { useMessages } from "./useMessages";

const EXAMPLE_ID = (mac: boolean) => (mac ? "com.example.bank" : "mybank.exe");

function Exclusions({ data, mac, offline }: SectionProps) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const { messages, set, fail } = useMessages();
  const idLabel = mac ? "Bundle identifier" : "Program file name";

  const add = async () => {
    const appId = id.trim();
    if (appId === "") {
      set("add", { kind: "error", text: `Write the app's ${idLabel.toLowerCase()}, for example ${EXAMPLE_ID(mac)}.` });
      return;
    }
    const failure = await data.setExcluded(appId, name.trim() || appId, true);
    fail("add", failure);
    if (!failure) {
      setId("");
      setName("");
    }
  };

  return (
    <Row
      label="Never expand in these apps"
      help="Iamb does not store keys typed in these apps. Add password managers, remote desktops and terminals where you type passwords."
      message={messages.remove ?? null}
    >
      {(ids) => (
        <div className="st-stack" role="group" aria-labelledby={ids.label}>
          {data.exclusions.length === 0 ? (
            <p className="help">No app is excluded.</p>
          ) : (
            <ul className="st-apps" aria-label="Excluded apps">
              {data.exclusions.map((x) => (
                <li key={x.appId}>
                  <Icon name="shield" />
                  <span className="st-app-name">
                    {x.displayName}
                    {x.displayName !== x.appId && <span className="help mono"> {x.appId}</span>}
                  </span>
                  <button
                    type="button"
                    className="button"
                    disabled={offline}
                    aria-label={`Remove ${x.displayName}`}
                    onClick={() => {
                      void data.setExcluded(x.appId, x.displayName, false).then((f) => fail("remove", f));
                    }}
                  >
                    Remove
                  </button>
                </li>
              ))}
            </ul>
          )}
          <form
            className="st-add"
            onSubmit={(e) => {
              e.preventDefault();
              void add();
            }}
          >
            <div className="field">
              <label htmlFor="st-add-id">{idLabel}</label>
              <input
                id="st-add-id"
                className="input mono"
                value={id}
                spellCheck={false}
                autoComplete="off"
                placeholder={EXAMPLE_ID(mac)}
                disabled={offline}
                onChange={(e) => setId(e.target.value)}
              />
            </div>
            <div className="field">
              <label htmlFor="st-add-name">
                Name <span className="optional">(optional)</span>
              </label>
              <input
                id="st-add-name"
                className="input"
                value={name}
                autoComplete="off"
                disabled={offline}
                onChange={(e) => setName(e.target.value)}
              />
            </div>
            <button type="submit" className="button" disabled={offline}>
              Add app
            </button>
          </form>
          {messages.add && (
            <p className="field-message error" role="alert">
              {messages.add.text}
            </p>
          )}
        </div>
      )}
    </Row>
  );
}

const METHODS: { value: Method | "decide"; label: string }[] = [
  { value: "decide", label: "Let Iamb decide" },
  { value: "keys", label: "Type it key by key" },
  { value: "paste", label: "Paste it" },
];

const BREAKS: { value: LineBreak | "default"; label: string }[] = [
  { value: "default", label: "Let Iamb decide" },
  { value: "enter", label: "Enter" },
  { value: "shift+enter", label: "Shift+Enter" },
];

function OverrideCard({
  entry,
  mac,
  disabled,
  onPatch,
  onRemove,
}: {
  entry: AppOverride;
  mac: boolean;
  disabled: boolean;
  onPatch: (patch: OverridePatch) => void;
  onRemove: () => void;
}) {
  const max = mac ? 1000 : 10_000;
  const [delay, setDelay] = useState(entry.keyDelayMs === null ? "" : String(entry.keyDelayMs));
  const n = delay.trim() === "" ? null : Number(delay);
  const bad = n !== null && (!Number.isInteger(n) || n < 0 || n > max);
  const commit = () => {
    if (!bad && n !== entry.keyDelayMs) onPatch({ keyDelayMs: n });
  };
  const base = `st-ov-${entry.app}`;
  return (
    <li className="st-override">
      <div className="st-override-head">
        <strong className="mono">{entry.app}</strong>
        <button
          type="button"
          className="button"
          disabled={disabled}
          aria-label={`Remove the settings for ${entry.app}`}
          onClick={onRemove}
        >
          Remove
        </button>
      </div>
      <div className="st-override-grid">
        <label htmlFor={`${base}-method`}>Send the text</label>
        <select
          id={`${base}-method`}
          className="input st-select"
          disabled={disabled}
          value={entry.method ?? "decide"}
          onChange={(e) => onPatch({ method: e.target.value === "decide" ? null : (e.target.value as Method) })}
        >
          {METHODS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        <label htmlFor={`${base}-break`}>Line breaks as</label>
        <select
          id={`${base}-break`}
          className="input st-select"
          disabled={disabled}
          value={entry.lineBreak ?? "default"}
          onChange={(e) =>
            onPatch({ lineBreak: e.target.value === "default" ? null : (e.target.value as LineBreak) })
          }
        >
          {BREAKS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        <label htmlFor={`${base}-delay`}>Slow down by</label>
        <span className="st-delay">
          <input
            id={`${base}-delay`}
            className={bad ? "input invalid" : "input"}
            inputMode="numeric"
            disabled={disabled}
            value={delay}
            aria-invalid={bad}
            aria-describedby={`${base}-delay-help`}
            onChange={(e) => setDelay(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commit();
              }
            }}
          />
          <span> ms</span>
        </span>
        <p
          className={bad ? "field-message error st-ov-help" : "help st-ov-help"}
          id={`${base}-delay-help`}
          role={bad ? "alert" : undefined}
        >
          {bad ? `Use a whole number from 0 to ${max}.` : "A pause between key presses. Leave empty for none."}
        </p>
      </div>
    </li>
  );
}

function Overrides({ data, mac, offline }: SectionProps) {
  const text = data.values?.injection_overrides;
  const entries = readOverrides(text);
  const [id, setId] = useState("");
  const { messages, set, fail } = useMessages();

  const write = async (next: { text: string | null } | null) => {
    if (!next) {
      set("ov", { kind: "error", text: "These settings were written by hand and Iamb cannot edit them here." });
      return;
    }
    fail("ov", await data.save("injection_overrides", next.text));
  };

  return (
    <Row
      label="If snippets come out wrong in an app"
      help="Only needed when an app gets the text wrong. The compatibility notes say which apps need it."
      message={entries === null ? { kind: "warning", text: "These settings were written by hand and Iamb cannot edit them here." } : (messages.ov ?? null)}
    >
      {(ids) => (
        <div className="st-stack" role="group" aria-labelledby={ids.label}>
          {entries !== null && entries.length > 0 && (
            <ul className="st-overrides" aria-label="Apps with their own settings">
              {entries.map((entry) => (
                <OverrideCard
                  key={entry.app}
                  entry={entry}
                  mac={mac}
                  disabled={offline}
                  onPatch={(patch) => void write(withOverride(text, entry.app, patch))}
                  onRemove={() => void write(withoutOverride(text, entry.app))}
                />
              ))}
            </ul>
          )}
          {entries !== null && (
            <form
              className="st-add"
              onSubmit={(e) => {
                e.preventDefault();
                const app = id.trim();
                if (app === "") return;
                if (entries.some((x) => sameApp(x.app, app))) {
                  set("ov", { kind: "error", text: "That app is in the list already." });
                  return;
                }
                // A new app starts by pasting nothing special: its first choice is made on its card.
                void write(withOverride(text, app, { method: null })).then(() => setId(""));
              }}
            >
              <div className="field">
                <label htmlFor="st-ov-id">{mac ? "Bundle identifier" : "Program file name"}</label>
                <input
                  id="st-ov-id"
                  className="input mono"
                  value={id}
                  spellCheck={false}
                  autoComplete="off"
                  placeholder={EXAMPLE_ID(mac)}
                  disabled={offline}
                  onChange={(e) => setId(e.target.value)}
                />
              </div>
              <button type="submit" className="button" disabled={offline}>
                Add app
              </button>
            </form>
          )}
        </div>
      )}
    </Row>
  );
}

/** Apps: where Iamb stays out, and how it types in the apps that need it (§4.7, ADR-0025). */
export function AppsSection(props: SectionProps) {
  return (
    <>
      <Exclusions {...props} />
      <Overrides {...props} />
    </>
  );
}
