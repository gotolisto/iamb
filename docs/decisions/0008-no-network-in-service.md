# ADR-0008: The service has no network code, checked four ways in CI

Status: proposed (awaiting owner approval)

**Context.** The product promise is that typed keys never leave the machine. A promise is stronger when it can be checked mechanically, and a crate deny list alone is not enough: the standard library and the Windows bindings both contain network APIs.

**Decision.** CI fails unless all of these hold for `iamb-service`: its resolved dependency tree per target contains no crate on the networking deny list; the enabled features of the Windows bindings are on an allow list that excludes networking namespaces; a lint forbids `std::net` in the service, IPC, store and platform crates; and the release binary is built with `-p iamb-service` alone so that feature unification cannot add code to it. The IPC transport uses only local sockets and pipes.

**Consequences.** The Phase 2 privacy review starts from machine-checked facts. Features that need the network (sync, updates, crash upload) must live in another process.

**Rejected.** A code-review-only rule (erodes over time). An OS firewall rule installed with the app (not portable, needs elevation, can be removed).
