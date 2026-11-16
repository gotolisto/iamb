# ADR-0012: Typed text lives only in a 64-character in-memory buffer

Status: proposed (awaiting owner approval)

**Context.** The privacy KPI is zero typed keys on disk, in logs or on the network.

**Decision.** The engine keeps at most 64 characters, in memory, zeroed on clear. Typed characters are wrapped in `Sensitive<T>`, which cannot be formatted or serialised. No IPC message carries the buffer. Diagnostics record the snippet and the reason, never the input, and stay in memory. Secure input, excluded apps and global pause stop buffering before the character is stored. Crash dumps omit heap contents.

**Consequences.** Leaking typed text requires defeating the type system on purpose, which a reviewer will see. The "why didn't it expand" log cannot say what the user typed, only which rule applied; that is an accepted limit.

**Rejected.** A debug mode that logs keys (it would ship eventually). Persisting the buffer across restarts (no user benefit).
