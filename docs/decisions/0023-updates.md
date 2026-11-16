# ADR-0023: The UI host applies updates; the supervisor stops and restarts the worker around them

Status: proposed (awaiting owner approval)

**Context.** The service has no network code, so it cannot fetch its own updates, and its binary cannot be replaced while it runs on Windows.

**Decision.** `iamb-desktop` downloads and verifies signed updates. To apply one it asks the supervisor to stop the worker and the sync agent, replaces the files, and asks the supervisor to start them again; the supervisor then restarts itself from the new binary. Expansion pauses for a few seconds during an update, which is applied only when the keyboard has been idle for 30 seconds or at the next login. Channels, rollback and signature rules belong to task 5.2.

**Consequences.** The no-network rule holds. A failed update leaves the old worker binary in place and running. The supervisor gains two control messages (stop, start) on a private pipe to the UI host with the peer verification of ADR-0006.

**Rejected.** An updater inside the service (network code next to keystrokes). A separate elevated updater service (needs administrator rights to install).
