# ADR-0001: Three processes: service, UI host, sync agent

Status: accepted (approved by the owner on 2026-11-18)

**Context.** Expansion must keep working whatever the UI does, typed keys must never reach the network, and the service must idle under 20 MB.

**Decision.** Run three per-user processes: `iamb-service` (keyboard, engine, store), `iamb-desktop` (tray and windows), `iamb-sync` (network). Only the service sees keystrokes; only the other two have network access.

**Consequences.** A UI or sync crash cannot stop expansion. The privacy review can prove "no network code in the service" from the dependency tree. The cost is an IPC layer and three things to install and supervise.

**Rejected.** One process (a web view crash or leak would take the keyboard hook with it, and the key path would share an address space with network code). Two processes with sync inside the UI host (sync would stop whenever the UI host is closed). Sync inside the service (breaks the no-network rule).
