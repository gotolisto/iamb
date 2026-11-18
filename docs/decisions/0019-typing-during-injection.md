# ADR-0019: Keys typed during an expansion are compensated or the expansion is abandoned

Status: proposed (awaiting owner approval)

**Context.** The listener only observes (ADR-0010). At normal typing speed the next key can reach the application after the trigger and before or during Iamb's delete and insert, which would delete the wrong characters or split the snippet.

**Decision.** Injection runs on its own thread. The injector brackets its output with marked start and end events that the listener reports in stream order. Real keys seen after the trigger and before the start marker are counted: the injector deletes that many extra characters and types them again after the expansion; a boundary key among them cancels the expansion. Where the OS delivers a synthetic batch without interleaving (Windows), delete and text go in one batch. Real keys seen between the markers cannot be repaired; the engine clears its buffer and records a diagnostic. The reliability run measures this at realistic typing speeds, and it counts against the 99.9% target.

**Consequences.** Fast typists get correct text in the common case and a visible, bounded failure in the rare one. The engine has an explicit "injecting" state that scripted sessions can test. A sub-millisecond window between counting and sending remains and is measured, not eliminated.

**Rejected.** Ignoring the problem (wrong deletions for anyone typing above roughly 60 words per minute). Swallowing keys during injection (contradicts ADR-0010 and risks lost input if the injector stalls).
