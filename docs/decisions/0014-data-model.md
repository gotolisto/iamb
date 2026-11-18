# ADR-0014: Profiles own folders; folders are shared to group profiles

Status: proposed (awaiting owner approval)

**Context.** Accounts are individual. Families and companies share selected folders, not whole libraries, and a company must be able to own content that outlives a member's departure. The model must work offline and sync row by row.

**Decision.** Every user has one personal profile. Group profiles (`family`, `company`, same rules) have members with roles `owner`, `admin`, `member`. A folder belongs to exactly one profile. A personally owned folder can be shared to a group through `FolderShare` (`view` or `edit`); a group-owned folder is readable by all members and writable by `owner`, `admin`, and members when its `member_permission` is `edit`. `edit` covers snippets and sub-folders inside the folder, never the folder itself. Tags and attachments belong to the folder owner's profile and are visible with the snippets that use them. Ids are UUID version 7 created on the device. Every synced row has `rev`, `updated_at`, `updated_by` and `deleted_at`. Abbreviations are unique under case-folded comparison across all non-deleted snippets a user can read; inside a profile the store enforces it, and across profiles a fixed precedence (personal, then earliest share, then folder id) picks the holder and library health lists the rest.

**Consequences.** "Share exactly one folder and nothing else" is one row. Rows can be created offline without id collisions. Access rules map directly onto row-level security, including for attachments. Nested-snippet lookup is unambiguous.

**Rejected.** Per-snippet sharing (too fine to reason about). Group-owned libraries with copies (edits diverge). Server-assigned integer ids (no offline creation). Uniqueness among enabled snippets only (a nested reference could match two snippets).
