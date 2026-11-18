# ADR-0024: Engine protocol details settled by the Phase 1 review

Status: accepted (owner decision on explicit requests, 2026-11-18). Amends ADR-0012 (buffer size) and ADR-0013 (explicit requests).

**Context.** Building and reviewing the engine in Phase 1 (`docs/reviews/1.9-phase1-review.md`) showed four places where the approved architecture was silent or too loose: the keystroke buffer was one character too short for the longest abbreviation with the word-start rule; a palette pick or form submit could type into a password field after a focus change; snippets with a resolve error disappeared from matching and let a shorter abbreviation fire instead; and the store and the matcher folded case differently.

**Decision.**
1. The keystroke buffer holds 66 characters: the 64-character maximum abbreviation, the character before it, and one delimiter (amends ADR-0012's "64-character buffer").
2. Explicit requests (palette pick, form submit, reopen last form) never type into a field Iamb has not confirmed as normal. After a focus change they wait for the focus verdict; anything but `normal` refuses them with a diagnostic, and a refused form keeps its values. Pause blocks typed abbreviations only, not deliberate palette picks or form submits (amends ADR-0013).
3. The matchable set is the enabled snippets whose templates parse. A resolve error keeps the snippet matchable and turns its expansion into a diagnostic.
4. One case-folding rule everywhere: per-character lower-casing with final sigma folded to σ.
5. Each expansion carries an id; the injector answers every expansion with start and finish markers or a cancel, and only one expansion is in flight at a time (architecture §4.6).

**Consequences.** A 64-character abbreviation works after other text. Iamb never types a snippet into a password field because of a stale focus. Broken nested references show up as diagnostics instead of silently changing which snippet fires. The store can never accept two abbreviations the matcher treats as one.

**Rejected.** Letting explicit requests through in every state (the review showed text landing in a password field). Blocking palette picks while paused (pause is meant to stop accidental expansions, not deliberate ones). Excluding resolve errors from matching (a shorter abbreviation fires instead, with no diagnostic).
