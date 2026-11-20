import { useId, useState } from "react";

import type { Tag } from "./api";
import { Icon } from "./Icon";
import { sameTagName } from "./model";

interface TagInputProps {
  id: string;
  /** The ids of the tags the snippet carries. */
  value: string[];
  tags: Tag[];
  onChange: (ids: string[]) => void;
  /** Creates a tag with this name; resolves with it, or null when that failed. */
  onCreate: (name: string) => Promise<Tag | null>;
  describedBy?: string;
  readOnly?: boolean;
}

/**
 * The editor's Tags field: the tags as removable pills, and an input that adds an existing tag
 * or makes a new one. Backspace in the empty input removes the last tag.
 */
export function TagInput({ id, value, tags, onChange, onCreate, describedBy, readOnly }: TagInputProps) {
  const listId = useId();
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  const [at, setAt] = useState(-1);
  const byId = new Map(tags.map((t) => [t.id, t]));
  const query = text.trim();
  const choices = tags.filter(
    (t) => !value.includes(t.id) && t.name.toLowerCase().includes(query.toLowerCase()),
  );
  const exact = tags.find((t) => sameTagName(t.name, query));
  const canCreate = query !== "" && !exact;
  const options: ({ kind: "tag"; tag: Tag } | { kind: "new" })[] = [
    ...choices.map((tag) => ({ kind: "tag" as const, tag })),
    ...(canCreate ? [{ kind: "new" as const }] : []),
  ];

  const add = (tag: Tag) => {
    if (!value.includes(tag.id)) onChange([...value, tag.id]);
    setText("");
    setAt(-1);
  };
  const create = async () => {
    const tag = await onCreate(query);
    if (tag) add(tag);
  };
  const choose = (n: number) => {
    const option = options[n];
    if (option?.kind === "tag") add(option.tag);
    else if (option?.kind === "new") void create();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setAt((n) => Math.min(options.length - 1, n + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setAt((n) => Math.max(-1, n - 1));
    } else if (e.key === "Enter") {
      if (at >= 0) {
        e.preventDefault();
        choose(at);
      } else if (query !== "") {
        e.preventDefault();
        if (exact) add(exact);
        else void create();
      }
    } else if (e.key === "Escape" && (open || text !== "")) {
      // Closes the list, not the editor.
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      setText("");
      setAt(-1);
    } else if (e.key === "Backspace" && text === "" && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };

  return (
    <div className="tag-input">
      <ul className="pills" role="list" aria-label="Tags of this snippet">
        {value.map((tagId) => {
          const tag = byId.get(tagId);
          if (!tag) return null;
          return (
            <li key={tagId} className="pill">
              <span className="dot" style={{ background: `var(--color-tag-${tag.color})` }} aria-hidden="true" />
              <span>{tag.name}</span>
              {!readOnly && (
                <button
                  type="button"
                  className="pill-remove"
                  aria-label={`Remove tag ${tag.name}`}
                  onClick={() => onChange(value.filter((v) => v !== tagId))}
                >
                  <Icon name="x" />
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {!readOnly && (
        <div className="tag-add">
          <input
            id={id}
            className="tag-text"
            role="combobox"
            aria-expanded={open && options.length > 0}
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={at >= 0 ? `${listId}-${at}` : undefined}
            aria-describedby={describedBy}
            placeholder="Add a tag"
            autoComplete="off"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setOpen(true);
              setAt(-1);
            }}
            onFocus={() => setOpen(true)}
            onBlur={() => {
              setOpen(false);
              setAt(-1);
            }}
            onKeyDown={onKeyDown}
          />
          {open && options.length > 0 && (
            <ul id={listId} className="suggest" role="listbox" aria-label="Tags">
              {options.map((o, n) => (
                <li
                  key={o.kind === "tag" ? o.tag.id : "new"}
                  id={`${listId}-${n}`}
                  role="option"
                  aria-selected={n === at}
                  className={n === at ? "suggest-item active" : "suggest-item"}
                  // Before the input loses focus, so the click still finds the list.
                  onMouseDown={(e) => {
                    e.preventDefault();
                    choose(n);
                  }}
                >
                  {o.kind === "tag" ? (
                    <>
                      <span className="dot" style={{ background: `var(--color-tag-${o.tag.color})` }} aria-hidden="true" />
                      {o.tag.name}
                    </>
                  ) : (
                    <>Create tag “{query}”</>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
