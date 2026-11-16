# ADR-0011: A supervisor process restarts crashed and stuck workers

Status: proposed (awaiting owner approval)

**Context.** The service must restart in under 2 seconds after a crash on both systems. The login-item mechanisms differ and the Windows scheduler cannot restart that fast. A worker can also be alive but useless: a deadlocked thread, or a hook the OS removed silently.

**Decision.** `iamb-service --supervise` is the login item. It holds a lock file for single-instance, starts the worker, the UI host and the sync agent, and restarts each on abnormal exit with backoff. The worker's engine thread sends a heartbeat every 500 ms; after 3 seconds of silence the supervisor kills and restarts the worker. Every 30 seconds the worker sends a marked no-op key event through the OS and expects its own listener to see it; on failure it reinstalls the hook, and after two failures it exits. The supervisor contains no hooks and handles no user data.

**Consequences.** Identical restart behaviour and one code path on both systems; hangs and dead hooks are recovered without the user noticing more than a few seconds of missed expansions. Two service processes appear in the process list. No administrator rights are needed.

**Rejected.** A system-wide service (cannot see the user's keyboard session without extra machinery, needs elevation). Relying on each OS's own restart rules (different behaviour; too slow on Windows; blind to hangs).
