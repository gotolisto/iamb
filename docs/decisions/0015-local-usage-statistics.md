# ADR-0015: Usage statistics stay on the device

Status: proposed (awaiting owner approval)

**Context.** Library health needs to know which snippets are unused, but usage counts reveal behaviour.

**Decision.** Expansion counts, last-used times and characters saved are stored in a device-local table and are not synced in version 1.

**Consequences.** Nothing about how a user types reaches the server. "Unused" is judged per device, which the UI states. Syncing aggregates can be added later as an opt-in without a schema break.

**Rejected.** Syncing usage by default (privacy cost for a minor feature; in shared folders it would expose members' activity to each other).
