# ADR-0016: Sync: a per-user change feed, last write wins per row, outbox on the device

Status: proposed (awaiting owner approval)

**Context.** Devices edit offline; two online devices must agree within 5 seconds; a newly shared folder must appear within 10 seconds; revoked content must disappear; no edit may be lost silently.

**Decision.** The server (Postgres with row-level security, on Supabase) keeps an ordered change feed per user containing row changes the user may see and visibility changes (`folder_visible`, `folder_revoked`). Devices keep one feed position. A visibility grant triggers a full fetch of the folder; a revocation triggers a local purge. Devices record local changes in an `outbox` in the same transaction as the change, with the revision each was based on. The server applies last write wins per row, keeps overwritten versions in a history table, and records a conflict when the base revision was stale so the device can show it. Deletes are soft and purged after 30 days. Attachments are stored per profile by hash and downloaded eagerly for the effective library, within size limits, by the sync agent, which writes them to disk itself. Task 4.1 specifies the conflict scenarios in detail.

**Consequences.** Sharing and unsharing take effect on every device without relying on row revisions. A stale offline edit still wins, but visibly and reversibly. Rich snippets work offline after the first sync. Field-level merging is not attempted.

**Rejected.** Pulling "rows with a revision above my cursor" per profile (never returns existing rows of a newly shared folder, never reports lost access, and cannot be keyed by profiles the user cannot query). CRDTs or operational transforms (complexity out of proportion to snippet editing). Whole-library file sync. Client clocks as the ordering authority. Lazy attachment download (breaks offline use and gives the network-less service no way to fetch).
