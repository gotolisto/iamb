import { type ReactNode, useCallback, useMemo, useRef } from "react";

import type { Summary } from "./api";
import { Icon } from "./Icon";
import {
  type Indexed,
  type SortMode,
  asksForValues,
  hasMod,
  isTemplateError,
  previewLine,
  rowTitle,
  shortcutLabel,
  speakAbbreviation,
} from "./model";
import type { OpenMenu } from "./Menu";
import { VirtualList } from "./VirtualList";

/** Rows are 48 px high (design system §4.4). */
export const ROW_HEIGHT = 48;

export interface Selection {
  ids: ReadonlySet<string>;
  /** The row the keyboard is on. */
  active: string | null;
  /** Where a Shift selection started. */
  anchor: string | null;
}

export const NO_SELECTION: Selection = { ids: new Set(), active: null, anchor: null };

export function single(id: string): Selection {
  return { ids: new Set([id]), active: id, anchor: id };
}

interface ListPaneProps {
  /** The accessible name of the list ("Snippets in Replies"). */
  listLabel: string;
  /** The filter field's label and placeholder ("Filter Replies"). */
  filterLabel: string;
  /** "Only you" under a folder's name; omitted for All snippets and tags. */
  header?: ReactNode;
  items: readonly Indexed[];
  filter: string;
  onFilter: (value: string) => void;
  sort: SortMode;
  onSort: (mode: SortMode) => void;
  /** The service counts usage, so "Most used on this computer" is offered. */
  canSortByUse?: boolean;
  selection: Selection;
  onSelection: (next: Selection) => void;
  /** Enter, or a double click: move to the editor. */
  onOpen: (id: string) => void;
  onNew: () => void;
  onDelete: (ids: string[]) => void;
  onToggle: (ids: string[]) => void;
  onDuplicate: (id: string) => void;
  onMenu: (menu: OpenMenu) => void;
  /** What shows when there is no row. */
  empty: ReactNode;
  mac: boolean;
  filterRef?: React.Ref<HTMLInputElement> | undefined;
  listRef?: React.Ref<HTMLDivElement> | undefined;
}

function rowLabel(s: Summary, index: number, total: number): string {
  const parts = [
    rowTitle(s),
    `abbreviation ${speakAbbreviation(s.abbreviation)}`,
  ];
  if (asksForValues(s.firstLine)) parts.push("asks for values");
  if (!s.enabled) parts.push("off");
  if (isTemplateError(s.broken)) parts.push("broken");
  else if (s.broken === "duplicate") parts.push("yours is used");
  parts.push(`${index + 1} of ${total}`);
  return parts.join(", ");
}

function SnippetRow({
  item,
  index,
  total,
  selected,
  active,
  onPick,
  onOpen,
  onContext,
}: {
  item: Indexed;
  index: number;
  total: number;
  selected: boolean;
  active: boolean;
  onPick: (e: React.MouseEvent, id: string) => void;
  onOpen: (id: string) => void;
  onContext: (e: React.MouseEvent, id: string) => void;
}) {
  const s = item.row;
  const classes = ["row-item"];
  if (selected) classes.push("selected");
  if (active) classes.push("active");
  if (!s.enabled) classes.push("off");
  return (
    <div
      id={`row-${s.id}`}
      role="option"
      className={classes.join(" ")}
      style={{ height: ROW_HEIGHT }}
      aria-selected={selected}
      aria-posinset={index + 1}
      aria-setsize={total}
      aria-label={rowLabel(s, index, total)}
      onMouseDown={(e) => onPick(e, s.id)}
      onDoubleClick={() => onOpen(s.id)}
      onContextMenu={(e) => onContext(e, s.id)}
    >
      <div className="row-top">
        <span className="row-title">{rowTitle(s)}</span>
        {asksForValues(s.firstLine) && (
          <span className="row-mark" title="Asks for values">
            <Icon name="field" />
          </span>
        )}
        {s.kind === "rich" && (
          <span className="row-mark" title="Formatted">
            <Icon name="area" />
          </span>
        )}
        {!s.enabled && <span className="badge">Off</span>}
        {isTemplateError(s.broken) && (
          <span className="badge badge-danger">
            <Icon name="warn" />
            Broken
          </span>
        )}
        {s.broken === "duplicate" && <span className="badge badge-warning">Yours is used</span>}
        <code className="abbr-badge" title={s.abbreviation}>
          {s.abbreviation}
        </code>
      </div>
      <div className="row-sub">{previewLine(s.firstLine)}</div>
    </div>
  );
}

