import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

interface VirtualListProps {
  /** How many rows there are in all; only the visible ones exist in the page. */
  count: number;
  /** Every row has this height in px. */
  rowHeight: number;
  label: string;
  multiselectable?: boolean;
  /** The index the keyboard is on (for `aria-activedescendant`), or -1. */
  activeIndex: number;
  /** The id of the row at `index`. */
  rowId: (index: number) => string;
  /** The row at `index`: an element with `role="option"` and a fixed height. */
  renderRow: (index: number) => ReactNode;
  onKeyDown: (e: React.KeyboardEvent<HTMLDivElement>) => void;
  /** Rows beyond the viewport that are drawn, above and below. */
  overscan?: number;
  /** The viewport's height when it cannot be measured (tests). */
  fallbackHeight?: number;
  /** Called with the number of rows the viewport shows, for Page Up and Page Down. */
  onPage?: (rows: number) => void;
  listRef?: React.Ref<HTMLDivElement> | undefined;
}

/**
 * A list that draws only the rows in view, for 10,000 snippets at 60 frames per second
 * (specification §10). The rows have one fixed height, so the first visible row is
 * `scrollTop / rowHeight` with no measuring; a scroll re-renders only when the first row drawn
 * changes. Keyboard focus stays on the list (`aria-activedescendant`), so a row that is scrolled
 * out of the page never loses it.
 */
export function VirtualList({
  count,
  rowHeight,
  label,
  multiselectable,
  activeIndex,
  rowId,
  renderRow,
  onKeyDown,
  overscan = 6,
  fallbackHeight = 600,
  onPage,
  listRef,
}: VirtualListProps) {
  const el = useRef<HTMLDivElement | null>(null);
  const [first, setFirst] = useState(0);
  const [height, setHeight] = useState(fallbackHeight);
  const firstRef = useRef(0);

  const setRef = useCallback(
    (node: HTMLDivElement | null) => {
      el.current = node;
      if (typeof listRef === "function") listRef(node);
      else if (listRef) listRef.current = node;
    },
    [listRef],
  );

  const measure = useCallback(() => {
    const node = el.current;
    if (!node) return;
    const h = node.clientHeight;
    if (h > 0) setHeight(h);
  }, []);

  useLayoutEffect(measure, [measure]);
  useEffect(() => {
    const node = el.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [measure]);

  useEffect(() => {
    onPage?.(Math.max(1, Math.floor(height / rowHeight) - 1));
  }, [height, rowHeight, onPage]);

  const syncFirst = useCallback(() => {
    const node = el.current;
    if (!node) return;
    const next = Math.max(0, Math.floor(node.scrollTop / rowHeight) - overscan);
    if (next !== firstRef.current) {
      firstRef.current = next;
      setFirst(next);
    }
  }, [rowHeight, overscan]);

  // The row the keyboard moved to stays in view.
  useEffect(() => {
    const node = el.current;
    if (!node || activeIndex < 0) return;
    const top = activeIndex * rowHeight;
    const view = node.clientHeight > 0 ? node.clientHeight : height;
    if (top < node.scrollTop) node.scrollTop = top;
    else if (top + rowHeight > node.scrollTop + view) node.scrollTop = top + rowHeight - view;
    // The scroll event follows in a browser; the window moves now either way.
    syncFirst();
  }, [activeIndex, rowHeight, height, syncFirst]);

  // A shorter list than before leaves no blank page.
  const clampedFirst = Math.min(first, Math.max(0, count - 1));
  const visible = Math.ceil(height / rowHeight) + overscan * 2;
  const end = Math.min(count, clampedFirst + visible);
  const rows: ReactNode[] = [];
  for (let i = clampedFirst; i < end; i++) rows.push(renderRow(i));

  return (
    <div
      ref={setRef}
      className="vlist"
      role="listbox"
      aria-label={label}
      aria-multiselectable={multiselectable ? true : undefined}
      aria-activedescendant={activeIndex >= 0 && activeIndex < count ? rowId(activeIndex) : undefined}
      tabIndex={0}
      onScroll={syncFirst}
      onKeyDown={onKeyDown}
    >
      <div className="vlist-space" role="presentation" style={{ height: count * rowHeight }}>
        <div
          className="vlist-window"
          role="presentation"
          style={{ transform: `translateY(${clampedFirst * rowHeight}px)` }}
        >
          {rows}
        </div>
      </div>
    </div>
  );
}
