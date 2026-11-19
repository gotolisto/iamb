# ADR-0026: Enter and Tab are not default delimiters

Status: accepted (the owner chose this after the Phase 2 privacy review)

**Context.** Architecture §4.4 made Enter and Tab default delimiters, and ADR-0010 says the listener never holds a key back. Both cannot hold safely for keys the application acts on. The application receives the real Enter or Tab first: Tab moves to the next field of a form, Enter sends a message or closes a dialog. Iamb's deletion, expansion and second key press arrive after that, in whatever has the focus by then. The service cancels only if the focus notification reaches it before the injector's start marker comes back, which takes well under a millisecond. The privacy review (task 2.8, finding 1) showed on the mock that the expansion lands in the password field in nearly every round once the notification is 0.1 ms late, deleting characters of that field first.

**Decision.**
1. The default delimiter set is space and `. , ; : ! ? ) ] } " '`. Enter and Tab end a word (they clear the buffer) without expanding it.
2. A user can add Enter (`\n`) and Tab (`\t`) to the `delimiters` setting. The settings screen (Phase 3) must say what that does in forms, chats and dialogs when they are turned on.
3. Independently, both injectors stop between steps when the focus has changed since the start marker, so any trigger that loses the race to a focus change sends at most one step to the wrong place.

**Consequences.** Abbreviations at the end of a line or a form field need a space or punctuation, or an immediate trigger. Sessions that test Enter and Tab as delimiters now set them explicitly. The compatibility matrix is unaffected: it uses immediate triggers.

**Rejected.** Keeping them and waiting a fixed time after the key for a focus notification: it adds delay to every such expansion and still loses to a slow application or browser accessibility layer. Keeping only Enter: Enter still sends chats and closes dialogs before the expansion arrives.
