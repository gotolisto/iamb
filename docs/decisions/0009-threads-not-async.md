# ADR-0009: The service uses threads and channels, not an async runtime

Status: accepted (approved by the owner on 2026-11-18)

**Context.** The service handles one key at a time and a handful of local clients. It must idle near zero CPU and stay small.

**Decision.** Dedicated threads (listener, engine, injector, focus, store, one reader per IPC client) connected by bounded channels. No async runtime in the service, and `iamb-ipc` is blocking-only. `iamb-sync`, which does real network I/O, uses an async runtime internally and talks to the service through the blocking client on its own thread.

**Consequences.** Blocking calls into OS APIs and SQLite are natural. Slow OS queries (focus inspection) cannot stall the key path because they have their own thread and a timeout. Fewer dependencies in the keystroke process and no runtime wake-ups while idle. IPC is capped at 8 clients, which is ample.

**Rejected.** An async runtime everywhere (most popular runtimes bring network code into the service, against ADR-0008, and OS hooks need dedicated threads regardless).
