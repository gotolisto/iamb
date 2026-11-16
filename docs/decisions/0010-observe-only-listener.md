# ADR-0010: The listener observes keys and never blocks or swallows them

Status: proposed (awaiting owner approval)

**Context.** A slow or crashed keyboard hook makes the whole computer feel broken, and operating systems remove hooks that respond slowly.

**Decision.** The hook callback translates the key, stamps a sequence number, pushes an event to a bounded channel and returns. It never waits on the engine and never suppresses a key. Abbreviations therefore appear on screen and are removed with `DeleteBack` on expansion. If the channel is full the event is dropped and the next delivered event is `Overflow`, which clears the buffer. The search-palette hotkey uses the OS hotkey API. The consequence for fast typists, keys arriving during injection, is handled by ADR-0019.

**Consequences.** Typing latency is unaffected by Iamb. The worst failure is a missed or abandoned expansion, never a lost or delayed keystroke. Expansion needs a reliable delete, which the per-app strategy table handles.

**Rejected.** Swallowing abbreviation keys until a match is decided (adds latency to every key and loses input if the service stalls).