/** The list pane: filter, New, the sort, and the virtualised rows. */
export function ListPane(props: ListPaneProps) {
  const { items, selection, onSelection, mac } = props;
  const page = useRef(8);
  const typed = useRef({ text: "", at: 0 });
  const indexOf = useMemo(() => new Map(items.map((i, n) => [i.row.id, n])), [items]);
  const activeIndex = selection.active === null ? -1 : (indexOf.get(selection.active) ?? -1);

  const pick = useCallback(
    (e: React.MouseEvent, id: string) => {
      if (e.button === 2 && selection.ids.has(id)) return;
      const mod = mac ? e.metaKey : e.ctrlKey;
      if (e.shiftKey && selection.anchor !== null) {
        const a = indexOf.get(selection.anchor) ?? 0;
        const b = indexOf.get(id) ?? 0;
        const [lo, hi] = a < b ? [a, b] : [b, a];
        onSelection({
          ids: new Set(items.slice(lo, hi + 1).map((i) => i.row.id)),
          active: id,
          anchor: selection.anchor,
        });
      } else if (mod) {
        const ids = new Set(selection.ids);
        if (ids.has(id)) ids.delete(id);
        else ids.add(id);
        onSelection({ ids, active: id, anchor: id });
      } else {
        onSelection(single(id));
      }
      // The list keeps the keyboard focus, so a click is followed by the arrow keys.
      const list = (e.currentTarget as HTMLElement).closest<HTMLElement>('[role="listbox"]');
      if (list) {
        requestAnimationFrame(() => {
          // Not when the click opened a dialog (leaving unsaved changes), which has the focus.
          if (!document.querySelector('[role="dialog"], [role="alertdialog"]')) {
            list.focus({ preventScroll: true });
          }
        });
      }
    },
    [indexOf, items, mac, onSelection, selection],
  );

  const context = useCallback(
    (e: React.MouseEvent, id: string) => {
      e.preventDefault();
      if (!selection.ids.has(id)) onSelection(single(id));
      props.onMenu(menuFor(props, id, selection.ids.has(id) ? [...selection.ids] : [id], e.clientX, e.clientY));
    },
    [props, selection, onSelection],
  );

  const move = (to: number, extend: boolean) => {
    const n = Math.max(0, Math.min(items.length - 1, to));
    const target = items[n];
    if (!target) return;
    if (extend && selection.anchor !== null) {
      const a = indexOf.get(selection.anchor) ?? n;
      const [lo, hi] = a < n ? [a, n] : [n, a];
      onSelection({
        ids: new Set(items.slice(lo, hi + 1).map((i) => i.row.id)),
        active: target.row.id,
        anchor: selection.anchor,
      });
    } else {
      onSelection(single(target.row.id));
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const at = activeIndex;
    const mod = hasMod(e, mac);
    const ids = selection.ids.size > 0 ? [...selection.ids] : at >= 0 && items[at] ? [items[at].row.id] : [];
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(at < 0 ? 0 : at + 1, e.shiftKey);
        return;
      case "ArrowUp":
        e.preventDefault();
        move(at < 0 ? items.length - 1 : at - 1, e.shiftKey);
        return;
      case "Home":
        e.preventDefault();
        move(0, e.shiftKey);
        return;
      case "End":
        e.preventDefault();
        move(items.length - 1, e.shiftKey);
        return;
      case "PageDown":
        e.preventDefault();
        move((at < 0 ? 0 : at) + page.current, e.shiftKey);
        return;
      case "PageUp":
        e.preventDefault();
        move((at < 0 ? 0 : at) - page.current, e.shiftKey);
        return;
      case "Enter": {
        const target = items[at];
        if (target) {
          e.preventDefault();
          props.onOpen(target.row.id);
        }
        return;
      }
      case "Delete":
      case "Backspace":
        if (e.key === "Delete" || (mac && mod)) {
          if (ids.length > 0) {
            e.preventDefault();
            props.onDelete(ids);
          }
        }
        return;
      case " ":
        if (ids.length > 0) {
          e.preventDefault();
          props.onToggle(ids);
        }
        return;
      case "ContextMenu":
      case "F10": {
        if (e.key === "F10" && !e.shiftKey) return;
        const target = items[at];
        if (!target) return;
        e.preventDefault();
        const row = document.getElementById(`row-${target.row.id}`);
        const r = (row ?? e.currentTarget).getBoundingClientRect();
        props.onMenu(menuFor(props, target.row.id, ids, r.left + 24, r.bottom));
        return;
      }
      default:
    }
    if (mod && (e.key === "d" || e.key === "D")) {
      const target = items[at];
      if (target) {
        e.preventDefault();
        props.onDuplicate(target.row.id);
      }
      return;
    }
    if (mod && (e.key === "a" || e.key === "A")) {
      e.preventDefault();
      onSelection({
        ids: new Set(items.map((i) => i.row.id)),
        active: selection.active ?? items[0]?.row.id ?? null,
        anchor: items[0]?.row.id ?? null,
      });
      return;
    }
    // Typing jumps to the next label that starts with what was typed.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && e.key !== " ") {
      const now = Date.now();
      const t = typed.current;
      t.text = now - t.at > 700 ? e.key.toLowerCase() : t.text + e.key.toLowerCase();
      t.at = now;
      const from = t.text.length > 1 ? Math.max(0, at) : at + 1;
      for (let k = 0; k < items.length; k++) {
        const n = (from + k) % items.length;
        if (items[n]?.key.startsWith(t.text)) {
          e.preventDefault();
          move(n, false);
          return;
        }
      }
    }
  };

  const onPage = useCallback((rows: number) => {
    page.current = rows;
  }, []);

  return (
    <section className="pane list-pane" aria-label={props.listLabel} data-pane="list">
      <div className="list-head">
        <div className="list-tools">
          <label className="filter">
            <Icon name="search" />
            <input
              ref={props.filterRef}
              type="search"
              className="filter-input"
              placeholder={props.filterLabel}
              aria-label={props.filterLabel}
              aria-keyshortcuts={mac ? "Meta+F" : "Control+F"}
              value={props.filter}
              onChange={(e) => props.onFilter(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape" && props.filter !== "") {
                  e.preventDefault();
                  e.stopPropagation();
                  props.onFilter("");
                } else if (e.key === "ArrowDown") {
                  e.preventDefault();
                  const list = (e.currentTarget.closest("section") as HTMLElement | null)?.querySelector<HTMLElement>(
                    '[role="listbox"]',
                  );
                  list?.focus();
                  if (selection.active === null && items[0]) onSelection(single(items[0].row.id));
                }
              }}
            />
          </label>
          <button
            type="button"
            className="button primary"
            onClick={props.onNew}
            aria-keyshortcuts={mac ? "Meta+N" : "Control+N"}
          >
            <Icon name="plus" />
            New{" "}
            <kbd className="kbd">{shortcutLabel({ key: "N" }, mac)}</kbd>
          </button>
        </div>
        <div className="list-sub">
          {props.header}
          <label className="sort">
            <span className="visually-hidden">Sort by</span>
            <select
              value={props.sort}
              onChange={(e) =>
                props.onSort(e.target.value === "changed" || e.target.value === "used" ? e.target.value : "label")
              }
              aria-label="Sort by"
            >
              <option value="label">Label</option>
              <option value="changed">Recently changed</option>
              {props.canSortByUse && <option value="used">Most used on this computer</option>}
            </select>
          </label>
        </div>
      </div>
      <div className="list-body">
        {items.length === 0 ? (
          <div className="empty-wrap">{props.empty}</div>
        ) : (
          <VirtualListHost
            {...props}
            activeIndex={activeIndex}
            onKeyDown={onKeyDown}
            onPage={onPage}
            renderItem={(item, index) => (
              <SnippetRow
                key={item.row.id}
                item={item}
                index={index}
                total={items.length}
                selected={selection.ids.has(item.row.id)}
                active={selection.active === item.row.id}
                onPick={pick}
                onOpen={props.onOpen}
                onContext={context}
              />
            )}
          />
        )}
      </div>
      <p className="visually-hidden" role="status">
        {props.filter ? `${items.length} ${items.length === 1 ? "snippet matches" : "snippets match"}` : ""}
      </p>
    </section>
  );
}


