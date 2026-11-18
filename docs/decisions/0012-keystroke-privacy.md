# ADR-0012: Typed text lives only in a 64-character in-memory buffer; entered values are never stored

Status: accepted (approved by the owner on 2026-11-18)

Amended by [ADR-0024](0024-engine-protocol-after-phase-1.md): the buffer holds 66 characters, and while an expansion is in flight the listener keeps a second, zeroed copy of the characters typed after the trigger (architecture §8 rule 1).

**Context.** The privacy KPI is zero typed keys on disk, in logs or on the network. Form values, clipboard content and palette queries are also the user's words and deserve the same care.

**Decision.** The engine keeps at most 64 characters, in locked memory, zeroed on clear. Typed characters and user-entered values are wrapped in `Sensitive<T>`, which cannot be formatted. No IPC message carries the buffer, and IPC bodies are never logged. Diagnostics name a snippet only after a match and never the input; they stay in memory. Secure fields, excluded apps, input-method composition, Iamb's own windows and any unknown focus state suspend buffering before a character is stored: after a focus change the engine waits for the focus thread's verdict and fails closed. Last form values are kept only in the worker's memory for at most 30 minutes. Pasted content is marked to stay out of clipboard history. Crash reports omit heap contents.

**Consequences.** Leaking typed text requires defeating the type system on purpose, which a reviewer will see. The first characters typed within about 100 ms of a focus change cannot trigger an expansion. The "why didn't it expand" log cannot say what the user typed. An administrator-configured full-memory dump remains outside Iamb's control and is documented as a limit.

**Rejected.** A debug mode that logs keys (it would ship eventually). Persisting the buffer or form values across restarts. Buffering while the secure state is still being determined (the first characters of a password would be stored and could trigger).
