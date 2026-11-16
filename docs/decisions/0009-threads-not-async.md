# ADR-0009: The service uses threads and channels, not an async runtime

Status: proposed (awaiting owner approval)

**Context.** The service handles one key at a time and a handful of local clients. It must idle near zero CPU and stay small.

**Decision.** Four kinds of thread (listener, engine, store, IPC clients) connected by bounded channels. No async runtime in the service. `iamb-sync`, which does real network I/O, uses an async runtime.

**Consequences.** Blocking calls into OS APIs and SQLite are natural. Fewer dependencies in the keystroke process and no runtime wake-ups while idle. IPC is capped at 8 clients, which is ample.

**Rejected.** An async runtime everywhere (most popular runtimes bring network code into the service, against ADR-0008, and OS hooks need dedicated threads regardless).
