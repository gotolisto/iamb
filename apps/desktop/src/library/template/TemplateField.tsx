// The template editor (task 3.4; UX specification §4.3, flow 2 steps 3–5 and 7, design system
// §4.6, §4.11 and §4.15): the Content field of the snippet editor, with macros as chips, the
// insert list opened by typing { or from the toolbar, a card per chip, copy and paste of chips
// as template text, the error under the field, and the live preview below.
//
// It is the Content field of the snippet editor (`../Editor.tsx`), with the props of the plain
// field it replaced: `id`, `value`, `onChange(text)`, `readOnly`, `placeholder`, `describedBy`
// and `onEscape`.
// `value` is the template text; `onChange` receives the template text after every edit. The
// text is never normalised: what is not edited is written back byte for byte (`model.ts`).
//
// Parsing and preview go through the service (ADR-0017): `ParseTemplate` after every change
// (the chips are drawn from its tokens; the text itself never waits for it), `RenderPreview`
// 150 ms after the last change.
//
// Formatted snippets (`kind="rich"`, task 3.8): the text is Markdown with the same chips, a second
// toolbar writes Markdown (bold, italic, link, list, image; `markdown.ts`), an image is added from
// the file picker or by pasting or dropping it (kept by the service as an attachment), and the
// preview shows the formatted result from the service's sanitised rendering.
//
// Privacy: nothing here logs or stores the template, the preview or test values.

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";

import type { FieldView } from "../../ipc";
import { failureCode, templateService, type TemplateError, type TemplateService } from "./api";
import { ChipCard } from "./ChipCard";
import { chipElement, draw, select, selectionIn } from "./dom";
import { errorMessage } from "./errors";
import { TplIcon } from "./Icon";
import {
  filterItems,
  INSERT_ITEMS,
  newMacro,
  newOptionalPart,
  opensCard,
  type InsertItem,
  type InsertKind,
} from "./macros";
import {
  applyTokens,
  chip,
  copySource,
  deleteBackward,
  deleteForward,
  deleteRange,
  displayOffset,
  displayText,
  docFromRaw,
  docLength,
  fieldNames,
  insertPieces,
  insertTemplate,
  insertText,
  locate,
  moveCaret,
  optionalPartOf,
  pieceStart,
  removeChips,
  selectedChip,
  selectionRange,
  serialize,
  slice,
  stateOf,
  updateChip,
  type DeleteUnit,
  type EditorState,
  type Macro,
} from "./model";
import { insertMarkdown, linkSelection, PLACEHOLDERS, toggleList, wrapSelection } from "./markdown";
import { TemplatePreview } from "./TemplatePreview";
import "./template.css";

export interface TemplateFieldProps {
  id: string;
  /** The template text. */
  value: string;
  /** Called with the template text after every edit. */
  onChange: (value: string) => void;
  /** The shared folder is read-only for this person. */
  readOnly?: boolean | undefined;
  placeholder?: string | undefined;
  /** The id of the element that holds the field's other messages. */
  describedBy?: string | undefined;
  /** Esc in the text (no list or card open): the editor returns to the list. */
  onEscape?: (() => void) | undefined;
  /** "Formatted" snippets have no cursor position (§4.3). */
  kind?: "plain" | "rich" | undefined;
  /** The service; the app's by default, a fake in tests. */
  service?: TemplateService | undefined;
  /** Show the preview below the field (default yes). */
  preview?: boolean | undefined;
  /** Test values for the preview, from "Try the form" (task 3.5). */
  previewValues?: Record<string, string> | undefined;
  /** Opens the fill-in form with these fields; shows "Try the form" in the preview. */
  onTryForm?: ((fields: FieldView[]) => void) | undefined;
  /** The library's snippets, offered in the card of "Another snippet". */
  snippets?: ReadonlyArray<{ abbreviation: string; label: string }> | undefined;
  /** The template's current error, when the service answered for the current text. */
  onErrorChange?: ((error: TemplateError | null) => void) | undefined;
  /** A formatted snippet's folder: images are kept in its profile. Without it (or a service
   * that cannot keep images) there is no Image button. */
  imageFolder?: string | undefined;
}

const isMac = () => typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** The toolbar's own items; the others are under More (flow 2, step 3). */
const TOOLBAR: InsertKind[] = ["date", "field", "cursor", "clipboard"];
const TOOLBAR_LABELS: Partial<Record<InsertKind, string>> = { cursor: "Cursor" };

