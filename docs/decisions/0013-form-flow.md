# ADR-0013: Fill-in forms delete the abbreviation only after submit

Status: proposed (awaiting owner approval)

**Context.** A form interrupts typing; the user may cancel, and focus leaves the target app while the form is open.

**Decision.** On a match that needs a form, the engine emits only `ShowForm`. The service remembers the focused window. On submit it restores focus, then the engine emits `DeleteBack` and the rendered text. On cancel, or if no UI host answers within 2 seconds, nothing changes.

**Consequences.** Cancel has no side effects and needs no undo. The abbreviation stays visible behind the form, which also confirms what was triggered. If the user moved the caret in the target app before submitting, the wrong characters could be deleted; the form window is modal-like and brief, so this is accepted and listed for the compatibility matrix.

**Rejected.** Deleting first and retyping the abbreviation on cancel (two injections where focus can go wrong). A dynamic form inside the target app (not possible across apps).
