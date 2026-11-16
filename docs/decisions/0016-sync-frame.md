# ADR-0016: Sync: server revisions, last write wins per row, outbox on the device

Status: proposed (awaiting owner approval)

**Context.** Devices edit offline; two online devices must agree within 5 seconds; no edit may be lost silently.

**Decision.** The server (Postgres with row-level security, on Supabase) assigns a monotonically increasing revision to every row change. Devices record local changes in an `outbox` table in the same transaction as the change. `iamb-sync` pushes the outbox in order and pulls by revision, applying remote rows through the service. Conflicts resolve per row by last write wins, and the server keeps overwritten versions in a history table. Deletes are soft and purged after 30 days. Attachment bytes go to object storage by hash and are fetched lazily. Task 4.1 specifies the conflict scenarios in detail.

**Consequences.** Simple, predictable rules that fit small, rarely contended rows. A lost concurrent edit is recoverable from history. Field-level merging is not attempted.

**Rejected.** CRDTs or operational transforms (complexity out of proportion to snippet editing). Whole-library file sync (conflicts lose whole libraries). Client clocks as the ordering authority (clock skew).
