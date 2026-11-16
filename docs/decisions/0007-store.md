# ADR-0007: SQLite in WAL mode; the service is the single writer

Status: proposed (awaiting owner approval)

**Context.** A library is at most tens of thousands of small rows, must survive a kill during a write, and must load in under 100 ms.

**Decision.** One SQLite file per user (`library.db`, WAL, bundled SQLite build), with forward-only numbered migrations. While the service runs it is the only writer; other processes go through IPC. The command-line tool may open the file directly only when the service is not running. After each write the store publishes a new immutable snapshot to the engine. Attachment bytes are files named by their SHA-256, not rows.

**Consequences.** One validation path for every edit, whoever makes it. No write contention. Crash safety comes from SQLite. The engine never touches the disk on the key path.

**Rejected.** Plain files such as one text file per snippet (no transactions, slow at 10,000 snippets, awkward revisions). Several writer processes on the same file (two validation paths, change notification needed anyway). An embedded key-value store (no ad hoc queries for search and library health).
