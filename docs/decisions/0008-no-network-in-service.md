# ADR-0008: The service has no network code, checked in CI

Status: proposed (awaiting owner approval)

**Context.** The product promise is that typed keys never leave the machine. A promise is stronger when it can be checked mechanically.

**Decision.** `iamb-service` and everything it depends on must not contain networking crates. A CI job inspects its resolved dependency tree against a deny list (HTTP clients, TLS stacks, async network runtimes, socket crates other than the local IPC transport) and fails on any match. The IPC transport uses only local sockets and pipes.

**Consequences.** The Phase 2 privacy review starts from a machine-checked fact. Features that need the network (sync, updates, crash upload) must live in another process.

**Rejected.** A code-review-only rule (erodes over time). An OS firewall rule installed with the app (not portable, needs elevation, can be removed).
