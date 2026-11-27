import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";

import type { Folder, Saved, Snippet, SnippetDraft, Tag, TagColor, Trigger } from "./api";
import { ConfirmDialog, FolderDialog, type FolderForm, LeaveDialog, TagDialog } from "./Dialogs";
import { type DraftSeed, Editor, type EditorHandle } from "./Editor";
import { EmptyState } from "./EmptyState";
import type { LibraryEnv } from "./env";
import { describeFailure } from "./errors";
import { Icon } from "./Icon";
import { ListPane, NO_SELECTION, type Selection, single } from "./ListPane";
import { MenuHost, type OpenMenu } from "./Menu";
import {
  type SortMode,
  filterIndexed,
  hasMod,
  isMac,
  isTemplateError,
  nextTagColor,
  rowTitle,
  shortcutLabel,
  subtreeIds,
} from "./model";
import { Diagnostics } from "./diagnostics";
import { HealthView, affectedCount, useHealth } from "./health";
import { SettingsView } from "./settings";
import { Sidebar, type View, sameView } from "./Sidebar";
import { Toast, type ToastState } from "./Toasts";
import { useLibraryData } from "./useLibraryData";
import { usePaletteRequests } from "./usePaletteRequests";
import "./library.css";

// ---- What the window remembers (a convenience: it renders without it) -----------------------

const STORE_KEY = "iamb.library";

/** How long "Start it again" waits for the service before it can be pressed again. */
export const RESTART_WAIT_MS = 15_000;

interface Remembered {
  sort: SortMode;
  view: View | null;
  snippet: string | null;
}

/**
 * A one-shot request from another window (onboarding, task 3.7) to open at a view: written just
 * before the window is shown, honoured when it is at most this old, and removed once the window
 * has used it.
 */
const OPEN_KEY = "iamb.library.open";
const OPEN_FRESH_MS = 10_000;

