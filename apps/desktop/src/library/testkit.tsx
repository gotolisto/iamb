// A fake service for the component tests: the library commands over an in-memory library, with
// the same rules the service applies that the interface relies on (abbreviations are unique
// without regard to case, a deleted snippet comes back when its id is written again, tag names
// are unique, folders nest four levels deep, `LibraryChanged` names the rows it changed).
//
// Test data is invented; nothing here is a user's text.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach } from "vitest";

import type { ConnectionState, UiEvent } from "../ipc";
import type {
  AbbreviationCheck,
  Defaults,
  Folder,
  Health,
  LibraryApi,
  Saved,
  Snippet,
  SnippetDraft,
  Summary,
  Tag,
} from "./api";
import { MAX_FOLDER_DEPTH, TAG_COLORS } from "./api";
import type { LibraryEnv } from "./env";
import { Library } from "./Library";
import { folderLevel } from "./model";
import type { TemplateService } from "./template";
import { referenceTokens } from "./template/reference-tokenizer";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

/** The template commands over the test tokenizer: tokens as the service answers them, the
 * unclosed-macro error (the only one the tests need), and an empty preview. */
export const fakeTemplate: TemplateService = {
  parse: async (source) => {
    let depth = 0;
    for (const ch of source) {
      if (ch === "{") depth += 1;
      if (ch === "}") depth -= 1;
    }
    return {
      tokens: referenceTokens(source),
      error:
        depth > 0
          ? { code: "E_UNCLOSED_MACRO", start: source.lastIndexOf("{"), end: source.length, snippet: null }
          : null,
    };
  },
  preview: async () => ({ parts: [], cursor: null, fields: [], error: null }),
};

/** The content field's text as the template editor shows it (chips by their labels). */
export function contentText(): string {
  return screen.getByRole("textbox", { name: "Content" }).textContent ?? "";
}

/** Types into the content field the way the browser reports it to the template editor. */
export async function typeContent(text: string): Promise<void> {
  const box = screen.getByRole("textbox", { name: "Content" });
  for (const ch of text) {
    await act(async () => {
      box.focus();
      // The caret goes to the end of the text, as the person's would after clicking there.
      const sel = window.getSelection();
      sel?.selectAllChildren(box);
      sel?.collapseToEnd();
      await Promise.resolve();
    });
    await act(async () => {
      box.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: ch, bubbles: true, cancelable: true }));
      for (let i = 0; i < 5; i++) await Promise.resolve();
    });
  }
}

/** Replaces the whole content with `text` (select all, then one insertion). */
export async function setContent(text: string): Promise<void> {
  const box = screen.getByRole("textbox", { name: "Content" });
  await act(async () => {
    box.focus();
    window.getSelection()?.selectAllChildren(box);
    box.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: text, bubbles: true, cancelable: true }));
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
}

export const NO_DEFAULTS: Defaults = { abbreviationPrefix: null, trigger: null, caseMode: null };

export class Failure extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

let seq = 0;
const newId = () => `00000000-0000-7000-8000-${String(++seq).padStart(12, "0")}`;

export interface FakeOptions {
  folders?: Partial<Folder>[];
  snippets?: Partial<Snippet>[];
  tags?: Partial<Tag>[];
  /** Usage on this computer by snippet index in `snippets`: uses and the time of the last. */
  usage?: Record<number, { uses: number; lastUsed: number | null }>;
}

export function snippetOf(p: Partial<Snippet> & { abbreviation: string }, folderId: string): Snippet {
  return {
    id: newId(),
    folderId,
    label: "",
    kind: "plain",
    body: "",
    trigger: "delimiter",
    caseMode: "insensitive",
    wordStart: true,
    enabled: true,
    tags: [],
    rev: 0,
    createdAt: 1,
    updatedAt: 1,
    ...p,
  };
}

