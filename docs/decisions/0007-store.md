# ADR-0007: SQLite in WAL mode; one writer, guarded by a lock file

Status: proposed (awaiting owner approval)

**Context.** A library is at most tens of thousands of small rows, must survive a kill during a write, and must load in under 100 ms.

**Decision.** One SQLite file per user (`library.db`, WAL, bundled SQLite build), with forward-only numbered migrations. A process that opens it for writing first takes an exclusive OS lock on `run/store.lock`. On the desktop the service worker holds that lock while it runs and other processes go through IPC; the command-line tool opens the store itself only when it can take the lock. After a change to library content or engine settings the store thread builds a new immutable snapshot; reads, search and dry runs are served from the snapshot. Usage and sync bookkeeping writes do not rebuild it. Attachment bytes are files, not rows.

**Consequences.** One validation path for every edit. No write contention and no race between the command-line tool and a restarting worker. Crash safety comes from SQLite. Neither the engine nor a search touches the disk. The same lock rule serves the mobile app and keyboard extension.

**Rejected.** Plain files such as one text file per snippet (no transactions, slow at 10,000 snippets, awkward revisions). Several writers relying on SQLite locking alone (two validation paths; change notification needed anyway). Deciding "is the service running" by whether the endpoint exists (stale sockets, restart windows). Full-text search in SQLite for the palette (prefix matching only, and queries would queue behind writes).
