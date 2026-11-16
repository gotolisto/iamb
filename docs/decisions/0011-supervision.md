# ADR-0011: A supervisor process from the same binary restarts the worker

Status: proposed (awaiting owner approval)

**Context.** The service must restart in under 2 seconds after a crash on both systems. The login-item mechanisms differ and the Windows scheduler cannot restart that fast.

**Decision.** `iamb-service --supervise` is the login item (LaunchAgent on macOS, per-user Run entry on Windows). It holds a lock file for single-instance, starts the worker, and restarts it on abnormal exit with backoff. It also starts the UI host and the sync agent when configured. It contains no hooks and handles no user data.

**Consequences.** Identical restart behaviour and one code path on both systems. Two service processes appear in the process list. No administrator rights are needed to install.

**Rejected.** A system-wide service (cannot see the user's keyboard session without extra machinery, needs elevation). Relying on each OS's own restart rules (different behaviour; too slow on Windows).
