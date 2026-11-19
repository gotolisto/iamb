# Decision records

One record per architectural choice. Once approved, a record is changed only by adding a new record that supersedes it.
See the [architecture document](../architecture.md) for how they fit together.

| Record | Decision |
| --- | --- |
| [ADR-0001](0001-three-processes.md) | Three processes: service, UI host, sync agent |
| [ADR-0002](0002-workspace-and-crate-names.md) | One Cargo workspace; crates named `iamb-*` |
| [ADR-0003](0003-pure-core.md) | `iamb-core` is pure: no I/O, clock, threads or OS calls |
| [ADR-0004](0004-engine-events-and-actions.md) | The engine is a state machine: events in, actions out |
| [ADR-0005](0005-platform-traits.md) | OS code sits behind traits in per-OS crates |
| [ADR-0006](0006-ipc.md) | Local IPC: length-prefixed JSON over a verified, user-only socket or named pipe |
| [ADR-0007](0007-store.md) | SQLite in WAL mode; one writer, guarded by a lock file |
| [ADR-0008](0008-no-network-in-service.md) | The service has no network code, checked four ways in CI |
| [ADR-0009](0009-threads-not-async.md) | The service uses threads and channels, not an async runtime |
| [ADR-0010](0010-observe-only-listener.md) | The listener observes keys and never blocks or swallows them |
| [ADR-0011](0011-supervision.md) | A supervisor process restarts crashed and stuck workers |
| [ADR-0012](0012-keystroke-privacy.md) | Typed text lives only in a 64-character in-memory buffer; entered values are never stored |
| [ADR-0013](0013-form-flow.md) | Forms delete the abbreviation only after submit; the focus holder hands focus back |
| [ADR-0014](0014-data-model.md) | Profiles own folders; folders are shared to group profiles |
| [ADR-0015](0015-local-usage-statistics.md) | Usage statistics stay on the device |
| [ADR-0016](0016-sync-frame.md) | Sync: a per-user change feed, last write wins per row, outbox on the device |
| [ADR-0017](0017-ui-host.md) | Tauri 2 UI host: always resident, pre-warmed panels, preview through the service |
| [ADR-0018](0018-rich-content.md) | Rich snippets are Markdown plus attachments, rendered in the core |
| [ADR-0019](0019-typing-during-injection.md) | Keys typed during an expansion are compensated or the expansion is abandoned |
| [ADR-0020](0020-account-endpoint-and-sign-in.md) | The sync agent serves its own endpoint; sign-in happens in the browser |
| [ADR-0021](0021-packaging-and-permissions.md) | The macOS service is a signed helper app; permissions are checked in the worker |
| [ADR-0022](0022-shared-snippet-trust.md) | Snippets from other people are reviewed before they can press keys or read the clipboard |
| [ADR-0023](0023-updates.md) | The UI host applies updates; the supervisor stops and restarts the worker around them |
| [ADR-0024](0024-engine-protocol-after-phase-1.md) | Engine protocol details settled by the Phase 1 review |
| [ADR-0025](0025-per-app-injection-settings.md) | One settings format for per-app injection overrides |
| [ADR-0026](0026-enter-and-tab-not-default-delimiters.md) | Enter and Tab are not default delimiters |
