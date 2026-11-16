# ADR-0003: `iamb-core` is pure: no I/O, clock, threads or OS calls

Status: proposed (awaiting owner approval)

**Context.** The same engine must run in the desktop service, the command-line tool, the editor preview (as WebAssembly) and the mobile keyboards, and its behaviour must be testable to the last character.

**Decision.** `iamb-core` contains only deterministic logic. Time, locale and clipboard arrive through the `Environment` trait; storage arrives as an immutable library snapshot. `unsafe` is forbidden.

**Consequences.** Every spec example and typing session is a plain unit test that runs on any OS. Property tests and fuzzing need no set-up. Callers must do the I/O and pass results in, which adds a little plumbing.

**Rejected.** Letting the core read the clock or clipboard directly (non-deterministic tests, no WebAssembly build). An async core (nothing in it waits).