function requestedView(): View | null {
  try {
    const raw = window.localStorage.getItem(OPEN_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as { view?: unknown; at?: unknown };
    if (v.view === "diagnostics" && typeof v.at === "number" && Date.now() - v.at < OPEN_FRESH_MS) {
      return { kind: "diagnostics" };
    }
  } catch {
    // No storage: the window opens where it was.
  }
  return null;
}

function recall(): Remembered {
  try {
    const raw = window.localStorage.getItem(STORE_KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<Remembered>;
      return {
        sort: v.sort === "changed" || v.sort === "used" ? v.sort : "label",
        view: v.view && (v.view.kind === "folder" || v.view.kind === "tag") ? v.view : null,
        snippet: typeof v.snippet === "string" ? v.snippet : null,
      };
    }
  } catch {
    // Private window or blocked storage: start from the defaults.
  }
  return { sort: "label", view: null, snippet: null };
}

function remember(value: Remembered): void {
  try {
    window.localStorage.setItem(STORE_KEY, JSON.stringify(value));
  } catch {
    // Nothing to do: it is only a convenience.
  }
}

// ---- Helpers --------------------------------------------------------------------------------

let counter = 0;
const nextKey = (prefix: string) => `${prefix}-${++counter}`;

function draftOf(s: Snippet): SnippetDraft {
  return {
    id: s.id,
    folderId: s.folderId,
    abbreviation: s.abbreviation,
    label: s.label,
    kind: s.kind,
    body: s.body,
    trigger: s.trigger,
    caseMode: s.caseMode,
    wordStart: s.wordStart,
    enabled: s.enabled,
    tags: s.tags,
  };
}

function savedParts(abbreviation: string, trigger: Trigger): ToastState["parts"] {
  return trigger === "immediate"
    ? [{ text: "Saved. Type " }, { text: abbreviation, strong: true }, { text: " to use it." }]
    : [
        { text: "Saved. Type " },
        { text: abbreviation, strong: true },
        { text: " and a space to use it." },
      ];
}

type Leave = { action: () => void } | null;

interface FolderDialogState {
  folder: Folder | null;
  parent: string | null;
}

interface TagDialogState {
  tag: Tag | null;
}

/** The library window: sidebar, snippet list and editor (specification §4.2, §4.3). */
export function Library({ env }: { env: LibraryEnv }) {
  const { api } = env;
  const mac = env.mac ?? isMac();
  const initial = useRef(recall());
  const requested = useRef(requestedView());
  useEffect(() => {
    try {
      window.localStorage.removeItem(OPEN_KEY);
    } catch {
      // Nothing to remove.
    }
  }, []);
  const [sort, setSort] = useState<SortMode>(initial.current.sort);
  // "Start it again" in the "Iamb is not running" banner: idle, waiting for the service, or refused.
  const [restart, setRestart] = useState<"idle" | "starting" | "failed">("idle");
  const [view, setView] = useState<View>(requested.current ?? initial.current.view ?? { kind: "all" });
  const [filter, setFilter] = useState("");
  const [selection, setSelection] = useState<Selection>(
    initial.current.snippet ? single(initial.current.snippet) : NO_SELECTION,
  );
  const [draft, setDraft] = useState<DraftSeed | null>(null);
  const [editorKey, setEditorKey] = useState("none");
  const [leave, setLeave] = useState<Leave>(null);
  const [leaveBusy, setLeaveBusy] = useState(false);
  const [leaveError, setLeaveError] = useState<string | null>(null);
  const [folderDialog, setFolderDialog] = useState<FolderDialogState | null>(null);
  const [tagDialog, setTagDialog] = useState<TagDialogState | null>(null);
  const [confirm, setConfirm] = useState<
    { title: string; body: string; confirm: string; run: () => Promise<void> } | null
  >(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [dialogBusy, setDialogBusy] = useState(false);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [toast, setToast] = useState<ToastState | null>(null);
  const editor = useRef<EditorHandle>(null);
  const filterInput = useRef<HTMLInputElement>(null);
  const listEl = useRef<HTMLDivElement>(null);
  const closing = useRef(false);

  const data = useLibraryData(env, sort);
  const online = env.connection === "connected";

  // ---- Toasts ---------------------------------------------------------------------------------
  const showToast = useCallback((parts: ToastState["parts"], action?: ToastState["action"]) => {
    setToast({ id: ++counter, parts, ...(action ? { action } : {}) });
  }, []);
  const dismissToast = useCallback((id: number) => {
    setToast((t) => (t && t.id === id ? null : t));
  }, []);

  // ---- Remembering --------------------------------------------------------------------------
  useEffect(() => {
    remember({
      sort,
      view: view.kind === "folder" || view.kind === "tag" ? view : null,
      snippet: selection.ids.size === 1 ? selection.active : null,
    });
  }, [sort, view, selection]);

  // A remembered folder, tag or snippet that is gone does not leave an empty window.
  useEffect(() => {
    if (!data.ready) return;
    if (view.kind === "folder" && !data.folders.some((f) => f.id === view.id)) setView({ kind: "all" });
    if (view.kind === "tag" && !data.tags.some((t) => t.id === view.id)) setView({ kind: "all" });
  }, [data.ready, data.folders, data.tags, view]);
  useEffect(() => {
    if (!data.ready || draft !== null || selection.ids.size === 0) return;
    // The snippet open in the editor stays selected when it goes: the editor says it no longer
    // exists and its Close button clears the selection (specification §6.3).
    const open = selection.ids.size === 1 ? selection.active : null;
    const stays = (id: string) => id === open || data.rowsById.has(id);
    const gone = [...selection.ids].filter((id) => !stays(id));
    if (gone.length === 0) return;
    const keep = [...selection.ids].filter(stays);
    setSelection(keep.length > 0 ? { ids: new Set(keep), active: keep[0] ?? null, anchor: keep[0] ?? null } : NO_SELECTION);
  }, [data.ready, data.rowsById, draft, selection]);

  // ---- The list in view -----------------------------------------------------------------------
  const scoped = useMemo(() => {
    if (view.kind === "folder") return data.items.filter((i) => i.row.folderId === view.id);
    if (view.kind === "tag") return data.items.filter((i) => i.row.tags.includes(view.id));
    return data.items;
  }, [data.items, view]);
  const deferred = useDeferredValue(filter);
  const visible = useMemo(() => filterIndexed(scoped, deferred), [scoped, deferred]);

  const folder = view.kind === "folder" ? data.folders.find((f) => f.id === view.id) : undefined;
  const tag = view.kind === "tag" ? data.tags.find((t) => t.id === view.id) : undefined;
  const scopeName = folder?.name ?? tag?.name ?? "All snippets";
  const filterLabel = folder || tag ? `Filter ${scopeName}` : "Filter snippets";
  const listLabel = folder || tag ? `Snippets in ${scopeName}` : "All snippets";
  const healthOpen = view.kind === "health";
  const report = useHealth(env, data.change.n, healthOpen);
  // The badge counts what library health lists; until it has answered, the broken rows.
  const broken = useMemo(
    () =>
      report.health
        ? affectedCount(report.health)
        : data.rows.filter((r) => isTemplateError(r.broken)).length,
    [report.health, data.rows],
  );

  useEffect(() => {
    document.title =
      view.kind === "settings"
        ? "Iamb – Settings"
        : view.kind === "health"
          ? "Iamb – Library health"
          : view.kind === "diagnostics"
            ? "Iamb – Why didn’t it expand?"
            : `Iamb – ${scopeName}`;
  }, [view, scopeName]);

  // ---- Leaving an editor with unsaved changes ---------------------------------------------
  const guard = useCallback((action: () => void) => {
    if (editor.current?.isDirty()) setLeave({ action });
    else action();
  }, []);

  useEffect(() => {
    if (!env.guardClose) return;
    return env.guardClose(
      () => !closing.current && (editor.current?.isDirty() ?? false),
      () =>
        setLeave({
          action: () => {
            closing.current = true;
            void api.dismiss();
          },
        }),
    );
  }, [env, api]);

  // "Quit Iamb" from the tray asks about unsaved edits as the close does (the Phase 3 UI
  // review's finding 17).
  useEffect(() => {
    if (!env.onQuitRequested) return;
    return env.onQuitRequested(() =>
      guard(() => {
        closing.current = true;
        void env.quit?.().catch(() => {
          closing.current = false;
        });
      }),
    );
  }, [env, guard]);

  // ---- Choosing what the editor shows -----------------------------------------------------
  const changeSelection = useCallback(
    (next: Selection) => {
      if (draft === null && next.ids.size === selection.ids.size && [...next.ids].every((id) => selection.ids.has(id))) {
        setSelection(next);
        return;
      }
      guard(() => {
        setDraft(null);
        setSelection(next);
        if (next.ids.size === 1 && next.active) setEditorKey(`snippet-${next.active}`);
      });
    },
    [draft, guard, selection.ids],
  );

  const changeView = useCallback(
    (next: View) => {
      if (sameView(next, view)) return;
      guard(() => {
        setView(next);
        setFilter("");
        setDraft(null);
        setSelection(NO_SELECTION);
        setEditorKey("none");
      });
    },
    [guard, view],
  );

  const focusList = () => requestAnimationFrame(() => listEl.current?.focus());

  // Focus is never lost (spec §5.6): the editor goes when Settings, Library health or the log
  // open from a shortcut, and the keyboard would be left on nothing. It goes to the view's title
  // (or back to the list when the list returns); a view opened from the sidebar keeps the focus
  // on the button that opened it.
  const viewTitle = useRef<HTMLHeadingElement>(null);
  const firstView = useRef(true);
  useEffect(() => {
    if (firstView.current) {
      firstView.current = false;
      return;
    }
    const frame = requestAnimationFrame(() => {
      const now = document.activeElement;
      if (now && now !== document.body) return;
      (viewTitle.current ?? listEl.current)?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [view.kind]);

  const openDraft = useCallback(
    (seed: DraftSeed) => {
      guard(() => {
        // The editor lives in the list's views: from Settings and the like, go back to them.
        setView((v) => (v.kind === "settings" || v.kind === "diagnostics" || v.kind === "health" ? { kind: "all" } : v));
        setSelection(NO_SELECTION);
        setDraft(seed);
        setEditorKey(`draft-${seed.key}`);
      });
    },
    [guard],
  );

  const newSnippet = useCallback(() => {
    const target =
      folder ??
      data.folders.find((f) => f.parentId === null) ??
      data.folders[0];
    if (!target) return;
    openDraft({
      key: nextKey("d"),
      folderId: target.id,
      abbreviation: target.defaults.abbreviationPrefix ?? "",
      label: "",
      body: "",
      trigger: target.defaults.trigger ?? "delimiter",
      caseMode: target.defaults.caseMode ?? "insensitive",
      tags: tag ? [tag.id] : [],
    });
  }, [data.folders, folder, openDraft, tag]);

  /** "New snippet labelled “…”" from the search palette: a draft with that label. */
  const newLabelled = useCallback(
    (label: string) => {
      const target =
        folder ??
        data.folders.find((f) => f.parentId === null) ??
        data.folders[0];
      if (!target) return;
      openDraft({
        key: nextKey("d"),
        folderId: target.id,
        abbreviation: target.defaults.abbreviationPrefix ?? "",
        label,
        body: "",
        trigger: target.defaults.trigger ?? "delimiter",
        caseMode: target.defaults.caseMode ?? "insensitive",
        tags: tag ? [tag.id] : [],
      });
    },
    [data.folders, folder, openDraft, tag],
  );

  const closeEditor = useCallback(() => {
    guard(() => {
      setDraft(null);
      setEditorKey("none");
      focusList();
    });
  }, [guard]);

  // The open snippet no longer exists (the editor learnt it from the service): it was kept
  // selected so that the editor could say so, and goes now. Nothing is left to save.
  const dropGone = useCallback(() => {
    setDraft(null);
    setEditorKey("none");
    setSelection((s) => (s.ids.size === 1 ? NO_SELECTION : s));
    focusList();
  }, []);

  const duplicate = useCallback(
    (s: Snippet) => {
      openDraft({
        key: nextKey("d"),
        folderId: s.folderId,
        abbreviation: `${s.abbreviation}2`,
        label: s.label,
        body: s.body,
        trigger: s.trigger,
        caseMode: s.caseMode,
        tags: s.tags,
      });
    },
    [openDraft],
  );

  const duplicateRow = useCallback(
    (id: string) => {
      void api.getSnippet(id).then(duplicate, () => {});
    },
    [api, duplicate],
  );

  const showSnippet = useCallback(
    (id: string) => {
      const row = data.rowsById.get(id);
      guard(() => {
        setDraft(null);
        if (row && view.kind === "folder" && row.folderId !== view.id) setView({ kind: "all" });
        setFilter("");
        setSelection(single(id));
        setEditorKey(`snippet-${id}`);
      });
    },
    [data.rowsById, guard, view],
  );

  usePaletteRequests(env.palette, data.ready, {
    onNew: newLabelled,
    onShow: showSnippet,
    onView: (next) => changeView(next === "settings" ? { kind: "settings" } : { kind: "diagnostics" }),
  });

  // ---- Saving, deleting, turning on and off ----------------------------------------------------
  const onSaved = useCallback(
    (saved: Saved, form: { abbreviation: string; trigger: Trigger }, wasNew: boolean) => {
      if (wasNew) {
        setDraft(null);
        setSelection(single(saved.id));
      }
      void data.reload({ rows: true, tags: true }).then((rows) => {
        const row = rows.find((r) => r.id === saved.id);
        if (row && isTemplateError(row.broken)) {
          showToast([{ text: "Saved, but it will not expand until the error is fixed." }]);
        } else {
          showToast(savedParts(form.abbreviation, form.trigger));
        }
      });
    },
    [data, showToast],
  );

  const neighbourOf = useCallback(
    (ids: ReadonlySet<string>): string | null => {
      const at = visible.findIndex((i) => ids.has(i.row.id));
      if (at < 0) return null;
      const after = visible.slice(at).find((i) => !ids.has(i.row.id));
      if (after) return after.row.id;
      for (let k = at - 1; k >= 0; k--) {
        const item = visible[k];
        if (item && !ids.has(item.row.id)) return item.row.id;
      }
      return null;
    },
    [visible],
  );

  const deleteSnippets = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return;
      const set = new Set(ids);
      const next = neighbourOf(set);
      // What Undo needs: the whole row, with its tags. A row that cannot be read is not deleted
      // (there would be nothing to undo with) and counts as failed.
      const read = await Promise.allSettled(ids.map((id) => api.getSnippet(id)));
      const toDelete = read.flatMap((r) => (r.status === "fulfilled" ? [r.value] : []));
      // Each delete on its own: one that fails must not hide the others, which are done
      // (the Phase 3 UI review's finding 6).
      const results = await Promise.allSettled(toDelete.map((s) => api.deleteSnippet(s.id)));
      const full = toDelete.filter((_, i) => results[i]?.status === "fulfilled");
      const failures: Array<{ id: string; reason: unknown }> = [];
      read.forEach((r, i) => {
        if (r.status === "rejected") failures.push({ id: ids[i] ?? "", reason: r.reason });
      });
      results.forEach((r, i) => {
        const s = toDelete[i];
        if (r.status === "rejected" && s) failures.push({ id: s.id, reason: r.reason });
      });
      const titleOf = (id: string): string => {
        const row = data.rowsById.get(id);
        return row ? rowTitle(row) : "";
      };
      const failed =
        failures.length === 0
          ? ""
          : `${
              failures.length === 1 && failures[0] && titleOf(failures[0].id)
                ? `“${titleOf(failures[0].id)}” could not be deleted`
                : `${failures.length} ${failures.length === 1 ? "snippet" : "snippets"} could not be deleted`
            }: ${describeFailure(failures[0]?.reason, "delete_snippet").text}`;
      if (full.length === 0) {
        if (failures.length > 0) showToast([{ text: describeFailure(failures[0]?.reason, "delete_snippet").text }]);
        return;
      }
      const label = full.length === 1 && full[0] ? rowTitle({ label: full[0].label, firstLine: full[0].body, abbreviation: full[0].abbreviation }) : "";
      setDraft(null);
      // What could not be deleted stays selected, so that the user sees what is left.
      const left = failures.map((f) => f.id).filter((id) => data.rowsById.has(id));
      if (left.length > 0) {
        setSelection({ ids: new Set(left), active: left[0] ?? null, anchor: left[0] ?? null });
        setEditorKey(left.length === 1 ? `snippet-${left[0]}` : "none");
      } else {
        setSelection(next ? single(next) : NO_SELECTION);
        setEditorKey(next ? `snippet-${next}` : "none");
      }
      focusList();
      const deleted = full.length === 1 ? `Deleted “${label}”.` : `Deleted ${full.length} snippets.`;
      showToast(
        [{ text: failed ? `${deleted} ${failed}` : deleted }],
        {
          label: "Undo",
          run: () => {
            // The service restores a deleted snippet when its own id is written again.
            void Promise.all(full.map((s) => api.saveSnippet(draftOf(s))))
              .then(() => data.reload())
              .then(() => {
                setSelection({
                  ids: new Set(full.map((s) => s.id)),
                  active: full[0]?.id ?? null,
                  anchor: full[0]?.id ?? null,
                });
                setEditorKey(full.length === 1 && full[0] ? `snippet-${full[0].id}` : "none");
              })
              .catch((e: unknown) => showToast([{ text: describeFailure(e, "put_snippet").text }]));
          },
        },
      );
    },
    [api, data, neighbourOf, showToast],
  );

  const deleteFromEditor = useCallback(
    (s: Snippet) => {
      void deleteSnippets([s.id]);
    },
    [deleteSnippets],
  );

  const toggleEnabled = useCallback(
    async (ids: string[]) => {
      const rows = ids.map((id) => data.rowsById.get(id)).filter((r) => r !== undefined);
      const turnOff = rows.some((r) => r.enabled);
      try {
        await Promise.all(rows.map((r) => api.setEnabled(r.id, !turnOff)));
        void data.reload({ rows: true });
      } catch (e) {
        showToast([{ text: describeFailure(e, "put_snippet").text }]);
      }
    },
    [api, data, showToast],
  );

  // ---- Folders ----------------------------------------------------------------------------------
  const saveFolder = async (form: FolderForm) => {
    setDialogBusy(true);
    setDialogError(null);
    const existing = folderDialog?.folder ?? null;
    const siblings = data.folders.filter((f) => f.parentId === form.parentId);
    try {
      const saved = await api.saveFolder({
        ...(existing ? { id: existing.id } : {}),
        parentId: form.parentId,
        name: form.name,
        position: existing && existing.parentId === form.parentId ? existing.position : Math.max(-1, ...siblings.map((f) => f.position)) + 1,
        defaults: form.defaults,
      });
      await data.reload({ folders: true });
      setFolderDialog(null);
      if (!existing) {
        setView({ kind: "folder", id: saved.id });
        setSelection(NO_SELECTION);
        setEditorKey("none");
      }
    } catch (e) {
      const failure = describeFailure(e, "put_folder");
      setDialogError(
        failure.kind === "invalid" && /levels deep/.test(failure.text)
          ? "Folders can be nested at most 4 levels deep."
          : failure.text,
      );
    } finally {
      setDialogBusy(false);
    }
  };

  const askDeleteFolder = useCallback(
    (f: Folder) => {
      const below = subtreeIds(data.folders, f.id);
      const count = data.rows.filter((r) => below.has(r.folderId)).length;
      setConfirm({
        title:
          count === 0
            ? `Delete “${f.name}”?`
            : `Delete “${f.name}” and its ${count} ${count === 1 ? "snippet" : "snippets"}?`,
        body: "Other snippets that include one of them will stop working.",
        confirm: "Delete",
        run: async () => {
          await api.deleteFolder(f.id);
          await data.reload();
          if (view.kind === "folder" && below.has(view.id)) {
            setView(f.parentId ? { kind: "folder", id: f.parentId } : { kind: "all" });
            setSelection(NO_SELECTION);
            setDraft(null);
            setEditorKey("none");
          }
        },
      });
    },
    [api, data, view],
  );

  const folderMenu = useCallback(
    (f: Folder, x: number, y: number) => {
      setMenu({
        x,
        y,
        label: f.name,
        items: [
          { label: "Folder settings…", onSelect: () => setFolderDialog({ folder: f, parent: f.parentId }) },
          { label: "New folder inside…", onSelect: () => setFolderDialog({ folder: null, parent: f.id }) },
          { label: "Delete…", danger: true, onSelect: () => askDeleteFolder(f) },
        ],
      });
    },
    [askDeleteFolder],
  );

  // ---- Tags -------------------------------------------------------------------------------------
  const saveTag = async (name: string, color: TagColor) => {
    setDialogBusy(true);
    setDialogError(null);
    const existing = tagDialog?.tag ?? null;
    try {
      await api.saveTag({ ...(existing ? { id: existing.id } : {}), name, color });
      await data.reload({ tags: true });
      setTagDialog(null);
    } catch (e) {
      const failure = describeFailure(e, "put_tag");
      setDialogError(failure.kind === "conflict" ? "A tag with that name already exists." : failure.text);
    } finally {
      setDialogBusy(false);
    }
  };

  const askDeleteTag = useCallback(
    (t: Tag) => {
      setConfirm({
        title: `Delete the tag “${t.name}”?`,
        body:
          t.snippets === 0
            ? "No snippet carries it."
            : `It is removed from ${t.snippets} ${t.snippets === 1 ? "snippet" : "snippets"}. The snippets stay.`,
        confirm: "Delete",
        run: async () => {
          await api.deleteTag(t.id);
          await data.reload();
          if (view.kind === "tag" && view.id === t.id) setView({ kind: "all" });
        },
      });
    },
    [api, data, view],
  );

  const createTag = useCallback(
    async (name: string): Promise<Tag | null> => {
      try {
        const saved = await api.saveTag({ name, color: nextTagColor(data.tags) });
        const tags = await api.listTags();
        void data.reload({ tags: true });
        return tags.find((t) => t.id === saved.id) ?? null;
      } catch (e) {
        const failure = describeFailure(e, "put_tag");
        showToast([{ text: failure.kind === "conflict" ? "A tag with that name already exists." : failure.text }]);
        return null;
      }
    },
    [api, data, showToast],
  );

  const tagMenu = useCallback(
    (t: Tag, x: number, y: number) => {
      setMenu({
        x,
        y,
        label: t.name,
        items: [
          { label: "Rename or recolour…", onSelect: () => setTagDialog({ tag: t }) },
          { label: "Delete…", danger: true, onSelect: () => askDeleteTag(t) },
        ],
      });
    },
    [askDeleteTag],
  );

  // ---- Panes and shortcuts ------------------------------------------------------------------
  const focusPane = (n: 0 | 1 | 2 | 3) => {
    const name = ["sidebar", "list", "editor", "toast"][n];
    const pane = document.querySelector<HTMLElement>(`[data-pane="${name}"]`);
    if (!pane) return;
    const target =
      n === 3
        ? pane.querySelector<HTMLElement>("button")
        : n === 0
        ? (pane.querySelector<HTMLElement>("[data-nav-focus]") ?? pane.querySelector<HTMLElement>('[tabindex="0"], button'))
        : n === 1
          ? pane.querySelector<HTMLElement>('[role="listbox"]')
          : (editor.current?.focus(), null);
    target?.focus();
  };

  // F6 visits the panes, and the toast too while it offers an action (Undo), which a keyboard
  // user would otherwise have to Tab to through the whole editor within 10 seconds.
  const cyclePane = (back: boolean) => {
    const names = ["sidebar", "list", "editor", "toast"];
    const there = names.map((n) => document.querySelector(`[data-pane="${n}"]`) !== null);
    const count = there[3] ? 4 : 3;
    const at = names.findIndex((n) => document.querySelector(`[data-pane="${n}"]`)?.contains(document.activeElement));
    const next = (((at < 0 ? 0 : at) + (back ? -1 : 1)) % count + count) % count;
    focusPane(next as 0 | 1 | 2 | 3);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // A dialog or a menu has the keyboard: the window's shortcuts wait.
    if (document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"]')) return;
    const k = e.key.toLowerCase();
    if (e.key === "F6") {
      e.preventDefault();
      cyclePane(e.shiftKey);
      return;
    }
    if (!hasMod(e, mac)) return;
    if (k === "n" && !e.shiftKey) {
      e.preventDefault();
      if (online) newSnippet();
    } else if (k === "n" && e.shiftKey) {
      e.preventDefault();
      if (online) setFolderDialog({ folder: null, parent: folder?.id ?? null });
    } else if (k === "o" && e.shiftKey) {
      e.preventDefault();
      editor.current?.toggleOptions();
    } else if (k === "s" && !e.shiftKey) {
      e.preventDefault();
      void editor.current?.save();
    } else if (k === "k" && !e.shiftKey) {
      // Find in all snippets: the search palette over this window (spec §5.2).
      e.preventDefault();
      if (online) void env.palette?.open().catch(() => undefined);
    } else if (k === "f" && !e.shiftKey) {
      e.preventDefault();
      filterInput.current?.focus();
      filterInput.current?.select();
    } else if (k === "1" || k === "2" || k === "3") {
      e.preventDefault();
      focusPane((Number(k) - 1) as 0 | 1 | 2);
    } else if (k === ",") {
      e.preventDefault();
      changeView({ kind: "settings" });
    } else if (k === "w") {
      e.preventDefault();
      guard(() => {
        closing.current = true;
        void api.dismiss();
      });
    }
  };

  // The shortcuts work wherever the focus is in the window, also when it is on nothing.
  const keys = useRef(onKeyDown);
  keys.current = onKeyDown;
  useEffect(() => {
    const handler = (e: KeyboardEvent) => keys.current(e);
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, []);

  // ---- Starting the service again ----------------------------------------------------------------
  const startAgain = async () => {
    setRestart("starting");
    try {
      await env.restartService?.();
    } catch {
      setRestart("failed");
    }
  };
  // The banner goes when the service answers; a later stop offers the button afresh.
  useEffect(() => {
    if (env.connection === "connected") setRestart("idle");
  }, [env.connection]);
  // A service that does not come back within a while: the button works again.
  useEffect(() => {
    if (restart !== "starting") return;
    const t = setTimeout(() => setRestart((r) => (r === "starting" ? "idle" : r)), RESTART_WAIT_MS);
    return () => clearTimeout(t);
  }, [restart]);

  // ---- The leave dialog ------------------------------------------------------------------------
  const leaveSave = async () => {
    const action = leave?.action;
    if (!action) return;
    setLeaveBusy(true);
    setLeaveError(null);
    const ok = (await editor.current?.save()) ?? false;
    setLeaveBusy(false);
    if (ok) {
      setLeave(null);
      action();
    } else {
      setLeaveError(editor.current?.lastError() ?? "The snippet could not be saved. Fix what is marked and try again.");
    }
  };

  // ---- Rendering -----------------------------------------------------------------------------------
  const empty = (() => {
    if (!data.ready) return null;
    if (filter.trim() !== "") {
      const elsewhere =
        view.kind === "all" ? 0 : filterIndexed(data.items, filter).length - visible.length;
      return (
        <EmptyState
          title={`Nothing in ${scopeName} matches “${filter.trim()}”`}
          actions={
            <>
              <button type="button" className="button" onClick={() => setFilter("")}>
                Clear filter
              </button>
              {elsewhere > 0 && (
                <button
                  type="button"
                  className="button"
                  onClick={() => {
                    setView({ kind: "all" });
                    setSelection(NO_SELECTION);
                  }}
                >
                  Show them
                </button>
              )}
            </>
          }
        >
          {elsewhere > 0 ? `${elsewhere} ${elsewhere === 1 ? "match" : "matches"} in other folders` : null}
        </EmptyState>
      );
    }
    if (view.kind === "all") {
      return (
        <EmptyState
          title="No snippets yet"
          actions={
            <button type="button" className="button primary" disabled={!online} onClick={newSnippet}>
              New snippet
            </button>
          }
        >
          A snippet is text you save once and type with a short abbreviation from then on.
        </EmptyState>
      );
    }
    if (view.kind === "folder") {
      return (
        <EmptyState
          title="This folder is empty"
          actions={
            <button type="button" className="button primary" disabled={!online} onClick={newSnippet}>
              New snippet
            </button>
          }
        />
      );
    }
    return <EmptyState title="No snippet has this tag" />;
  })();

  const status = {
    text: !online ? (env.connection === "connecting" ? "Connecting…" : "Iamb is not running") : env.paused ? (env.pausedKept ? "Iamb is still paused" : "Iamb is paused") : "Iamb is on",
    paused: env.paused,
    running: online,
  };

  const single1 = selection.ids.size === 1 ? selection.active : null;
  const editorTarget = draft
    ? ({ kind: "draft", seed: draft } as const)
    : single1
      ? ({ kind: "snippet", id: single1 } as const)
      : null;

  const rightArea = (() => {
    if (view.kind === "settings" || view.kind === "diagnostics" || view.kind === "health") {
      return (
        <main className="pane view-pane" aria-label={view.kind === "settings" ? "Settings" : view.kind === "health" ? "Library health" : "Why didn’t it expand?"}>
          <h1 className="view-title" ref={viewTitle} tabIndex={-1}>
            {view.kind === "settings" ? "Settings" : view.kind === "health" ? "Library health" : "Why didn’t it expand?"}
          </h1>
          {view.kind === "health" ? (
            <HealthView
              health={report.health}
              error={report.error}
              rows={data.rowsById}
              total={data.rows.length}
              onRetry={report.reload}
              onOpen={(id) => {
                guard(() => {
                  setView({ kind: "all" });
                  setFilter("");
                  setDraft(null);
                  setSelection(single(id));
                  setEditorKey(`snippet-${id}`);
                });
              }}
              onTurnOff={(id) => {
                void api
                  .setEnabled(id, false)
                  .then(() => data.reload({ rows: true }))
                  .then(() => report.reload())
                  .catch((e: unknown) => showToast([{ text: describeFailure(e, "put_snippet").text }]));
              }}
              onDelete={(id) => void deleteSnippets([id]).then(() => report.reload())}
            />
          ) : view.kind === "settings" ? (
            <SettingsView env={env} section={view.section} />
          ) : (
            <Diagnostics
              env={env}
              onOpenSnippet={(id) => {
                guard(() => {
                  setView({ kind: "all" });
                  setFilter("");
                  setDraft(null);
                  setSelection(single(id));
                  setEditorKey(`snippet-${id}`);
                });
              }}
              onOpenSettings={(section) => changeView({ kind: "settings", section })}
            />
          )}
        </main>
      );
    }
    return (
      <main className="library-main">
        <h1 className="visually-hidden">{listLabel}</h1>
        <ListPane
          listLabel={listLabel}
          filterLabel={filterLabel}
          header={
            folder ? (
              <div className="folder-head">
                <span className="help">Only you</span>
                <button
                  type="button"
                  className="icon-button"
                  aria-label={`More actions for ${folder.name}`}
                  aria-haspopup="menu"
                  onClick={(e) => {
                    const r = e.currentTarget.getBoundingClientRect();
                    folderMenu(folder, r.left, r.bottom + 2);
                  }}
                >
                  <Icon name="more" />
                </button>
              </div>
            ) : null
          }
          items={visible}
          filter={filter}
          onFilter={setFilter}
          sort={sort}
          canSortByUse={data.rows.some((r) => r.uses !== null)}
          onSort={setSort}
          selection={draft ? NO_SELECTION : selection}
          onSelection={changeSelection}
          onOpen={() => editor.current?.focus()}
          onNew={newSnippet}
          onDelete={(ids) => void deleteSnippets(ids)}
          onToggle={(ids) => void toggleEnabled(ids)}
          onDuplicate={duplicateRow}
          onMenu={setMenu}
          empty={empty}
          mac={mac}
          filterRef={filterInput}
          listRef={listEl}
        />
        {editorTarget ? (
          <Editor
            key={editorKey}
            ref={editor}
            env={env}
            data={data}
            target={editorTarget}
            mac={mac}
            onSaved={onSaved}
            onClose={closeEditor}
            onGone={dropGone}
            onDelete={deleteFromEditor}
            onDuplicate={duplicate}
            onShowSnippet={showSnippet}
            onCreateTag={createTag}
          />
        ) : selection.ids.size > 1 ? (
          <section className="pane editor-pane center" aria-label="Snippet" data-pane="editor">
            <div className="empty">
              <h2 className="empty-title">{selection.ids.size} snippets selected</h2>
              <div className="empty-actions">
                <button type="button" className="button" onClick={() => void toggleEnabled([...selection.ids])}>
                  Turn off
                </button>
                <button type="button" className="button danger-quiet" onClick={() => void deleteSnippets([...selection.ids])}>
                  Delete
                </button>
              </div>
            </div>
          </section>
        ) : (
          <section className="pane editor-pane center" aria-label="Snippet" data-pane="editor">
            <p className="help">Select a snippet, or press {shortcutLabel({ key: "N" }, mac)} to make one.</p>
          </section>
        )}
      </main>
    );
  })();

  return (
    <div className="library-root">
      {env.connection === "disconnected" && (
        <div className="banner danger window-banner" role="alert">
          <Icon name="warn" />
          <p>
            <strong>Iamb is not running.</strong> Its background service stopped, so nothing can be saved or expanded.
            {restart === "failed" ? " Iamb could not start it from here. Quit Iamb from its menu and open it again." : ""}
          </p>
          {env.restartService && (
            <button type="button" className="button" disabled={restart === "starting"} aria-busy={restart === "starting" ? true : undefined} onClick={() => void startAgain()}>
              {restart === "starting" ? "Starting…" : "Start it again"}
            </button>
          )}
        </div>
      )}
      {data.error && online && (
        <div className="banner danger window-banner" role="alert">
          <Icon name="warn" />
          <p>{data.error}</p>
          <button type="button" className="button" onClick={() => void data.reload()}>
            Try again
          </button>
        </div>
      )}
      <div className="library-panes">
        <Sidebar
          folders={data.folders}
          tags={data.tags}
          total={data.rows.length}
          folderCounts={data.folderCounts}
          broken={broken}
          view={view}
          onView={changeView}
          onNewFolder={() => setFolderDialog({ folder: null, parent: folder?.id ?? null })}
          onFolderMenu={folderMenu}
          onRenameFolder={(f) => setFolderDialog({ folder: f, parent: f.parentId })}
          onDeleteFolder={askDeleteFolder}
          onNewTag={() => setTagDialog({ tag: null })}
          onTagMenu={tagMenu}
          status={status}
          onStatus={setMenu}
          onTogglePause={() => void api.setPaused(!env.paused).catch(() => {})}
          mac={mac}
        />
        {rightArea}
      </div>

      <MenuHost menu={menu} onClose={() => setMenu(null)} />
      <Toast toast={toast} onDismiss={dismissToast} fallback={focusList} />

      {leave && (
        <LeaveDialog
          label={editor.current?.label() ?? "this snippet"}
          mac={mac}
          busy={leaveBusy}
          error={leaveError}
          onSave={() => void leaveSave()}
          onDiscard={() => {
            const action = leave.action;
            setLeave(null);
            setLeaveError(null);
            // The editor is left, so what was typed goes with it: an abandoned edit restores the saved text.
            if (draft) setDraft(null);
            action();
          }}
          onCancel={() => {
            setLeave(null);
            setLeaveError(null);
          }}
        />
      )}
      {folderDialog && (
        <FolderDialog
          folder={folderDialog.folder}
          folders={data.folders}
          initialParent={folderDialog.parent}
          mac={mac}
          error={dialogError}
          busy={dialogBusy}
          onSave={(f) => void saveFolder(f)}
          onCancel={() => {
            setFolderDialog(null);
            setDialogError(null);
          }}
        />
      )}
      {tagDialog && (
        <TagDialog
          tag={tagDialog.tag}
          mac={mac}
          error={dialogError}
          busy={dialogBusy}
          onSave={(n, c) => void saveTag(n, c)}
          {...(tagDialog.tag
            ? {
                onDelete: () => {
                  const t = tagDialog.tag;
                  setTagDialog(null);
                  if (t) askDeleteTag(t);
                },
              }
            : {})}
          onCancel={() => {
            setTagDialog(null);
            setDialogError(null);
          }}
        />
      )}
      {confirm && (
        <ConfirmDialog
          title={confirm.title}
          body={confirm.body}
          confirm={confirm.confirm}
          mac={mac}
          onConfirm={() => {
            const run = confirm.run;
            setConfirm(null);
            void run().catch((e: unknown) => showToast([{ text: describeFailure(e, "delete").text }]));
          }}
          onCancel={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
