import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

import { afterNextPaint } from "../paint";
import { Icon } from "../library/Icon";
import { asksForValues, isTemplateError, speakAbbreviation } from "../library/model";
import type { TemplatePreview } from "../library/template/api";
import {
  isFocusLost,
  listenPalette,
  paletteApi,
  type ListenPalette,
  type PaletteApi,
  type PaletteEvent,
  type PaletteOpen,
  type PaletteResults,
  type PickAction,
} from "./api";
import {
  actionFor,
  bodyRanges,
  copy,
  currentOs,
  editAction,
  itemsFor,
  jump,
  keysFor,
  matchedTag,
  move,
  pieces,
  previewText,
  rangesIn,
  subLine,
  titleOf,
  verbOf,
  type Answer,
  type Item,
  type Os,
} from "./model";
import "./palette.css";

/** How long the previous results wait for an answer before they are dimmed (flow 5). */
export const SLOW_MS = 300;
/** How long before the palette says the service is not running (flow 5). */
export const DOWN_MS = 2000;
/** The preview strip follows the selection after this many milliseconds. */
export const PREVIEW_MS = 60;

export interface PaletteViewProps {
  api?: PaletteApi;
  listen?: ListenPalette;
  os?: Os;
  /** Reports a paint (tests replace the browser's frame clock). */
  onNextPaint?: (report: (paintedAtMs: number) => void) => void;
}

type Health = "ok" | "slow" | "down";

/** Text with its matches heavier and tinted (`<mark>`), not by colour alone. */
function Marked({ text, ranges }: { text: string; ranges: ReadonlyArray<readonly [number, number]> }) {
  return (
    <>
      {pieces(text, ranges).map((p, i) =>
        p.marked ? <mark key={i}>{p.text}</mark> : <span key={i}>{p.text}</span>,
      )}
    </>
  );
}

function rowLabel(item: Item, index: number, total: number): string {
  const position = `${index + 1} of ${total}`;
  switch (item.kind) {
    case "again":
      return `${copy.again(item.label)}, ${copy.badgeAgain}, ${position}`;
    case "new":
      return `${copy.newSnippet(item.label)}, ${copy.badgeNew}, ${position}`;
    case "hit": {
      const row = item.hit.row;
      const parts = [titleOf(item.hit), `abbreviation ${speakAbbreviation(row.abbreviation)}`];
      if (asksForValues(row)) parts.push("asks for values");
      if (!row.enabled) parts.push("off");
      if (row.needsReview) parts.push("needs your review");
      if (isTemplateError(row.broken)) parts.push("broken");
      parts.push(position);
      return parts.join(", ");
    }
  }
}

function ItemRow({
  item,
  index,
  total,
  selected,
  query,
  onChoose,
}: {
  item: Item;
  index: number;
  total: number;
  selected: boolean;
  query: string;
  onChoose: (index: number) => void;
}) {
  const classes = ["pl-row"];
  if (selected) classes.push("on");
  let body: ReactNode;
  if (item.kind === "again") {
    body = (
      <>
        <span className="pl-nm">
          <Icon name="field" />
          {copy.again(item.label)}
        </span>
        <span className="pl-badge">{copy.badgeAgain}</span>
      </>
    );
  } else if (item.kind === "new") {
    body = (
      <>
        <span className="pl-nm">
          <Icon name="plus" />
          {copy.newSnippet(item.label)}
        </span>
        <span className="pl-badge">{copy.badgeNew}</span>
      </>
    );
  } else {
    const { hit } = item;
    const row = hit.row;
    const tag = matchedTag(hit, query);
    body = (
      <>
        <span className="pl-nm">
          {asksForValues(row) && (
            <span className="pl-ask" title={copy.asks}>
              <Icon name="field" title={copy.asks} />
            </span>
          )}
          {row.label.trim() === "" ? (
            titleOf(hit)
          ) : (
            <Marked text={row.label} ranges={rangesIn(hit, "label")} />
          )}
        </span>
        <span className="pl-end">
          {!row.enabled && <span className="pl-badge">{copy.off}</span>}
          {row.needsReview && (
            <span className="pl-badge warn">
              <Icon name="eye" />
              {copy.review}
            </span>
          )}
          {isTemplateError(row.broken) && (
            <span className="pl-badge bad">
              <Icon name="warn" />
              {copy.broken}
            </span>
          )}
          <code className="pl-abbr" title={row.abbreviation}>
            <Marked text={row.abbreviation} ranges={rangesIn(hit, "abbreviation")} />
          </code>
        </span>
        <span className="pl-sub">
          {row.folder && <span>{row.folder} · </span>}
          {tag >= 0 && (
            <span className="pl-tag">
              <Marked text={row.tags[tag] ?? ""} ranges={rangesIn(hit, "tag")} />{" "}
            </span>
          )}
          {hit.field === "body" ? (
            <Marked text={subLine(hit)} ranges={bodyRanges(hit)} />
          ) : (
            subLine(hit)
          )}
        </span>
      </>
    );
  }
  return (
    <div
      id={`pl-opt-${index}`}
      role="option"
      aria-selected={selected}
      aria-posinset={index + 1}
      aria-setsize={total}
      aria-label={rowLabel(item, index, total)}
      className={classes.join(" ")}
      // The query box keeps the keyboard: a click chooses without moving it.
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => onChoose(index)}
    >
      {body}
    </div>
  );
}

