import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { ChangedRows } from "../ipc";
import type { Folder, LibraryApi, Summary, Tag } from "./api";
import type { LibraryEnv } from "./env";
import { countByFolder, indexRows, sortIndexed, type Indexed, type SortMode } from "./model";

const PAGE = 1000;
/** Library changes that arrive together are read once. */
const DEBOUNCE_MS = 40;

/** Every summary, in pages; read again when the library changed between two pages. */
export async function fetchAllSummaries(api: LibraryApi): Promise<Summary[]> {
  let last: unknown;
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const first = await api.listSummaries(0, PAGE);
      const rest: Promise<Awaited<ReturnType<LibraryApi["listSummaries"]>>>[] = [];
      if (first.next !== null) {
        for (let offset = first.next; offset < first.total; offset += PAGE) {
          rest.push(api.listSummaries(offset, PAGE));
        }
      }
      const pages = await Promise.all(rest);
      if (pages.every((p) => p.revision === first.revision)) {
        return pages.reduce((all, p) => all.concat(p.summaries), first.summaries);
      }
    } catch (e) {
      last = e;
      break;
    }
  }
  throw last ?? new Error("the library kept changing while it was read");
}

export interface LibraryData {
  /** The first answer has arrived. */
  ready: boolean;
  profile: string;
  folders: Folder[];
  tags: Tag[];
  /** All snippets, in the order of `sort`, with what the filter looks in. */
  items: Indexed[];
  rows: Summary[];
  rowsById: ReadonlyMap<string, Summary>;
  folderCounts: ReadonlyMap<string, number>;
  /** A sentence when the last read failed. */
  error: string | null;
  /** Reads what changed (everything by default); resolves with the new rows. */
  reload: (parts?: { folders?: boolean; tags?: boolean; rows?: boolean }) => Promise<Summary[]>;
  /** Counts library events, and names the snippets of the last (null: any). */
  change: { n: number; snippets: string[] | null };
}

function sentence(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e && typeof e.message === "string") {
    return e.message;
  }
  return "The snippets could not be loaded.";
}

/** The library held in memory: folders, tags and every snippet's summary, kept current. */
export function useLibraryData(env: LibraryEnv, sort: SortMode): LibraryData {
  const { api, onEvent, connection } = env;
  const [profile, setProfile] = useState("");
  const [folders, setFolders] = useState<Folder[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [rows, setRows] = useState<Summary[]>([]);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [change, setChange] = useState<{ n: number; snippets: string[] | null }>({
    n: 0,
    snippets: null,
  });
  const rowsRef = useRef<Summary[]>([]);
  const epoch = useRef({ folders: 0, tags: 0, rows: 0 });
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  const reload = useCallback(
    async (parts: { folders?: boolean; tags?: boolean; rows?: boolean } = {}) => {
      const all = parts.folders === undefined && parts.tags === undefined && parts.rows === undefined;
      const want = {
        folders: all || parts.folders === true,
        tags: all || parts.tags === true,
        rows: all || parts.rows === true,
      };
      const jobs: Promise<void>[] = [];
      let fresh = rowsRef.current;
      if (want.folders) {
        const mine = ++epoch.current.folders;
        jobs.push(
          api.listFolders().then((f) => {
            if (live.current && mine === epoch.current.folders) {
              setProfile(f.profile);
              setFolders(f.folders);
            }
          }),
        );
      }
      if (want.tags) {
        const mine = ++epoch.current.tags;
        jobs.push(
          api.listTags().then((t) => {
            if (live.current && mine === epoch.current.tags) setTags(t);
          }),
        );
      }
      if (want.rows) {
        const mine = ++epoch.current.rows;
        jobs.push(
          fetchAllSummaries(api).then((r) => {
            if (live.current && mine === epoch.current.rows) {
              rowsRef.current = r;
              setRows(r);
            }
            fresh = r;
          }),
        );
      }
      try {
        await Promise.all(jobs);
        if (live.current) {
          setError(null);
          setReady(true);
        }
      } catch (e) {
        if (live.current) setError(sentence(e));
      }
      return fresh;
    },
    [api],
  );

  // The first read, and again after the connection came back.
  useEffect(() => {
    if (connection === "connected") void reload();
  }, [connection, reload]);

  // Library events: read what they name, after a moment so that a burst is one read.
  useEffect(() => {
    const pending = { folders: false, tags: false, rows: false };
    let timer: ReturnType<typeof setTimeout> | undefined;
    const flush = () => {
      timer = undefined;
      const parts = { ...pending };
      pending.folders = pending.tags = pending.rows = false;
      void reload(parts);
    };
    const stop = onEvent((event) => {
      if (event.type !== "library_changed") return;
      const changed: ChangedRows | null = event.changed;
      if (changed === null) {
        pending.folders = pending.tags = pending.rows = true;
      } else {
        if (changed.snippets.length > 0) pending.rows = true;
        if (changed.folders.length > 0) pending.folders = true;
        if (changed.tags.length > 0) pending.tags = true;
      }
      setChange((c) => ({ n: c.n + 1, snippets: changed === null ? null : changed.snippets }));
      if (timer === undefined) timer = setTimeout(flush, DEBOUNCE_MS);
    });
    return () => {
      stop();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [onEvent, reload]);

  const indexed = useMemo(() => indexRows(rows, tags), [rows, tags]);
  const items = useMemo(() => sortIndexed(indexed, sort), [indexed, sort]);
  const rowsById = useMemo(() => new Map(rows.map((r) => [r.id, r])), [rows]);
  const folderCounts = useMemo(() => countByFolder(rows), [rows]);

  return { ready, profile, folders, tags, items, rows, rowsById, folderCounts, error, reload, change };
}