interface Menu {
  /** Where the "{" that opened it is. */
  anchor: number;
  active: number;
}

interface Parsed {
  source: string;
  error: TemplateError | null;
}

/** The image types the service keeps (`iamb_store::attachments`). */
export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** The largest image the service keeps (`iamb_store::attachments::MAX_IMAGE_BYTES`). */
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024;

/** What the field says when an image cannot be added. */
export const IMAGE_REFUSED = "Iamb can add PNG, JPEG, GIF and WebP images of up to 5 MB.";
export const IMAGE_FAILED = "The image could not be added. Try again.";

/** Consecutive typing is undone as one step when it follows within this time. */
const TYPING_GROUP_MS = 1000;

export function TemplateField(props: TemplateFieldProps) {
  const { id, value, onChange, readOnly, placeholder, describedBy, onEscape, kind = "plain" } = props;
  const service = props.service ?? templateService;
  const rootRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<HTMLElement>(null);
  const stateRef = useRef<EditorState>(stateOf(docFromRaw(value), 0));
  const [, render] = useReducer((n: number) => n + 1, 0);
  const [parsed, setParsed] = useState<Parsed | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [card, setCard] = useState<number | null>(null);
  const [more, setMore] = useState<number | null>(null);
  const [labelledBy, setLabelledBy] = useState<string | null>(null);
  const history = useRef<{ undo: EditorState[]; redo: EditorState[]; typedAt: number }>({ undo: [], redo: [], typedAt: 0 });
  const composing = useRef<EditorState | null>(null);
  const focused = useRef(false);
  const ids = { error: useId(), menu: useId(), more: useId() };
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const onErrorRef = useRef(props.onErrorChange);
  onErrorRef.current = props.onErrorChange;
  const rich = kind === "rich";
  const imageFolder = props.imageFolder;
  const canAddImage = rich && !readOnly && imageFolder !== undefined && service.addAttachment !== undefined;
  const fileInput = useRef<HTMLInputElement>(null);
  const [imageError, setImageError] = useState<string | null>(null);

  // ---- Parsing (ParseTemplate after every change; the latest wins) ------------------------

  const parse = useRef({ inFlight: false, latest: "", live: true });
  const requestParse = useCallback(
    (source: string) => {
      const p = parse.current;
      p.latest = source;
      if (p.inFlight) return;
      const send = (src: string) => {
        p.inFlight = true;
        service.parse(src).then(
          (answer) => {
            if (!p.live) return;
            const current = serialize(stateRef.current.doc);
            if (src === current) {
              setParsed({ source: src, error: answer.error });
              const next = applyTokens(stateRef.current, src, answer.tokens);
              if (next !== stateRef.current) {
                stateRef.current = next;
                render();
              }
            }
          },
          () => {
            if (p.live && src === serialize(stateRef.current.doc)) setParsed({ source: src, error: null });
          },
        ).finally(() => {
          p.inFlight = false;
          if (p.live && p.latest !== src) send(p.latest);
        });
      };
      send(source);
    },
    [service],
  );

  useEffect(() => {
    parse.current.live = true;
    requestParse(serialize(stateRef.current.doc));
    return () => {
      parse.current.live = false;
    };
  }, [requestParse]);

  // A value from outside (another snippet, a reset): shown as it is, then parsed.
  useLayoutEffect(() => {
    if (value === serialize(stateRef.current.doc)) return;
    stateRef.current = stateOf(docFromRaw(value), 0);
    history.current = { undo: [], redo: [], typedAt: 0 };
    setMenu(null);
    setCard(null);
    render();
    requestParse(value);
  }, [value, requestParse]);

  // ---- Committing edits ---------------------------------------------------------------------

  const commit = useCallback(
    (next: EditorState, how: "type" | "edit" | "select" = "edit") => {
      const prev = stateRef.current;
      if (next === prev) return;
      stateRef.current = next;
      if (next.doc !== prev.doc) {
        const before = serialize(prev.doc);
        const after = serialize(next.doc);
        if (before !== after) {
          const h = history.current;
          const now = Date.now();
          if (!(how === "type" && now - h.typedAt < TYPING_GROUP_MS && h.undo.length > 0)) h.undo.push(prev);
          if (h.undo.length > 200) h.undo.shift();
          h.redo = [];
          h.typedAt = how === "type" ? now : 0;
          onChangeRef.current(after);
          requestParse(after);
        }
      }
      render();
    },
    [requestParse],
  );

  const undo = (redo = false) => {
    const h = history.current;
    const from = redo ? h.redo : h.undo;
    const to = redo ? h.undo : h.redo;
    const state = from.pop();
    if (!state) return;
    to.push(stateRef.current);
    h.typedAt = 0;
    stateRef.current = state;
    const source = serialize(state.doc);
    onChangeRef.current(source);
    requestParse(source);
    render();
  };

  // ---- Selection ----------------------------------------------------------------------------

  /** Takes the DOM selection into the state (the user moved it with the pointer or keys). */
  const syncSelection = useCallback(() => {
    const root = rootRef.current;
    if (!root || composing.current) return;
    const sel = selectionIn(root, stateRef.current.doc);
    if (!sel) return;
    const s = stateRef.current;
    if (sel.anchor !== s.anchor || sel.focus !== s.focus) {
      stateRef.current = { ...s, ...sel };
      lastPlaced.current = stateRef.current;
      render();
    }
  }, []);

  useEffect(() => {
    const onSelection = () => {
      if (focused.current) syncSelection();
    };
    document.addEventListener("selectionchange", onSelection);
    return () => document.removeEventListener("selectionchange", onSelection);
  }, [syncSelection]);

  // ---- The insert list ----------------------------------------------------------------------

  const state = stateRef.current;
  const text = displayText(state.doc);
  let menuItems: InsertItem[] = [];
  let menuOpen = false;
  if (menu) {
    const caret = state.focus;
    const query = text.slice(menu.anchor + 1, caret);
    if (state.anchor === caret && caret > menu.anchor && text[menu.anchor] === "{" && !/[\s￼]/.test(query)) {
      menuItems = filterItems(query, kind);
      menuOpen = menuItems.length > 0;
    }
  }
  const active = menuOpen && menu ? Math.min(menu.active, menuItems.length - 1) : 0;

  useEffect(() => {
    if (menu && !menuOpen) setMenu(null);
  }, [menu, menuOpen]);

  // ---- The error ----------------------------------------------------------------------------

  const source = serialize(state.doc);
  const current = parsed !== null && parsed.source === source;
  const error = parsed?.error ?? null;
  let errorRange: [number, number] | null = null;
  let errorMacro: Macro | null = null;
  let errorChip: number | null = null;
  if (error && current && error.start !== null && error.end !== null) {
    const from = displayOffset(state.doc, error.start, true);
    const to = Math.max(from + 1, displayOffset(state.doc, error.end));
    errorRange = [from, Math.min(to, docLength(state.doc))];
    const { index } = locate(state.doc, from);
    const p = state.doc[index];
    if (p?.kind === "chip") {
      errorMacro = p.macro;
      errorChip = p.id;
    }
  }
  const message = error ? errorMessage(error, errorMacro) : null;

  useEffect(() => {
    if (current) onErrorRef.current?.(error);
  }, [current, error]);

  // ---- Drawing ------------------------------------------------------------------------------

  const selected = selectedChip(state);
  const drawn = useRef<{ doc: unknown; selected: number | null; error: string } | null>(null);
  const lastPlaced = useRef<EditorState | null>(null);
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root || composing.current) return;
    const key = { doc: state.doc, selected, error: errorRange ? errorRange.join(":") : "" };
    const d = drawn.current;
    if (!d || d.doc !== key.doc || d.selected !== key.selected || d.error !== key.error) {
      draw(root, state.doc, { selectedChip: selected, error: errorRange });
      drawn.current = key;
    }
    // The DOM selection follows the state when the state changed (an edit, a key, a card), not
    // on every render: the user may have moved it since.
    const placed = lastPlaced.current;
    const redrawn = drawn.current !== d;
    if (focused.current && (redrawn || !placed || placed.doc !== state.doc || placed.anchor !== state.anchor || placed.focus !== state.focus)) {
      select(root, state.doc, state.anchor, state.focus);
    }
    lastPlaced.current = state;
    // The card sits under its chip.
    if (card !== null && cardRef.current && wrapRef.current) {
      const index = state.doc.findIndex((p) => p.kind === "chip" && p.id === card);
      const el = index >= 0 ? chipElement(root, index) : null;
      if (el) {
        const wrap = wrapRef.current.getBoundingClientRect();
        const r = el.getBoundingClientRect();
        const maxLeft = Math.max(0, wrap.width - cardRef.current.offsetWidth);
        cardRef.current.style.top = `${r.bottom - wrap.top + 4}px`;
        cardRef.current.style.left = `${Math.min(Math.max(0, r.left - wrap.left), maxLeft)}px`;
      }
    }
  });

  // The label of the field names the text box (a `<label for>` cannot label a div).
  useEffect(() => {
    const label = Array.from(document.getElementsByTagName("label")).find((l) => l.htmlFor === id);
    if (!label) return;
    if (!label.id) label.id = `${id}-label`;
    setLabelledBy(label.id);
    const focus = () => rootRef.current?.focus();
    label.addEventListener("click", focus);
    return () => label.removeEventListener("click", focus);
  }, [id]);

  // ---- Edits --------------------------------------------------------------------------------

  const openCardFor = (chipId: number) => {
    setMenu(null);
    setCard(chipId);
  };

  const insertItem = (kindOf: InsertKind, at?: EditorState) => {
    const s = at ?? stateRef.current;
    const fields = fieldNames(s.doc);
    if (kindOf === "if") {
      const [from, to] = selectionRange(s);
      const open = chip(newOptionalPart(fields));
      const inner = slice(s.doc, from, to);
      const next = insertPieces(s, [open, ...inner, chip({ name: "endif", attrs: [] })]);
      const caret = inner.length === 0 ? from + 1 : next.focus;
      commit({ ...next, anchor: caret, focus: caret });
      openCardFor(open.id);
      return;
    }
    const c = chip(newMacro(kindOf, fields));
    commit(insertPieces(s, [c]));
    if (opensCard(kindOf)) openCardFor(c.id);
  };

  const chooseFromMenu = (item: InsertItem) => {
    if (!menu) return;
    const s = stateRef.current;
    setMenu(null);
    insertItem(item.kind, { ...s, anchor: menu.anchor, focus: s.focus });
  };

  /** Focus back to the text from the editor's own controls: the selection comes back where the
   * state has it (the browser may have moved it while the focus was away). */
  const restoreOnFocus = useRef(false);
  const focusEditor = () => {
    restoreOnFocus.current = true;
    rootRef.current?.focus();
    restoreOnFocus.current = false;
  };

  const cardIndex = card === null ? -1 : state.doc.findIndex((p) => p.kind === "chip" && p.id === card);
  const cardPiece = cardIndex >= 0 ? state.doc[cardIndex] : undefined;
  const cardMacro = cardPiece?.kind === "chip" ? cardPiece.macro : null;

  useEffect(() => {
    if (card !== null && cardIndex < 0) setCard(null);
  }, [card, cardIndex]);

  const closeCard = () => {
    const index = stateRef.current.doc.findIndex((p) => p.kind === "chip" && p.id === card);
    setCard(null);
    if (index >= 0) {
      const after = pieceStart(stateRef.current.doc, index) + 1;
      stateRef.current = { ...stateRef.current, anchor: after, focus: after };
    }
    focusEditor();
    render();
  };

  // A click outside the card closes it.
  useEffect(() => {
    if (card === null) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node | null;
      // The click that opened the card can reach here after its chip was drawn again.
      if (!t || !t.isConnected) return;
      if (cardRef.current?.contains(t) || rootRef.current?.querySelector(`[data-chip="${card}"]`)?.contains(t)) return;
      setCard(null);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [card]);

  const changeChip = (macro: Macro) => {
    const s = stateRef.current;
    const index = s.doc.findIndex((p) => p.kind === "chip" && p.id === card);
    if (index < 0) return;
    const doc = updateChip(s.doc, index, macro);
    const start = pieceStart(doc, index);
    commit({ doc, anchor: start, focus: start + 1 });
  };

  const removeChip = () => {
    const s = stateRef.current;
    const index = s.doc.findIndex((p) => p.kind === "chip" && p.id === card);
    if (index < 0) return;
    const start = pieceStart(s.doc, index);
    const doc = removeChips(s.doc, [index, ...optionalPartOf(s.doc, index)]);
    setCard(null);
    commit({ doc, anchor: start, focus: start });
    focusEditor();
  };

  // Native input events: the browser never edits the text box itself (`dom.ts`).
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const onBeforeInput = (e: InputEvent) => {
      if (e.inputType === "insertCompositionText") return;
      e.preventDefault();
      if (readOnly) return;
      syncSelection();
      const s = stateRef.current;
      switch (e.inputType) {
        case "insertText":
        case "insertReplacementText": {
          const data = e.data ?? e.dataTransfer?.getData("text/plain") ?? "";
          if (!data) return;
          const next = insertText(s, data);
          commit(next, "type");
          if (data === "{") setMenu({ anchor: next.focus - 1, active: 0 });
          return;
        }
        case "insertParagraph":
        case "insertLineBreak":
          commit(insertText(s, "\n"), "type");
          return;
        case "deleteContentBackward":
          commit(deleteBackward(s, "char"));
          return;
        case "deleteWordBackward":
          commit(deleteBackward(s, "word"));
          return;
        case "deleteSoftLineBackward":
        case "deleteHardLineBackward":
          commit(deleteBackward(s, "line"));
          return;
        case "deleteContentForward":
          commit(deleteForward(s, "char"));
          return;
        case "deleteWordForward":
          commit(deleteForward(s, "word"));
          return;
        case "deleteSoftLineForward":
        case "deleteHardLineForward":
          commit(deleteForward(s, "line"));
          return;
        case "deleteContent":
        case "deleteByCut":
          commit(deleteRange(s, s.anchor, s.focus));
          return;
        case "historyUndo":
          undo();
          return;
        case "historyRedo":
          undo(true);
          return;
        default:
          // Formatting, drops and anything else: not in a plain template field.
          return;
      }
    };
    const onCompositionStart = () => {
      syncSelection();
      composing.current = stateRef.current;
    };
    const onCompositionEnd = (e: CompositionEvent) => {
      const at = composing.current;
      composing.current = null;
      drawn.current = null; // the browser changed the text box: draw it again
      if (!at || readOnly) {
        render();
        return;
      }
      if (e.data) commit(insertText(at, e.data), "type");
      else render();
    };
    const onCopy = (e: ClipboardEvent, cut: boolean) => {
      syncSelection();
      const s = stateRef.current;
      if (s.anchor === s.focus) return;
      e.preventDefault();
      e.clipboardData?.setData("text/plain", copySource(s.doc, s.anchor, s.focus));
      if (cut && !readOnly) commit(deleteRange(s, s.anchor, s.focus));
    };
    const onPaste = (e: ClipboardEvent) => {
      e.preventDefault();
      if (readOnly) return;
      syncSelection();
      const images = imageFiles(e.clipboardData);
      if (images.length > 0 && canAddImage) {
        void addImages(images);
        return;
      }
      const pasted = e.clipboardData?.getData("text/plain") ?? "";
      if (!pasted) return;
      const at = stateRef.current;
      // Pasted text is template text: its macros become chips (with its own tokens).
      service.parse(pasted).then(
        (answer) => {
          const s = stateRef.current.doc === at.doc ? at : stateRef.current;
          commit(insertTemplate(s, pasted, answer.tokens));
        },
        () => {
          const s = stateRef.current.doc === at.doc ? at : stateRef.current;
          commit(insertTemplate(s, pasted, []));
        },
      );
    };
    const copy = (e: Event) => onCopy(e as ClipboardEvent, false);
    const cut = (e: Event) => onCopy(e as ClipboardEvent, true);
    const paste = (e: Event) => onPaste(e as ClipboardEvent);
    const drop = (e: Event) => {
      e.preventDefault();
      const images = imageFiles((e as DragEvent).dataTransfer);
      if (images.length > 0 && canAddImage && !readOnly) {
        focused.current = true;
        void addImages(images);
      }
    };
    root.addEventListener("beforeinput", onBeforeInput);
    root.addEventListener("compositionstart", onCompositionStart);
    root.addEventListener("compositionend", onCompositionEnd);
    root.addEventListener("copy", copy);
    root.addEventListener("cut", cut);
    root.addEventListener("paste", paste);
    root.addEventListener("drop", drop);
    return () => {
      root.removeEventListener("beforeinput", onBeforeInput);
      root.removeEventListener("compositionstart", onCompositionStart);
      root.removeEventListener("compositionend", onCompositionEnd);
      root.removeEventListener("copy", copy);
      root.removeEventListener("cut", cut);
      root.removeEventListener("paste", paste);
      root.removeEventListener("drop", drop);
    };
  });

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (composing.current || e.nativeEvent.isComposing) return;
    syncSelection();
    const s = stateRef.current;
    const mac = isMac();
    const mod = mac ? e.metaKey : e.ctrlKey;
    if (menuOpen) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = menuItems.length;
        setMenu({ anchor: menu?.anchor ?? 0, active: (active + (e.key === "ArrowDown" ? 1 : n - 1)) % n });
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const item = menuItems[active];
        if (item) chooseFromMenu(item);
        return;
      }
      if (e.key === "Escape") {
        // Esc keeps a plain brace.
        e.preventDefault();
        setMenu(null);
        return;
      }
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "p") {
      e.preventDefault();
      previewRef.current?.focus();
      return;
    }
    if (rich && mod && !e.shiftKey && !e.altKey && !readOnly && ["b", "i", "k"].includes(e.key.toLowerCase())) {
      e.preventDefault();
      const k = e.key.toLowerCase();
      commit(
        k === "b"
          ? wrapSelection(s, "**", "**", PLACEHOLDERS.bold)
          : k === "i"
            ? wrapSelection(s, "*", "*", PLACEHOLDERS.italic)
            : linkSelection(s),
      );
      return;
    }
    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (!readOnly) undo(e.shiftKey);
      return;
    }
    if (!mac && e.ctrlKey && e.key.toLowerCase() === "y") {
      e.preventDefault();
      if (!readOnly) undo(true);
      return;
    }
    const plainKey = !e.altKey && !e.ctrlKey && !e.metaKey;
    const chipAt = selectedChip(s);
    if (plainKey && !e.shiftKey && (e.key === "Enter" || e.key === " ") && chipAt !== null && !readOnly) {
      e.preventDefault();
      const p = s.doc[chipAt];
      if (p?.kind === "chip") openCardFor(p.id);
      return;
    }
    if (plainKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      commit(moveCaret(s, e.key === "ArrowLeft" ? -1 : 1, e.shiftKey), "select");
      return;
    }
    if ((e.key === "Backspace" || e.key === "Delete") && !readOnly) {
      e.preventDefault();
      const unit: DeleteUnit = mac ? (e.metaKey ? "line" : e.altKey ? "word" : "char") : e.ctrlKey ? "word" : "char";
      commit(e.key === "Backspace" ? deleteBackward(s, unit) : deleteForward(s, unit));
      return;
    }
    if (e.key === "Escape" && !e.defaultPrevented) {
      onEscape?.();
    }
  };

  const onMouseDown = (e: ReactMouseEvent<HTMLDivElement>) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-chip]");
    if (!el || !rootRef.current?.contains(el)) return;
    e.preventDefault();
    const index = Number(el.dataset.p);
    const start = pieceStart(stateRef.current.doc, index);
    focused.current = true;
    rootRef.current.focus();
    stateRef.current = { ...stateRef.current, anchor: start, focus: start + 1 };
    render();
    const p = stateRef.current.doc[index];
    if (p?.kind === "chip" && !readOnly) openCardFor(p.id);
  };

  // ---- Toolbar ------------------------------------------------------------------------------

  const toolbarInsert = (k: InsertKind) => {
    setMore(null);
    // The selection the text box has (a pointer on the toolbar does not take the focus), or
    // had when it lost the focus (keyboard, the More menu).
    if (document.activeElement === rootRef.current) syncSelection();
    focused.current = true;
    focusEditor();
    insertItem(k);
  };
  /** A formatting button: an edit of the selection the text box has or had. */
  const format = (edit: (s: EditorState) => EditorState) => {
    setMore(null);
    if (document.activeElement === rootRef.current) syncSelection();
    focused.current = true;
    commit(edit(stateRef.current));
    focusEditor();
  };

  /** Adds images through the service and inserts their Markdown where the selection was. */
  const addImages = async (files: readonly File[]) => {
    const add = service.addAttachment;
    if (!add || imageFolder === undefined) return;
    setImageError(null);
    for (const file of files) {
      // Refused before a byte is read: a large file is neither read into memory nor sent over.
      if (!IMAGE_TYPES.includes(file.type) || file.size > MAX_IMAGE_BYTES) {
        setImageError(IMAGE_REFUSED);
        continue;
      }
      try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const added = await add.call(service, imageFolder, file.name, bytes);
        commit(insertMarkdown(stateRef.current, added.markdown));
      } catch (e) {
        setImageError(failureCode(e) === "invalid" ? IMAGE_REFUSED : IMAGE_FAILED);
      }
    }
    focusEditor();
  };

  const moreItems = INSERT_ITEMS.filter((i) => !TOOLBAR.includes(i.kind) && (kind === "plain" || i.kind !== "cursor"));
  const toolbarItems = INSERT_ITEMS.filter((i) => TOOLBAR.includes(i.kind) && (kind === "plain" || i.kind !== "cursor"));

  const describedByIds = [describedBy, message ? ids.error : null].filter(Boolean).join(" ") || undefined;
  const fields = fieldNames(state.doc);
  const empty = state.doc.length === 0;

  return (
    <div className="tpl-editor">
      {rich && !readOnly && (
        <div className="tpl-bar tpl-format-bar" role="toolbar" aria-label="Formatting">
          <button
            type="button"
            className="tpl-btn tpl-quiet"
            aria-keyshortcuts={isMac() ? "Meta+B" : "Control+B"}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => format((st) => wrapSelection(st, "**", "**", PLACEHOLDERS.bold))}
          >
            <b>B</b> Bold
          </button>
          <button
            type="button"
            className="tpl-btn tpl-quiet"
            aria-keyshortcuts={isMac() ? "Meta+I" : "Control+I"}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => format((st) => wrapSelection(st, "*", "*", PLACEHOLDERS.italic))}
          >
            <i>I</i> Italic
          </button>
          <button
            type="button"
            className="tpl-btn tpl-quiet"
            aria-keyshortcuts={isMac() ? "Meta+K" : "Control+K"}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => format(linkSelection)}
          >
            Link
          </button>
          <button
            type="button"
            className="tpl-btn tpl-quiet"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => format(toggleList)}
          >
            List
          </button>
          {canAddImage && (
            <>
              <button
                type="button"
                className="tpl-btn tpl-quiet"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  if (document.activeElement === rootRef.current) syncSelection();
                  fileInput.current?.click();
                }}
              >
                Image…
              </button>
              <input
                ref={fileInput}
                type="file"
                accept={IMAGE_TYPES.join(",")}
                hidden
                tabIndex={-1}
                aria-hidden="true"
                data-testid="image-file"
                onChange={(e) => {
                  const files = Array.from(e.currentTarget.files ?? []);
                  e.currentTarget.value = "";
                  if (files.length > 0) void addImages(files);
                }}
              />
            </>
          )}
        </div>
      )}
      {!readOnly && (
        <div className="tpl-bar" role="toolbar" aria-label="Insert">
          {toolbarItems.map((item) => (
            <button
              key={item.kind}
              type="button"
              className="tpl-btn tpl-quiet"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => toolbarInsert(item.kind)}
            >
              <TplIcon name={item.icon} />
              {TOOLBAR_LABELS[item.kind] ?? item.label}
            </button>
          ))}
          <span className="tpl-sep" />
          <div className="tpl-more">
            <button
              type="button"
              className="tpl-btn tpl-quiet"
              aria-haspopup="menu"
              aria-expanded={more !== null}
              aria-controls={more !== null ? ids.more : undefined}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setMore(more === null ? 0 : null)}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown" && more === null) {
                  e.preventDefault();
                  setMore(0);
                }
              }}
            >
              More
              <TplIcon name="down" />
            </button>
            {more !== null && (
              <MoreMenu
                id={ids.more}
                items={moreItems}
                active={more}
                onActive={setMore}
                onChoose={(item) => toolbarInsert(item.kind)}
                onClose={() => setMore(null)}
              />
            )}
          </div>
        </div>
      )}
      <div ref={wrapRef} className="tpl-wrap">
        <div
          ref={rootRef}
          id={id}
          className={`tpl-text${message && current ? " is-error" : ""}`}
          role="textbox"
          aria-multiline="true"
          aria-labelledby={labelledBy ?? undefined}
          aria-label={labelledBy ? undefined : "Content"}
          aria-describedby={describedByIds}
          aria-invalid={message ? true : undefined}
          aria-readonly={readOnly ? true : undefined}
          aria-haspopup={readOnly ? undefined : "listbox"}
          aria-controls={menuOpen ? ids.menu : undefined}
          aria-activedescendant={menuOpen ? `${ids.menu}-${active}` : undefined}
          aria-keyshortcuts={isMac() ? "Meta+Shift+P" : "Control+Shift+P"}
          contentEditable={readOnly ? "false" : "true"}
          suppressContentEditableWarning
          spellCheck
          tabIndex={0}
          data-empty={empty ? "" : undefined}
          data-placeholder={placeholder}
          onKeyDown={onKeyDown}
          onMouseDown={onMouseDown}
          onFocus={() => {
            focused.current = true;
            // Focus from the keyboard or a card: the selection comes back where it was. A click
            // places its own, which the selection events bring in.
            const root = rootRef.current;
            if (root && (restoreOnFocus.current || !selectionIn(root, stateRef.current.doc))) {
              select(root, stateRef.current.doc, stateRef.current.anchor, stateRef.current.focus);
              lastPlaced.current = stateRef.current;
            }
          }}
          onBlur={() => {
            syncSelection();
            focused.current = false;
          }}
        />
        {menuOpen && (
          <div className="tpl-menu" id={ids.menu} role="listbox" aria-label="Insert">
            {menuItems.map((item, i) => (
              <div
                key={item.kind}
                id={`${ids.menu}-${i}`}
                role="option"
                aria-selected={i === active}
                className={`tpl-mi${i === active ? " on" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  chooseFromMenu(item);
                }}
              >
                <TplIcon name={item.icon} />
                {item.label}
                <small>{item.hint}</small>
              </div>
            ))}
          </div>
        )}
        {cardMacro && (
          <div ref={cardRef} className="tpl-card-anchor">
            <ChipCard
              key={card}
              macro={cardMacro}
              service={service}
              fields={fields}
              snippets={props.snippets}
              onChange={changeChip}
              onRemove={removeChip}
              onClose={closeCard}
            />
          </div>
        )}
      </div>
      {message && (
        <p id={ids.error} className="tpl-err" role="alert">
          <TplIcon name="warn" />
          <span>
            {message.runs.map((r, i) => (r.strong ? <b key={i}>{r.text}</b> : <span key={i}>{r.text}</span>))}{" "}
            <code className="tpl-code">{message.code}</code>
            {message.action && !readOnly && (
              <>
                {" "}
                <button
                  type="button"
                  className="tpl-link"
                  onClick={() => {
                    if (message.action?.kind === "insert-endif") {
                      const s = stateRef.current;
                      const end = docLength(s.doc);
                      commit(insertPieces({ ...s, anchor: end, focus: end }, [chip({ name: "endif", attrs: [] })]));
                      focusEditor();
                    } else if (errorChip !== null) {
                      openCardFor(errorChip);
                    }
                  }}
                >
                  {message.action.label}
                </button>
              </>
            )}
          </span>
        </p>
      )}
      {imageError && (
        <p className="tpl-err" role="alert">
          <TplIcon name="warn" />
          <span>{imageError}</span>
        </p>
      )}
      {props.preview !== false && (
        <TemplatePreview
          ref={previewRef}
          rich={rich}
          source={source}
          service={service}
          values={props.previewValues}
          onTryForm={props.onTryForm}
          onBack={() => focusEditor()}
        />
      )}
    </div>
  );
}

/** The image files of a paste or a drop. */
function imageFiles(data: DataTransfer | null | undefined): File[] {
  if (!data) return [];
  return Array.from(data.files ?? []).filter((f) => f.type.startsWith("image/"));
}

function MoreMenu({
  id,
  items,
  active,
  onActive,
  onChoose,
  onClose,
}: {
  id: string;
  items: InsertItem[];
  active: number;
  onActive: (i: number) => void;
  onChoose: (item: InsertItem) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]")[active]?.focus();
  }, [active]);
  return (
    <div
      ref={ref}
      id={id}
      className="tpl-menu tpl-more-menu"
      role="menu"
      aria-label="More"
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          onActive((active + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length);
        } else if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        } else if (e.key === "Tab") {
          onClose();
        }
      }}
    >
      {items.map((item, i) => (
        <button
          key={item.kind}
          type="button"
          role="menuitem"
          tabIndex={i === active ? 0 : -1}
          className={`tpl-mi${i === active ? " on" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onChoose(item)}
        >
          <TplIcon name={item.icon} />
          {item.label}
          <small>{item.hint}</small>
        </button>
      ))}
    </div>
  );
}
