# ADR-0022: Snippets from other people are reviewed before they can press keys or read the clipboard

Status: proposed (awaiting owner approval)

**Context.** Anyone with edit rights on a shared folder controls text that other members' machines will type. Text followed by Enter runs a command in a terminal; `{clipboard}` followed by Enter posts the clipboard into a chat.

**Decision.** A snippet created or changed by another user is held for review before its first expansion if its template contains `{key}`, `{wait}`, `{clipboard}` or a line break. The review shows the author and the difference from the last approved revision; until approval the last approved revision keeps expanding, or nothing. Approvals are stored per device. Single-line plain text from other members expands without review. Templates can use no modifier other than Shift. Rich content has raw HTML disabled and link schemes restricted; attachments are scoped to a profile and verified by hash.

**Consequences.** The dangerous capabilities need one deliberate click per change, per device; the common case (sharing canned single-line replies) has no friction. The documentation states plainly that an approved snippet can do whatever typing can do.

**Rejected.** Claiming templates are harmless because they cannot press shortcuts (false). Reviewing every remote change (too much friction for families and teams; people would stop reading the prompts). Banning `{key}` and `{clipboard}` in shared folders (removes legitimate uses such as shared form-filling snippets).
