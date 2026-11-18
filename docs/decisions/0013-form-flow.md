# ADR-0013: Forms delete the abbreviation only after submit; the focus holder hands focus back

Status: proposed (awaiting owner approval)

**Context.** A form interrupts typing; the user may cancel, and focus leaves the target app while the form is open. Operating systems let only the process that holds the foreground give it away, so a background service cannot restore focus.

**Decision.** On a match that needs a form, the engine captures the clock and clipboard, opens the single form session, and emits only `ShowForm`. The service attaches a focus token for the target window. The UI host shows the form (a non-activating panel on macOS; with a foreground grant from the service on Windows), and on submit or cancel it returns focus to the token's window itself and confirms it before answering. Only then does the engine emit `DeleteBack` and the rendered text. A second trigger while a form is open is dropped. The palette uses the same token mechanism.

**Consequences.** Cancel has no side effects. Dates and clipboard content reflect the moment the abbreviation was typed, not the moment of submit. Focus hand-back works within the rules of both systems. The form panels need a little native window code. If the user moved the caret in the target app before submitting, the wrong characters could be deleted; this is accepted and listed for the compatibility matrix.

**Rejected.** Deleting first and retyping the abbreviation on cancel (two injections where focus can go wrong). Having the service restore focus (refused by Windows foreground rules and ignored by macOS cooperative activation). Several simultaneous forms.
