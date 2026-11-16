# ADR-0014: Profiles own folders; folders are shared to group profiles

Status: proposed (awaiting owner approval)

**Context.** Accounts are individual. Families and companies share selected folders, not whole libraries. The model must work offline and sync row by row.

**Decision.** Every user has one personal profile. Group profiles (`family`, `company`) have members with roles. A folder belongs to exactly one profile and can be shared to group profiles through `FolderShare` with `view` or `edit` permission; sub-folders follow their parent. Ids are UUID version 7 created on the device. Every synced row has `rev`, `updated_at` and `deleted_at`. Abbreviations are unique within a user's effective library; on a conflict the personal snippet wins and library health reports it.

**Consequences.** "Share exactly one folder and nothing else" is one row. Rows can be created offline without id collisions. Access rules reduce to "own profile, or a share to a profile I belong to", which maps directly onto row-level security.

**Rejected.** Per-snippet sharing (too fine to reason about; folders are the unit users think in). Group-owned libraries with copies (edits diverge). Server-assigned integer ids (no offline creation).
