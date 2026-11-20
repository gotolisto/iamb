import {
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";

import type { AbbreviationCheck, CaseMode, Folder, Saved, Snippet, Tag, Trigger } from "./api";
import { TemplateField, templateService } from "./template";
import { hasFormatting } from "./template/markdown";
import { ConfirmDialog } from "./Dialogs";
import { describeFailure, type Failure } from "./errors";
import { Icon } from "./Icon";
import { Menu, type OpenMenu, below } from "./Menu";
import {
  type Message,
  abbreviationMessages,
  ariaShortcut,
  folderOptions,
  hasMod,
  messageText,
  optionsSummary,
  shortcutLabel,
  usageText,
} from "./model";
import { TagInput } from "./TagInput";
import type { LibraryData } from "./useLibraryData";
import type { LibraryEnv } from "./env";
import type { FieldView } from "../ipc";

/** Exactly the specification's copy (flow 2, step 2). */
export const CONTENT_PLACEHOLDER =
  "The text Iamb types for you. Type { to insert a date, a fill-in field and more.";

/** The question before a formatted snippet becomes plain text and would lose its formatting
 * (specification §4.3, "Kinds"; the wording is ours: the specification gives none). */
export const TO_PLAIN_TITLE = "Switch to plain text?";
export const TO_PLAIN_BODY =
  "The formatting would be lost. The text stays as it is, so marks such as ** and - are typed as written, and images become their names.";
export const TO_PLAIN_CONFIRM = "Switch to plain text";

/** What a new snippet starts with. */
export interface DraftSeed {
  /** Identifies the draft; a new one remounts the editor. */
  key: string;
  folderId: string;
  abbreviation: string;
  label: string;
  body: string;
  trigger: Trigger;
  caseMode: CaseMode;
  tags: string[];
}

export type EditorTarget = { kind: "snippet"; id: string } | { kind: "draft"; seed: DraftSeed };

/** What the window can ask of the editor it holds. */
export interface EditorHandle {
  isDirty: () => boolean;
  /** Saves; resolves with whether the snippet was written. */
  save: () => Promise<boolean>;
  /** What the snippet is called in the leave dialog. */
  label: () => string;
  /** The last failure of a save, as a sentence. */
  lastError: () => string | null;
  focus: () => void;
  /** Opens or closes Options (Ctrl+Shift+O, wherever the focus is in the window). */
  toggleOptions: () => void;
}

interface Form {
  abbreviation: string;
  label: string;
  body: string;
  folderId: string;
  tags: string[];
  trigger: Trigger;
  caseMode: CaseMode;
  wordStart: boolean;
  enabled: boolean;
  kind: "plain" | "rich";
}

function fromSnippet(s: Snippet): Form {
  return {
    abbreviation: s.abbreviation,
    label: s.label,
    body: s.body,
    folderId: s.folderId,
    tags: s.tags,
    trigger: s.trigger,
    caseMode: s.caseMode,
    wordStart: s.wordStart,
    enabled: s.enabled,
    kind: s.kind,
  };
}

function fromSeed(seed: DraftSeed): Form {
  return {
    abbreviation: seed.abbreviation,
    label: seed.label,
    body: seed.body,
    folderId: seed.folderId,
    tags: seed.tags,
    trigger: seed.trigger,
    caseMode: seed.caseMode,
    wordStart: seed.trigger === "delimiter",
    enabled: true,
    kind: "plain",
  };
}

function sameForm(a: Form, b: Form): boolean {
  const tags = (f: Form) => [...f.tags].sort().join(",");
  return (
    a.abbreviation === b.abbreviation &&
    a.label === b.label &&
    a.body === b.body &&
    a.folderId === b.folderId &&
    a.trigger === b.trigger &&
    a.caseMode === b.caseMode &&
    a.wordStart === b.wordStart &&
    a.enabled === b.enabled &&
    a.kind === b.kind &&
    tags(a) === tags(b)
  );
}

function swapFirstLetter(text: string): string {
  const i = [...text].findIndex((c) => c.toLowerCase() !== c.toUpperCase());
  if (i < 0) return text;
  const chars = [...text];
  chars[i] = (chars[i] ?? "").toUpperCase();
  return chars.join("");
}

interface EditorProps {
  env: LibraryEnv;
  data: LibraryData;
  target: EditorTarget;
  mac: boolean;
  /** A save wrote the row. */
  onSaved: (saved: Saved, form: { abbreviation: string; trigger: Trigger }, wasNew: boolean) => void;
  /** Cancel, Esc, "Close": back to the list. The window decides whether to ask first. */
  onClose: () => void;
  /**
   * "Close" on the notice that the snippet no longer exists: the window drops it from the
   * selection, whether or not its list has caught up with the deletion yet.
   */
  onGone: () => void;
  onDelete: (snippet: Snippet) => void;
  onDuplicate: (snippet: Snippet) => void;
  onShowSnippet: (id: string) => void;
  onCreateTag: (name: string) => Promise<Tag | null>;
  ref?: React.Ref<EditorHandle>;
}

type Notice = {
  tone: "danger" | "warning";
  text: string;
  actions: { label: string; run: () => void }[];
};

/** The editor of one snippet, or of a new one (specification §4.3, flow 2). */
export function Editor(props: EditorProps) {
  const { env, data, target, mac } = props;
  const { api } = env;
  const ids = {
    abbreviation: useId(),
    label: useId(),
    content: useId(),
    folder: useId(),
    tags: useId(),
    abbrMessages: useId(),
    abbrHelp: useId(),
    contentMessages: useId(),
    optionsPanel: useId(),
  };
  const isDraft = target.kind === "draft";
  const seed = target.kind === "draft" ? target.seed : null;
  const rowId = useRef<string | null>(target.kind === "snippet" ? target.id : null);
  const [base, setBase] = useState<Form | null>(seed ? fromSeed(seed) : null);
  const [form, setForm] = useState<Form | null>(base);
  const [meta, setMeta] = useState<{ updatedAt: number } | null>(null);
  const [saved, setSaved] = useState<Snippet | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [external, setExternal] = useState<Snippet | null>(null);
  const [check, setCheck] = useState<AbbreviationCheck | null>(null);
  const [touched, setTouched] = useState(false);
  const [saving, setSaving] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [askPlain, setAskPlain] = useState(false);
  /** The answers of "Try the form", the preview's test values (spec §4.3). */
  const [previewValues, setPreviewValues] = useState<Record<string, string> | undefined>(undefined);
  const abbrInput = useRef<HTMLInputElement>(null);
  const root = useRef<HTMLFormElement>(null);

  const dirty = base !== null && form !== null && !sameForm(base, form);
  const latest = useRef({ form, base, dirty, saving, check, saved });
  latest.current = { form, base, dirty, saving, check, saved };

  // ---- Loading the saved snippet -------------------------------------------------------------
  const id = target.kind === "snippet" ? target.id : null;
  const usageRow = id ? data.rowsById.get(id) : undefined;
  const usage = usageRow ? usageText(usageRow.uses, usageRow.lastUsed) : null;
  useEffect(() => {
    if (id === null) return;
    // A draft that was just saved is already what the row holds.
    if (rowId.current === id && latest.current.base !== null) return;
    rowId.current = id;
    let live = true;
    setBase(null);
    setForm(null);
    api.getSnippet(id).then(
      (s) => {
        if (!live) return;
        setSaved(s);
        setBase(fromSnippet(s));
        setForm(fromSnippet(s));
        setMeta({ updatedAt: s.updatedAt });
      },
      (e: unknown) => {
        if (live) setNotice(goneNotice(describeFailure(e, "get_snippet"), props.onGone));
      },
    );
    return () => {
      live = false;
    };
  }, [id, api]);

  // ---- Changes made elsewhere ---------------------------------------------------------------
  const seen = useRef(data.change.n);
  useEffect(() => {
    if (data.change.n === seen.current) return;
    seen.current = data.change.n;
    const row = rowId.current;
    if (row === null || latest.current.base === null) return;
    if (data.change.snippets !== null && !data.change.snippets.includes(row)) return;
    let live = true;
    api.getSnippet(row).then(
      (s) => {
        if (!live) return;
        if (meta !== null && s.updatedAt === meta.updatedAt) return;
        if (latest.current.dirty) {
          setExternal(s);
        } else {
          setSaved(s);
          setBase(fromSnippet(s));
          setForm(fromSnippet(s));
          setMeta({ updatedAt: s.updatedAt });
        }
      },
      (e: unknown) => {
        if (live) setNotice(goneNotice(describeFailure(e, "get_snippet"), props.onGone));
      },
    );
    return () => {
      live = false;
    };
  }, [data.change.n]);

  // A new draft starts in the Abbreviation field, after the folder's prefix.
  useEffect(() => {
    if (!isDraft) return;
    const input = abbrInput.current;
    if (!input) return;
    input.focus();
    const end = input.value.length;
    input.setSelectionRange(end, end);
  }, [isDraft, seed?.key]);

  // ---- The service's view of the abbreviation -----------------------------------------------
  const abbreviation = form?.abbreviation ?? "";
  const trigger = form?.trigger ?? "delimiter";
  useEffect(() => {
    if (form === null) return;
    if (abbreviation === "" || /\s/u.test(abbreviation)) {
      setCheck(null);
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      api.checkAbbreviation(abbreviation, trigger, rowId.current ?? undefined).then(
        (c) => {
          if (live) setCheck(c);
        },
        () => {
          // The service cannot say right now; saving still asks it.
          if (live) setCheck(null);
        },
      );
    }, 120);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [abbreviation, trigger, form === null, api, data.change.n]);

  const folderName = useCallback(
    (fid: string) => data.folders.find((f) => f.id === fid)?.name ?? "this folder",
    [data.folders],
  );
  const messages: Message[] = useMemo(
    () =>
      abbreviationMessages(
        { abbreviation, trigger, rowOf: (i) => data.rowsById.get(i), folderName },
        check,
      ),
    [abbreviation, trigger, check, data.rowsById, folderName],
  );
  const showMessages = !isDraft || dirty || touched;
  const shown = showMessages ? messages : [];
  const blocking = messages.some((m) => m.blocks) || abbreviation === "";
  const online = env.connection === "connected";
  const canSave = form !== null && dirty && !blocking && !saving && online;

  // ---- Saving -------------------------------------------------------------------------------
  const save = useCallback(async (): Promise<boolean> => {
    const { form: f, dirty: d, saving: s, base: b } = latest.current;
    if (f === null || b === null || s) return false;
    if (!d) return true;
    setTouched(true);
    if (messages.some((m) => m.blocks) || f.abbreviation === "") return false;
    setSaving(true);
    setNotice(null);
    setLastError(null);
    try {
      const row = rowId.current;
      const result = await api.saveSnippet({
        ...(row !== null ? { id: row } : {}),
        folderId: f.folderId,
        abbreviation: f.abbreviation,
        label: f.label,
        kind: f.kind,
        body: f.body,
        trigger: f.trigger,
        caseMode: f.caseMode,
        wordStart: f.wordStart,
        enabled: f.enabled,
        tags: f.tags,
      });
      const wasNew = row === null;
      rowId.current = result.id;
      setBase(f);
      setMeta({ updatedAt: result.updatedAt });
      setExternal(null);
      props.onSaved(result, { abbreviation: f.abbreviation, trigger: f.trigger }, wasNew);
      return true;
    } catch (e) {
      const failure = describeFailure(e, "put_snippet");
      setLastError(failure.text);
      if (failure.kind === "conflict") {
        // The abbreviation message explains it; ask again so it names the snippet.
        void api
          .checkAbbreviation(f.abbreviation, f.trigger, rowId.current ?? undefined)
          .then(setCheck, () => {});
      } else if (failure.kind === "gone") {
        setNotice(goneNotice(failure, props.onGone));
      } else {
        setNotice({
          tone: "danger",
          text: failure.text,
          actions:
            failure.kind === "internal"
              ? [{ label: "Copy details", run: () => void copy(env, failure.detail) }]
              : [],
        });
      }
      return false;
    } finally {
      setSaving(false);
    }
  }, [api, messages, env]);

  const title = (): string => {
    const text = (latest.current.base?.label || latest.current.form?.label || "").trim();
    return text || (isDraft && rowId.current === null ? "New snippet" : latest.current.base?.abbreviation || "this snippet");
  };

  useImperativeHandle(
    props.ref,
    () => ({
      isDirty: () => latest.current.dirty,
      save,
      label: title,
      lastError: () => lastError,
      focus: () => (abbrInput.current ?? root.current)?.focus(),
      toggleOptions: () => setOptionsOpen((o) => !o),
    }),
    [save, lastError],
  );

  if (form === null || base === null) {
    return (
      <section className="pane editor-pane" aria-label="Snippet" data-pane="editor">
        {notice && <Banner notice={notice} />}
      </section>
    );
  }

  const update = (patch: Partial<Form>) => setForm((f) => (f ? { ...f, ...patch } : f));
  const template = env.template ?? templateService;
  // "Try the form": the real panel over this window; its answers become the preview's test
  // values, typed nowhere (flow 2 step 5, flow 4's entry points).
  const tryForm =
    template.tryForm && form
      ? (fields: FieldView[]) => {
          void template.tryForm?.(fields, form.label, form.abbreviation).then(
            (values) => {
              if (values) setPreviewValues(values);
            },
            () => undefined,
          );
        }
      : undefined;
  const folder: Folder | undefined = data.folders.find((x) => x.id === form.folderId);
  const prefix = folder?.defaults.abbreviationPrefix ?? "";

  const setTrigger = (next: Trigger) =>
    setForm((f) => {
      if (!f) return f;
      // "Only at the start of a word" follows the trigger until the person decides otherwise.
      const follows = f.wordStart === (f.trigger === "delimiter");
      return { ...f, trigger: next, wordStart: follows ? next === "delimiter" : f.wordStart };
    });

  const setFolder = (folderId: string) =>
    setForm((f) => {
      if (!f) return f;
      const from = data.folders.find((x) => x.id === f.folderId)?.defaults.abbreviationPrefix ?? "";
      const to = data.folders.find((x) => x.id === folderId)?.defaults.abbreviationPrefix ?? "";
      // A draft that only holds the old folder's prefix takes the new one.
      const abbreviation = isDraft && rowId.current === null && f.abbreviation === from ? to : f.abbreviation;
      return { ...f, folderId, abbreviation };
    });

  /** "Plain text / Formatted": going back to plain text asks first when formatting would be
   * lost (specification §4.3). */
  const chooseKind = (next: "plain" | "rich") => {
    if (next === form.kind) return;
    if (next === "plain" && hasFormatting(form.body)) {
      setAskPlain(true);
      return;
    }
    update({ kind: next });
  };

  const toggleEnabled = (next: boolean) => {
    update({ enabled: next });
    if (rowId.current === null) return;
    const row = rowId.current;
    // A saved snippet's On switch takes effect at once (design system §4.3).
    api.setEnabled(row, next).then(
      (result) => {
        setBase((b) => (b ? { ...b, enabled: next } : b));
        setMeta({ updatedAt: result.updatedAt });
      },
      (e: unknown) => {
        update({ enabled: !next });
        const failure = describeFailure(e, "put_snippet");
        setNotice({ tone: "danger", text: failure.text, actions: [] });
      },
    );
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (hasMod(e, mac) && (e.key === "s" || e.key === "S") && !e.shiftKey) {
      e.preventDefault();
      void save();
    } else if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      props.onClose();
    }
  };

  const openMore = (e: React.MouseEvent<HTMLButtonElement>) => {
    const s = latest.current.saved;
    const at = below(e.currentTarget);
    setMenu({
      ...at,
      label: "More actions",
      items: [
        {
          label: "Duplicate",
          shortcut: shortcutLabel({ key: "D" }, mac),
          disabled: s === null || dirty,
          onSelect: () => s && props.onDuplicate(s),
        },
        { label: "Copy content", onSelect: () => void copy(env, form.body) },
        {
          label: "Delete",
          danger: true,
          disabled: s === null,
          onSelect: () => s && props.onDelete({ ...s, ...toSnippetFields(base) }),
        },
      ],
    });
  };

  const showCancel = isDraft && rowId.current === null ? true : dirty;
  const prefixHelp =
    prefix !== "" && form.abbreviation.startsWith(prefix)
      ? `Starts with ${prefix}, the prefix of the folder ${folder?.name ?? ""}.`
      : null;
  const sampleBase = form.abbreviation === "" ? ";sig" : form.abbreviation;

  return (
    <section className="pane editor-pane" aria-label="Snippet" data-pane="editor">
      {notice && <Banner notice={notice} />}
      {external && (
        <Banner
          notice={{
            tone: "warning",
            text: "This snippet was changed on another device.",
            actions: [
              {
                label: "Show the new version",
                run: () => {
                  setSaved(external);
                  setBase(fromSnippet(external));
                  setForm(fromSnippet(external));
                  setMeta({ updatedAt: external.updatedAt });
                  setExternal(null);
                },
              },
              {
                label: "Keep editing mine",
                run: () => {
                  setMeta({ updatedAt: external.updatedAt });
                  setExternal(null);
                },
              },
            ],
          }}
        />
      )}
      <form
        ref={root}
        className="editor"
        aria-label={title()}
        onKeyDown={onKeyDown}
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
        noValidate
      >
        <header className="editor-head">
          <h2 className="editor-title">{title()}</h2>
          <div className="seg" role="group" aria-label="Kind">
            {(
              [
                ["plain", "Plain text"],
                ["rich", "Formatted"],
              ] as const
            ).map(([k, label]) => (
              <button
                key={k}
                type="button"
                aria-pressed={form.kind === k}
                disabled={!online && form.kind !== k}
                onClick={() => chooseKind(k)}
              >
                {label}
              </button>
            ))}
          </div>
          {dirty && (
            <span className="not-saved" role="status">
              Not saved
            </span>
          )}
          <span className="spacer" />
          <button
            type="button"
            role="switch"
            aria-checked={form.enabled}
            className={form.enabled ? "switch on" : "switch"}
            onClick={() => toggleEnabled(!form.enabled)}
          >
            <span className="switch-track" aria-hidden="true">
              <span className="switch-thumb" />
            </span>
            <span>On</span>
          </button>
          {!(isDraft && rowId.current === null) && (
            <button
              type="button"
              className="icon-button"
              aria-label="More actions"
              aria-haspopup="menu"
              onClick={openMore}
            >
              <Icon name="more" />
            </button>
          )}
          {showCancel && (
            <button type="button" className="button" onClick={props.onClose}>
              Cancel
            </button>
          )}
          <button
            type="submit"
            className="button primary"
            disabled={!canSave}
            aria-keyshortcuts={ariaShortcut({ key: "S" }, mac)}
          >
            Save
            <kbd className="kbd">{shortcutLabel({ key: "S" }, mac)}</kbd>
          </button>
        </header>

        <div className="editor-body">
          <div className="row-fields">
            <div className="field grow">
              <label htmlFor={ids.abbreviation}>Abbreviation</label>
              <input
                ref={abbrInput}
                id={ids.abbreviation}
                className={`input mono${shown.some((m) => m.tone === "error") ? " invalid" : ""}`}
                value={form.abbreviation}
                aria-invalid={shown.some((m) => m.tone === "error") ? true : undefined}
                aria-describedby={`${ids.abbrMessages}${prefixHelp ? ` ${ids.abbrHelp}` : ""}`}
                autoComplete="off"
                autoCapitalize="off"
                spellCheck={false}
                onChange={(e) => update({ abbreviation: e.target.value })}
                onBlur={() => setTouched(true)}
              />
              {prefixHelp && (
                <p id={ids.abbrHelp} className="help">
                  {prefixHelp}
                </p>
              )}
              <div id={ids.abbrMessages} className="messages">
                {shown.map((m, i) => (
                  <MessageLine key={i} message={m} onAction={(a) => props.onShowSnippet(a.snippet)} />
                ))}
              </div>
            </div>
            <div className="field grow">
              <label htmlFor={ids.label}>
                Label <span className="optional">optional</span>
              </label>
              <input
                id={ids.label}
                className="input"
                value={form.label}
                placeholder="Shown in lists and search"
                onChange={(e) => update({ label: e.target.value })}
              />
            </div>
          </div>

          <div className="field">
            <label htmlFor={ids.content}>Content</label>
            <TemplateField
              id={ids.content}
              value={form.body}
              placeholder={CONTENT_PLACEHOLDER}
              describedBy={ids.contentMessages}
              kind={form.kind}
              imageFolder={form.kind === "rich" ? form.folderId : undefined}
              snippets={data.rows}
              service={env.template}
              previewValues={previewValues}
              onTryForm={tryForm}
              onChange={(body) => update({ body })}
            />
            {/* The template's error is shown once, by the field (spec section 6.1). */}
            <div id={ids.contentMessages} className="messages" />
          </div>

          <div className="row-fields">
            <div className="field grow">
              <label htmlFor={ids.folder}>Folder</label>
              <select
                id={ids.folder}
                className="input"
                value={form.folderId}
                onChange={(e) => setFolder(e.target.value)}
              >
                {folderOptions(data.folders).map((o) => (
                  <option key={o.id} value={o.id}>
                    {" ".repeat(o.depth - 1)}
                    {o.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="field grow">
              <label htmlFor={ids.tags}>
                Tags <span className="optional">optional</span>
              </label>
              <TagInput
                id={ids.tags}
                value={form.tags}
                tags={data.tags}
                onChange={(tags) => update({ tags })}
                onCreate={props.onCreateTag}
              />
            </div>
          </div>
        </div>

        <footer className="editor-foot">
          <p className="foot-text">
            {optionsSummary(form.trigger, form.caseMode)} ·{" "}
            <button
              type="button"
              className="link"
              aria-expanded={optionsOpen}
              aria-controls={ids.optionsPanel}
              aria-keyshortcuts={ariaShortcut({ shift: true, key: "O" }, mac)}
              onClick={() => setOptionsOpen((o) => !o)}
            >
              Options
            </button>
            {usage && <span className="usage-fact"> · {usage}</span>}
          </p>
        </footer>

        {optionsOpen && (
          <div id={ids.optionsPanel} className="options" role="group" aria-label="Options">
            <Radios
              label="Expand"
              value={form.trigger}
              onChange={setTrigger}
              options={[
                {
                  value: "delimiter",
                  label: "After a space or punctuation",
                  help: "When the word is finished. Enter and Tab do not count.",
                },
                {
                  value: "immediate",
                  label: "Immediately",
                  help: "On the last character. Good for abbreviations that are never part of a word.",
                },
              ]}
            />
            <Radios
              label="Capital letters"
              value={form.caseMode}
              onChange={(caseMode) => update({ caseMode })}
              options={[
                { value: "exact", label: "Exactly as written", help: `Only ${sampleBase}` },
                { value: "insensitive", label: "Ignore", help: `${swapFirstLetter(sampleBase)} works too` },
                {
                  value: "adaptive",
                  label: "Ignore, and follow mine",
                  help: `${swapFirstLetter(sampleBase)} starts with a capital, ${sampleBase.toUpperCase()} is all capitals`,
                },
              ]}
            />
            <label className="check">
              <input
                type="checkbox"
                checked={form.wordStart}
                onChange={(e) => update({ wordStart: e.target.checked })}
              />
              <span>
                Only at the start of a word
                <span className="help block">Does not expand in the middle of other text.</span>
              </span>
            </label>
          </div>
        )}
      </form>
      {menu && (
        <Menu items={menu.items} x={menu.x} y={menu.y} label={menu.label} onClose={() => setMenu(null)} />
      )}
      {askPlain && (
        <ConfirmDialog
          title={TO_PLAIN_TITLE}
          body={TO_PLAIN_BODY}
          confirm={TO_PLAIN_CONFIRM}
          mac={mac}
          onConfirm={() => {
            setAskPlain(false);
            update({ kind: "plain" });
          }}
          onCancel={() => setAskPlain(false)}
        />
      )}
    </section>
  );
}

function toSnippetFields(f: Form): Partial<Snippet> {
  return {
    abbreviation: f.abbreviation,
    label: f.label,
    body: f.body,
    folderId: f.folderId,
    tags: f.tags,
    trigger: f.trigger,
    caseMode: f.caseMode,
    wordStart: f.wordStart,
    enabled: f.enabled,
    kind: f.kind,
  };
}

async function copy(env: LibraryEnv, text: string): Promise<void> {
  try {
    if (env.copyText) await env.copyText(text);
    else await navigator.clipboard.writeText(text);
  } catch {
    // Nothing to tell: the clipboard was refused.
  }
}

function goneNotice(failure: Failure, close: () => void): Notice {
  return { tone: "danger", text: failure.text, actions: [{ label: "Close", run: close }] };
}

function Banner({ notice }: { notice: Notice }): ReactNode {
  return (
    <div className={`banner ${notice.tone}`} role={notice.tone === "danger" ? "alert" : "status"}>
      <Icon name="warn" />
      <p>{notice.text}</p>
      {notice.actions.map((a) => (
        <button key={a.label} type="button" className="button" onClick={a.run}>
          {a.label}
        </button>
      ))}
    </div>
  );
}

function MessageLine({
  message,
  onAction,
}: {
  message: Message;
  onAction: (action: NonNullable<Message["action"]>) => void;
}) {
  const error = message.tone === "error";
  return (
    <p className={`field-message ${error ? "error" : "warning"}`} role={error ? "alert" : "status"} aria-label={messageText(message)}>
      <Icon name={error ? "warn" : "info"} />
      <span>
        {message.parts.map((p, i) =>
          p.strong ? (
            <strong key={i} className="mono">
              {p.text}
            </strong>
          ) : (
            <span key={i}>{p.text}</span>
          ),
        )}
        {message.action && (
          <>
            {" "}
            <button
              type="button"
              className="link"
              onClick={() => message.action && onAction(message.action)}
            >
              {message.action.label}
            </button>
          </>
        )}
      </span>
    </p>
  );
}

function Radios<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string; help: string }[];
  onChange: (value: T) => void;
}) {
  const name = useId();
  return (
    <div className="radios" role="radiogroup" aria-label={label}>
      <span className="label">{label}</span>
      {options.map((o) => (
        <label key={o.value} className={value === o.value ? "radio checked" : "radio"}>
          <input
            type="radio"
            name={name}
            checked={value === o.value}
            onChange={() => onChange(o.value)}
          />
          <span>
            {o.label}
            <span className="help block">{o.help}</span>
          </span>
        </label>
      ))}
    </div>
  );
}
