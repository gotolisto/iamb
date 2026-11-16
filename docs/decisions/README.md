# Decision records

One record per architectural choice. A record is changed only by adding a new record that supersedes it.
See the [architecture document](../architecture.md) for how they fit together.

| Record | Decision |
| --- | --- |
| [ADR-0001](0001-three-processes.md) | Three processes: service, UI host, sync agent |
| [ADR-0002](0002-workspace-and-crate-names.md) | One Cargo workspace; crates named `iamb-*` |
| [ADR-0003](0003-pure-core.md) | `iamb-core` is pure: no I/O, clock, threads or OS calls |
| [ADR-0004](0004-engine-events-and-actions.md) | The engine is a state machine: events in, actions out |
| [ADR-0005](0005-platform-traits.md) | OS code sits behind traits in per-OS crates |
| [ADR-0006](0006-ipc.md) | Local IPC: length-prefixed JSON over a user-only socket or named pipe |
| [ADR-0007](0007-store.md) | SQLite in WAL mode; the service is the single writer |
| [ADR-0008](0008-no-network-in-service.md) | The service has no network code, checked in CI |
| [ADR-0009](0009-threads-not-async.md) | The service uses threads and channels, not an async runtime |
| [ADR-0010](0010-observe-only-listener.md) | The listener observes keys and never blocks or swallows them |
| [ADR-0011](0011-supervision.md) | A supervisor process from the same binary restarts the worker |
| [ADR-0012](0012-keystroke-privacy.md) | Typed text lives only in a 64-character in-memory buffer |
| [ADR-0013](0013-form-flow.md) | Fill-in forms delete the abbreviation only after submit |
| [ADR-0014](0014-data-model.md) | Profiles own folders; folders are shared to group profiles |
| [ADR-0015](0015-local-usage-statistics.md) | Usage statistics stay on the device |
| [ADR-0016](0016-sync-frame.md) | Sync: server revisions, last write wins per row, outbox on the device |
| [ADR-0017](0017-ui-host.md) | Tauri 2 UI host: resident tray, pre-warmed form and palette |
| [ADR-0018](0018-rich-content.md) | Rich snippets are Markdown plus attachments, rendered in the core |