function VirtualListHost(
  props: ListPaneProps & {
    activeIndex: number;
    onKeyDown: (e: React.KeyboardEvent<HTMLDivElement>) => void;
    onPage: (rows: number) => void;
    renderItem: (item: Indexed, index: number) => ReactNode;
  },
) {
  const { items } = props;
  return (
    <VirtualList
      count={items.length}
      rowHeight={ROW_HEIGHT}
      label={props.listLabel}
      multiselectable
      activeIndex={props.activeIndex}
      rowId={(i) => `row-${items[i]?.row.id ?? ""}`}
      renderRow={(i) => {
        const item = items[i];
        return item ? props.renderItem(item, i) : null;
      }}
      onKeyDown={props.onKeyDown}
      onPage={props.onPage}
      listRef={props.listRef}
    />
  );
}

function menuFor(props: ListPaneProps, id: string, ids: string[], x: number, y: number): OpenMenu {
  const many = ids.length > 1;
  const row = props.items.find((i) => i.row.id === id)?.row;
  const turnOff = ids.some((i) => props.items.find((it) => it.row.id === i)?.row.enabled);
  const label = many ? `${ids.length} snippets` : row ? rowTitle(row) : "Snippet";
  return {
    x,
    y,
    label,
    items: [
      ...(many ? [] : [{ label: "Open", onSelect: () => props.onOpen(id) }]),
      { label: turnOff ? "Turn off" : "Turn on", onSelect: () => props.onToggle(ids) },
      ...(many
        ? []
        : [
            {
              label: "Duplicate",
              shortcut: shortcutLabel({ key: "D" }, props.mac),
              onSelect: () => props.onDuplicate(id),
            },
          ]),
      { label: "Delete", danger: true, onSelect: () => props.onDelete(ids) },
    ],
  };
}