/**
 * The search palette, created hidden at start so that it can appear within 100 ms
 * (architecture §4.5, ADR-0017; spec §4.5; flow 5).
 *
 * The Rust side (src-tauri/src/palette.rs) says a palette is open; this view fetches what to
 * show, draws the empty state while the panel is still hidden, and, once the panel is shown,
 * puts the keyboard in the query box and reports the paint. Picks and Esc go back to Rust,
 * which hides the panel and hands the focus back before anything reaches the service.
 *
 * The query lives in this component's state only. It is dropped when the palette closes: on a
 * pick, on Esc, when the shortcut is pressed again, and when Rust says the palette is over.
 */
export function PaletteView({
  api = paletteApi,
  listen = listenPalette,
  os = currentOs(),
  onNextPaint = afterNextPaint,
}: PaletteViewProps) {
  const [open, setOpen] = useState<PaletteOpen | null>(null);
  const [query, setQuery] = useState("");
  const [answer, setAnswer] = useState<Answer | null>(null);
  // The snippets used most recently on this computer: what the list shows with nothing typed.
  const [recent, setRecent] = useState<PaletteResults | null>(null);
  const [health, setHealth] = useState<Health>("ok");
  const [selected, setSelected] = useState(0);
  const [preview, setPreview] = useState<TemplatePreview | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // "Start it again" under "Iamb is not running.": idle, asked, or refused.
  const [restart, setRestart] = useState<"idle" | "starting" | "failed">("idle");
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const session = useRef<number | null>(null);
  const pendingShown = useRef<number | null>(null);
  const busy = useRef(false);
  const searchSeq = useRef(0);
  const previewSeq = useRef(0);
  const keys = keysFor(os);

  const startAgain = async () => {
    setRestart("starting");
    try {
      await api.restartService?.();
    } catch {
      setRestart("failed");
      return;
    }
    // The next key searches again; the note goes as soon as the service answers.
    inputRef.current?.focus();
  };
  useEffect(() => {
    if (health === "ok") setRestart("idle");
  }, [health]);

  const forget = useCallback(() => {
    session.current = null;
    pendingShown.current = null;
    busy.current = false;
    searchSeq.current += 1;
    previewSeq.current += 1;
    setOpen(null);
    setQuery("");
    setAnswer(null);
    setRecent(null);
    setHealth("ok");
    setSelected(0);
    setPreview(null);
    setNotice(null);
  }, []);

  const panelShown = useCallback(
    (s: number) => {
      inputRef.current?.focus();
      onNextPaint((paintedAtMs) => {
        void api.painted(s, paintedAtMs).catch(() => undefined);
      });
    },
    [api, onNextPaint],
  );

  const load = useCallback(async () => {
    let next: PaletteOpen | null;
    try {
      next = await api.current();
    } catch {
      return;
    }
    if (!next || next.session === session.current) return;
    session.current = next.session;
    busy.current = false;
    searchSeq.current += 1;
    previewSeq.current += 1;
    setQuery("");
    setAnswer(null);
    setHealth("ok");
    setSelected(0);
    setPreview(null);
    setNotice(null);
    setRecent(null);
    setOpen(next);
    const mine = next.session;
    api.recent().then(
      (r) => {
        if (session.current === mine) setRecent(r);
      },
      // Without usage the palette shows its hint.
      () => undefined,
    );
  }, [api]);

  useEffect(() => {
    let live = true;
    let stop: (() => void) | undefined;
    void listen((event: PaletteEvent) => {
      switch (event.type) {
        case "open":
          void load();
          break;
        case "shown":
          if (event.session === session.current) panelShown(event.session);
          else pendingShown.current = event.session;
          break;
        case "closed":
          if (event.session === session.current) forget();
          break;
      }
    })
      .then((unlisten) => {
        if (live) {
          stop = unlisten;
          // Listening: the Rust side may now have the hidden panel paint once.
          void api.ready().catch(() => undefined);
        } else unlisten();
      })
      .catch(() => undefined);
    // A palette that opened before this page had loaded.
    void load();
    return () => {
      live = false;
      stop?.();
    };
  }, [api, listen, load, panelShown, forget]);

  // Once a palette is rendered (still hidden): a `shown` event that came first is honoured.
  const renderedSession = open?.session ?? null;
  useLayoutEffect(() => {
    if (renderedSession === null) return;
    if (pendingShown.current === renderedSession) {
      pendingShown.current = null;
      panelShown(renderedSession);
    }
  }, [renderedSession, panelShown]);

  // ---- Searching: on every change; an answer to an older query is dropped --------------------
  const runSearch = useCallback(
    (text: string) => {
      const mine = ++searchSeq.current;
      const slow = setTimeout(() => {
        if (mine === searchSeq.current) setHealth("slow");
      }, SLOW_MS);
      const down = setTimeout(() => {
        if (mine === searchSeq.current) setHealth("down");
      }, DOWN_MS);
      api.search(text).then(
        (results) => {
          clearTimeout(slow);
          clearTimeout(down);
          if (mine !== searchSeq.current) return;
          setHealth("ok");
          setAnswer({ query: text, results });
        },
        () => {
          clearTimeout(slow);
          clearTimeout(down);
          if (mine !== searchSeq.current) return;
          setHealth("down");
        },
      );
    },
    [api],
  );

  const change = (text: string) => {
    setQuery(text);
    setSelected(0);
    setNotice(null);
    if (text.trim() === "") {
      searchSeq.current += 1;
      setAnswer(null);
      setHealth("ok");
    } else {
      runSearch(text);
    }
  };

  const items = useMemo(
    () => itemsFor(query, answer, open?.lastForm ?? null, recent),
    [query, answer, open?.lastForm, recent],
  );
  const index = items.length ? Math.min(selected, items.length - 1) : 0;
  const item: Item | undefined = items[index];

  // ---- The preview of the selected snippet, as it would be typed now -----------------------------
  const previewId = item?.kind === "hit" ? item.hit.row.id : null;
  useEffect(() => {
    const mine = ++previewSeq.current;
    if (previewId === null) {
      setPreview(null);
      return;
    }
    const timer = setTimeout(() => {
      api.preview(previewId).then(
        (p) => {
          if (mine === previewSeq.current) setPreview(p);
        },
        () => {
          if (mine === previewSeq.current) setPreview(null);
        },
      );
    }, PREVIEW_MS);
    return () => clearTimeout(timer);
  }, [api, previewId]);

  useEffect(() => {
    const el = document.getElementById(`pl-opt-${index}`);
    if (el && typeof el.scrollIntoView === "function") el.scrollIntoView({ block: "nearest" });
  }, [index, items.length]);

  // ---- Picking -----------------------------------------------------------------------------------
  const pick = useCallback(
    (action: PickAction) => {
      const s = session.current;
      if (s === null || busy.current) return;
      busy.current = true;
      setNotice(null);
      api.pick(s, action).then(
        () => {
          if (session.current === s) forget();
        },
        (e: unknown) => {
          if (session.current !== s) return;
          busy.current = false;
          if (isFocusLost(e)) setNotice(copy.couldNotGetBack(open?.app ?? null));
          else if (action.kind === "close") forget();
          else setNotice(copy.pickFailed);
        },
      );
    },
    [api, forget, open?.app],
  );

  const choose = (i: number) => {
    const chosen = items[i];
    if (chosen) pick(actionFor(chosen, open?.ownWindow ?? false));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.nativeEvent.isComposing) return;
    const mod = os === "mac" ? e.metaKey : e.ctrlKey;
    const key = e.key;
    // Enter on a button of the panel ("Start it again") presses that button, not the row.
    if (key === "Enter" && e.target instanceof HTMLButtonElement) return;
    if (key === "Escape") {
      e.preventDefault();
      pick({ kind: "close" });
    } else if (key === "ArrowDown" || key === "ArrowUp") {
      e.preventDefault();
      setSelected(move(index, key === "ArrowDown" ? 1 : -1, items.length));
    } else if (key === "PageDown" || key === "PageUp") {
      e.preventDefault();
      setSelected(jump(index, key === "PageDown" ? 1 : -1, items.length));
    } else if (key === "Enter" && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      choose(index);
    } else if (mod && !e.altKey && !e.shiftKey && key.toLowerCase() === "e") {
      e.preventDefault();
      const action = editAction(item);
      if (action) pick(action);
    } else if (mod && !e.altKey && !e.shiftKey && /^[1-9]$/.test(key)) {
      e.preventDefault();
      choose(Number(key) - 1);
    }
  };

  if (!open) {
    // Hidden and empty between palettes.
    return <div className="pl-root" />;
  }

  const typed = query.trim() !== "";
  const noMatch = typed && answer?.query.trim() === query.trim() && answer.results.hits.length === 0;
  const count =
    typed && answer?.query.trim() === query.trim()
      ? copy.count(answer.results.hits.length, answer.results.total)
      : null;
  const hitCount = answer?.query.trim() === query.trim() ? answer.results.hits.length : null;
  const announced =
    hitCount === null
      ? ""
      : [
          copy.results(hitCount),
          ...(item?.kind === "hit"
            ? [titleOf(item.hit), `abbreviation ${speakAbbreviation(item.hit.row.abbreviation)}`]
            : []),
        ].join(", ");
  const verb = item ? copy.verb[verbOf(item, open.ownWindow)] : null;
  const canEdit = item?.kind === "hit";

  return (
    <div className="pl-root">
      <div
        className="pl"
        role="dialog"
        aria-label={copy.title}
        data-health={health}
        onKeyDown={onKeyDown}
      >
        <div className="pl-q">
          <Icon name="search" />
          <input
            ref={inputRef}
            className="pl-input"
            type="text"
            role="combobox"
            aria-label={copy.title}
            aria-expanded={items.length > 0}
            aria-controls="pl-list"
            aria-autocomplete="list"
            aria-activedescendant={item ? `pl-opt-${index}` : undefined}
            placeholder={copy.placeholder}
            value={query}
            onChange={(e) => change(e.target.value)}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
          />
          {count && <span className="pl-count">{count}</span>}
        </div>
        {notice && (
          <div className="pl-notice" role="alert">
            {notice}
          </div>
        )}
        {health === "down" && (
          <div className="pl-notice" role="alert">
            <span>
              {copy.notRunning}
              {restart === "failed" ? ` ${copy.couldNotStart}` : ""}
            </span>
            {api.restartService && (
              <button
                type="button"
                className="pl-link pl-notice-action"
                disabled={restart === "starting"}
                aria-busy={restart === "starting" ? true : undefined}
                onClick={() => void startAgain()}
              >
                {restart === "starting" ? copy.starting : copy.startAgain}
              </button>
            )}
          </div>
        )}
        <div className="pl-body">
          {noMatch && (
            <div className="pl-none" role="status">
              {copy.noMatch(query.trim())}
            </div>
          )}
          {!typed && !open.lastForm && items.length === 0 && <p className="pl-hint">{copy.hint}</p>}
          <div
            id="pl-list"
            ref={listRef}
            className="pl-list"
            role="listbox"
            aria-label={copy.title}
            aria-keyshortcuts={keys.aria.rows}
            data-dim={health !== "ok" ? "true" : undefined}
          >
            {items.map((it, i) => (
              <ItemRow
                key={it.kind === "hit" ? it.hit.row.id : it.kind}
                item={it}
                index={i}
                total={items.length}
                selected={i === index}
                query={query}
                onChoose={choose}
              />
            ))}
          </div>
          {!typed && open.lastForm && items.length <= 1 && <p className="pl-hint">{copy.hint}</p>}
        </div>
        <div className="pl-pv" role="group" aria-label={copy.preview}>
          {previewText(preview)}
        </div>
        <div className="pl-ft">
          {items.length > 1 && (
            <span>
              <kbd aria-hidden="true">↑↓</kbd> {copy.move}
            </span>
          )}
          {verb && (
            <span>
              <kbd aria-hidden="true">{keys.enter}</kbd> {verb}
            </span>
          )}
          {canEdit && (
            <span>
              <kbd aria-hidden="true">{keys.edit}</kbd> {copy.edit}
            </span>
          )}
          <span className="pl-sp" />
          <span>
            <kbd aria-hidden="true">{keys.close}</kbd> {copy.close}
          </span>
        </div>
        <div className="pl-sr" role="status" aria-live="polite">
          {announced}
        </div>
      </div>
    </div>
  );
}