export function createFake(options: FakeOptions = {}) {
  let clock = 1000;
  let revision = 1;
  const folders = new Map<string, Folder>();
  const tags = new Map<string, Tag>();
  const live = new Map<string, Snippet>();
  const gone = new Map<string, Snippet>();
  const listeners = new Set<(e: UiEvent) => void>();
  const calls: string[] = [];
  let down: string | null = null;
  let listDelayMs = 0;

  const addFolder = (p: Partial<Folder> & { name: string }): Folder => {
    const f: Folder = {
      id: newId(),
      parentId: null,
      position: folders.size,
      defaults: NO_DEFAULTS,
      ...p,
    };
    folders.set(f.id, f);
    return f;
  };
  const rootFolders = options.folders ?? [{ name: "Snippets" }];
  for (const f of rootFolders) addFolder({ name: "Snippets", ...f });
  const firstFolder = [...folders.values()][0] as Folder;
  for (const t of options.tags ?? []) {
    const tag: Tag = { id: newId(), name: "tag", color: "blue", snippets: 0, ...t };
    tags.set(tag.id, tag);
  }
  for (const s of options.snippets ?? []) {
    const sn = snippetOf({ abbreviation: "x", ...s }, s.folderId ?? firstFolder.id);
    live.set(sn.id, sn);
  }

  const usage = new Map<string, { uses: number; lastUsed: number | null }>();
  for (const [index, u] of Object.entries(options.usage ?? {})) {
    const id = [...live.keys()][Number(index)];
    if (id) usage.set(id, u);
  }
  const DAY = 86_400_000;

  const emit = (changed: { snippets?: string[]; folders?: string[]; tags?: string[] } | null) => {
    revision += 1;
    const event: UiEvent = {
      type: "library_changed",
      revision,
      changed: changed && { snippets: [], folders: [], tags: [], ...changed },
    };
    for (const l of [...listeners]) l(event);
  };

  const fail = (code: string, message: string): never => {
    throw new Failure(code, message);
  };
  const guardUp = () => {
    if (down) fail(down, "the service is not connected");
  };
  const fold = (s: string) => s.toLowerCase();

  const brokenOf = (s: Snippet): string | null => {
    let depth = 0;
    for (const ch of s.body) {
      if (ch === "{") depth += 1;
      if (ch === "}") depth -= 1;
    }
    return depth > 0 ? "E_UNCLOSED_MACRO" : null;
  };

  const summary = (s: Snippet): Summary => ({
    id: s.id,
    folderId: s.folderId,
    abbreviation: s.abbreviation,
    label: s.label,
    firstLine: s.body.split("\n")[0] ?? "",
    kind: s.kind,
    trigger: s.trigger,
    enabled: s.enabled,
    tags: s.tags,
    broken: brokenOf(s),
    uses: usage.get(s.id)?.uses ?? 0,
    lastUsed: usage.get(s.id)?.lastUsed ?? null,
    updatedAt: s.updatedAt,
  });

  /** What the service's health would say, with the rules the interface relies on. */
  const healthOf = (): Health => {
    const all = [...live.values()];
    const excluded = all
      .filter((s) => s.enabled && brokenOf(s))
      .map((s) => ({ snippet: s.id, reason: brokenOf(s) as string, kept: null, reference: null, included: null }));
    const shadowed = all.flatMap((a) =>
      a.enabled && a.trigger === "immediate"
        ? all
            .filter((b) => b.enabled && b.id !== a.id && fold(b.abbreviation).startsWith(fold(a.abbreviation)))
            .map((b) => ({ by: a.id, hidden: b.id }))
        : [],
    );
    const skip = new Set([...excluded.map((e) => e.snippet), ...shadowed.map((x) => x.hidden)]);
    const unused = all
      .filter((s) => s.enabled && !skip.has(s.id) && usage.has(s.id))
      .filter((s) => (usage.get(s.id)?.lastUsed ?? 0) < Date.now() - 90 * DAY)
      .map((s) => ({ snippet: s.id, lastUsed: usage.get(s.id)?.lastUsed ?? null }));
    return { shadowed, excluded, unused };
  };

  const check = (abbreviation: string, trigger: string, snippet?: string): AbbreviationCheck => {
    const others = [...live.values()].filter((s) => s.id !== snippet);
    const key = fold(abbreviation);
    return {
      conflict: others.find((s) => fold(s.abbreviation) === key)?.id ?? null,
      hides:
        trigger === "immediate"
          ? others.filter((s) => fold(s.abbreviation).startsWith(key) && fold(s.abbreviation) !== key).map((s) => s.id)
          : [],
      hiddenBy: others
        .filter((s) => s.trigger === "immediate" && key.startsWith(fold(s.abbreviation)) && key !== fold(s.abbreviation))
        .map((s) => s.id),
    };
  };

  const stamp = (): Saved => ({ id: "", rev: 0, updatedAt: ++clock });

  const api: LibraryApi = {
    listFolders: async () => {
      calls.push("listFolders");
      guardUp();
      return { profile: "profile-1", folders: [...folders.values()] };
    },
    listTags: async () => {
      calls.push("listTags");
      guardUp();
      return [...tags.values()]
        .map((t) => ({ ...t, snippets: [...live.values()].filter((s) => s.tags.includes(t.id)).length }))
        .sort((a, b) => a.name.localeCompare(b.name));
    },
    listSummaries: async (offset, limit) => {
      calls.push(`listSummaries ${offset}`);
      if (listDelayMs > 0) await new Promise((r) => setTimeout(r, listDelayMs));
      guardUp();
      const all = [...live.values()];
      const page = all.slice(offset, offset + limit).map(summary);
      const next = offset + limit < all.length ? offset + limit : null;
      return { revision, total: all.length, summaries: page, next };
    },
    getSnippet: async (id) => {
      calls.push("getSnippet");
      guardUp();
      const s = live.get(id);
      if (!s) return fail("not_found", `snippet ${id} not found`);
      return { ...s };
    },
    saveSnippet: async (draft: SnippetDraft) => {
      calls.push("saveSnippet");
      guardUp();
      if (/\s/u.test(draft.abbreviation) || draft.abbreviation === "") {
        fail("invalid", "bad abbreviation");
      }
      const id = draft.id ?? newId();
      if (check(draft.abbreviation, draft.trigger, id).conflict) {
        fail("conflict", "abbreviation already used by another snippet");
      }
      const before = live.get(id) ?? gone.get(id);
      const row: Snippet = {
        ...draft,
        id,
        rev: before?.rev ?? 0,
        createdAt: before?.createdAt ?? clock,
        updatedAt: ++clock,
      };
      gone.delete(id);
      live.set(id, row);
      emit({ snippets: [id] });
      return { id, rev: 0, updatedAt: row.updatedAt };
    },
    deleteSnippet: async (id) => {
      calls.push("deleteSnippet");
      guardUp();
      const s = live.get(id);
      if (!s) return fail("not_found", `snippet ${id} not found`);
      live.delete(id);
      gone.set(id, s);
      emit({ snippets: [id] });
    },
    setEnabled: async (id, enabled) => {
      calls.push("setEnabled");
      guardUp();
      const s = live.get(id);
      if (!s) return fail("not_found", `snippet ${id} not found`);
      s.enabled = enabled;
      s.updatedAt = ++clock;
      emit({ snippets: [id] });
      return { id, rev: 0, updatedAt: s.updatedAt };
    },
    saveFolder: async (draft) => {
      calls.push("saveFolder");
      guardUp();
      const id = draft.id ?? newId();
      const row: Folder = {
        id,
        parentId: draft.parentId,
        name: draft.name,
        position: draft.position,
        defaults: draft.defaults,
      };
      const trial = new Map(folders).set(id, row);
      if (folderLevel([...trial.values()], id) > MAX_FOLDER_DEPTH) {
        fail("invalid", `folder ${id} would be nested 5 levels deep (at most 4)`);
      }
      folders.set(id, row);
      emit({ folders: [id] });
      return { ...stamp(), id };
    },
    deleteFolder: async (id) => {
      calls.push("deleteFolder");
      guardUp();
      if (!folders.has(id)) return fail("not_found", "no such folder");
      const below = new Set([id]);
      for (const f of folders.values()) if (f.parentId && below.has(f.parentId)) below.add(f.id);
      for (const f of below) folders.delete(f);
      for (const s of [...live.values()]) if (below.has(s.folderId)) live.delete(s.id);
      emit(null);
    },
    saveTag: async (draft) => {
      calls.push("saveTag");
      guardUp();
      if (!TAG_COLORS.includes(draft.color)) fail("invalid", "colour");
      const id = draft.id ?? newId();
      if ([...tags.values()].some((t) => t.id !== id && fold(t.name) === fold(draft.name.trim()))) {
        fail("conflict", "a tag has that name");
      }
      tags.set(id, { id, name: draft.name.trim(), color: draft.color, snippets: 0 });
      emit({ tags: [id] });
      return { ...stamp(), id };
    },
    deleteTag: async (id) => {
      calls.push("deleteTag");
      guardUp();
      if (!tags.has(id)) return fail("not_found", "no such tag");
      tags.delete(id);
      const touched: string[] = [];
      for (const s of live.values()) {
        if (s.tags.includes(id)) {
          s.tags = s.tags.filter((t) => t !== id);
          touched.push(s.id);
        }
      }
      emit({ tags: [id], snippets: touched });
    },
    checkAbbreviation: async (abbreviation, trigger, snippet) => {
      calls.push("checkAbbreviation");
      guardUp();
      return check(abbreviation, trigger, snippet);
    },
    health: async () => {
      calls.push("health");
      guardUp();
      return healthOf();
    },
    resetUsage: async () => {
      calls.push("resetUsage");
      guardUp();
      usage.clear();
    },
    setPaused: async () => ({ connection: "connected", service: null }),
    dismiss: async () => {
      calls.push("dismiss");
    },
  };

  return {
    api,
    calls,
    folders,
    tags,
    live,
    gone,
    addFolder,
    /** The service goes down (`code` is what commands fail with) or comes back (null). */
    setDown(code: string | null) {
      down = code;
    },
    /** The snippet list answers this much later (a slow machine: other reads overtake it). */
    setListDelay(ms: number) {
      listDelayMs = ms;
    },
    /** The snippet was used on this computer. */
    setUsage(id: string, uses: number, lastUsed: number | null) {
      usage.set(id, { uses, lastUsed });
    },
    usage,
    /** A change made by someone else. */
    emit,
    listeners,
    byAbbreviation: (a: string) => [...live.values()].find((s) => s.abbreviation === a),
    firstFolder,
  };
}

export type Fake = ReturnType<typeof createFake>;

export function envFor(fake: Fake, over: Partial<LibraryEnv> & { connection?: ConnectionState } = {}): LibraryEnv {
  return {
    api: fake.api,
    onEvent: (h) => {
      fake.listeners.add(h);
      return () => fake.listeners.delete(h);
    },
    connection: "connected",
    paused: false,
    mac: false,
    copyText: async () => {},
    template: fakeTemplate,
    ...over,
  };
}

/** Renders the library over a fake service, and waits for its first read. */
export async function renderLibrary(fake: Fake = createFake(), over: Partial<LibraryEnv> = {}) {
  const env = envFor(fake, over);
  const view = render(<Library env={env} />);
  await view.findByRole("navigation", { name: "Library" });
  return { fake, env, ...view };
}
