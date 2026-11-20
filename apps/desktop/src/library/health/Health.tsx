import type { ReactNode } from "react";

import type { Health as HealthReport, Summary } from "../api";
import { copy, errorSentence, isClean, isTaken, lastUsedText, nameOf } from "./model";
import "./health.css";

export interface HealthProps {
  /** null until the first answer. */
  health: HealthReport | null;
  /** A sentence when the last read failed. */
  error: string | null;
  rows: ReadonlyMap<string, Summary>;
  total: number;
  onOpen: (id: string) => void;
  onTurnOff: (id: string) => void;
  onDelete: (id: string) => void;
  onRetry: () => void;
}

function Section({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="hl-section" aria-label={title}>
      <h2 className="hl-title">{title}</h2>
      {note && <p className="help">{note}</p>}
      <ul className="hl-list">{children}</ul>
    </section>
  );
}

function Entry({ id, children, actions }: { id: string; children: ReactNode; actions: ReactNode }) {
  return (
    <li className="hl-entry" data-snippet={id}>
      <p className="hl-text">{children}</p>
      <div className="hl-actions">{actions}</div>
    </li>
  );
}

function Link({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button type="button" className="button" onClick={onClick}>
      {children}
    </button>
  );
}

/**
 * Library health (specification §4.9): one section per kind of problem, each snippet with a
 * line that says what is wrong and the buttons that fix it. Sections with nothing in them are
 * not shown.
 */
export function HealthView(props: HealthProps) {
  const { health, error, rows, onOpen } = props;
  if (error && !health) {
    return (
      <div className="banner danger" role="alert">
        <p>{error}</p>
        <button type="button" className="button" onClick={props.onRetry}>
          {copy.tryAgain}
        </button>
      </div>
    );
  }
  if (!health) return null;
  if (isClean(health)) return <p>{copy.clean(props.total)}</p>;

  const abbr = (id: string) => rows.get(id)?.abbreviation ?? "";
  const errors = health.excluded.filter((e) => !isTaken(e));
  const taken = health.excluded.filter(isTaken);
  return (
    <div className="hl">
      {errors.length > 0 && (
        <Section title={copy.errors}>
          {errors.map((e) => (
            <Entry
              key={e.snippet}
              id={e.snippet}
              actions={<Link onClick={() => onOpen(e.snippet)}>{copy.open}</Link>}
            >
              <strong>{nameOf(rows.get(e.snippet))}</strong> · {errorSentence(e)}
            </Entry>
          ))}
        </Section>
      )}
      {taken.length > 0 && (
        <Section title={copy.taken}>
          {taken.map((e) => {
            const kept = e.kept ?? "";
            return (
              <Entry
                key={e.snippet}
                id={e.snippet}
                actions={
                  <>
                    <Link onClick={() => onOpen(e.snippet)}>{copy.open}</Link>
                    {kept && <Link onClick={() => onOpen(kept)}>Open {abbr(kept) || "the other"}</Link>}
                  </>
                }
              >
                <strong>{nameOf(rows.get(e.snippet))}</strong> · {abbr(e.snippet)} is used by{" "}
                {kept ? nameOf(rows.get(kept)) : "another snippet"} instead.
              </Entry>
            );
          })}
        </Section>
      )}
      {health.shadowed.length > 0 && (
        <Section title={copy.never}>
          {health.shadowed.map((s) => (
            <Entry
              key={`${s.by}-${s.hidden}`}
              id={s.hidden}
              actions={
                <>
                  <Link onClick={() => onOpen(s.by)}>Open {abbr(s.by)}</Link>
                  <Link onClick={() => onOpen(s.hidden)}>Open {abbr(s.hidden)}</Link>
                </>
              }
            >
              <strong>{nameOf(rows.get(s.hidden))}</strong> ({abbr(s.hidden)}) · {copy.neverReached(abbr(s.by))}
            </Entry>
          ))}
        </Section>
      )}
      {health.unused.length > 0 && (
        <Section title={copy.unused} note={copy.unusedNote}>
          {health.unused.map((u) => (
            <Entry
              key={u.snippet}
              id={u.snippet}
              actions={
                <>
                  <Link onClick={() => props.onTurnOff(u.snippet)}>{copy.turnOff}</Link>
                  <Link onClick={() => props.onDelete(u.snippet)}>{copy.delete}</Link>
                </>
              }
            >
              <strong>{nameOf(rows.get(u.snippet))}</strong> · {lastUsedText(u.lastUsed)}
            </Entry>
          ))}
        </Section>
      )}
    </div>
  );
}

