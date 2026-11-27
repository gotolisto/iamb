# ADR-0023: The UI host applies updates; the supervisor stops and restarts the worker around them

Status: accepted (approved by the owner on 2026-11-18)

**Context.** The service has no network code, so it cannot fetch its own updates, and its binary cannot be replaced while it runs on Windows.

**Decision.** `iamb-desktop` downloads and verifies signed updates. To apply one it asks the supervisor to stop the worker and the sync agent, replaces the files, and asks the supervisor to start them again; the supervisor then restarts itself from the new binary. Expansion pauses for a few seconds during an update, which is applied only when the keyboard has been idle for 30 seconds or at the next login. Channels, rollback and signature rules belong to task 5.2.

**Consequences.** The no-network rule holds. A failed update leaves the old worker binary in place and running. The supervisor gains two control messages (stop, start) on a private pipe to the UI host with the peer verification of ADR-0006.

**Rejected.** An updater inside the service (network code next to keystrokes). A separate elevated updater service (needs administrator rights to install).

**Note (2026-11-22, the beta-gap review's finding 5).** Before the pipe exists, "Start it again" and "Restart the helper" use a stand-in: the UI host leaves the empty file `run/restart-worker`, which the supervisor takes as "restart the worker". It has no peer verification, rests on `run/` being the user's alone, and can do nothing the user's processes cannot already do by ending the worker (architecture §3.2, §8). It must not carry a stop or an update; those need the pipe this ADR decides.
