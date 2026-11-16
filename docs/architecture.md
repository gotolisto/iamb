# Iamb architecture

Status: draft for approval · Task 0.1 · Decision records: [`docs/decisions/`](decisions/README.md)

Iamb expands abbreviations into snippets as you type. This document fixes the processes, crates,
messages and data model that Phases 1 to 4 build against. Every choice made here has a decision
record (ADR); the record holds the reasoning and the alternatives that were rejected.

## 1. Goals the architecture must meet

| Goal | Target | Where it is decided |
| --- | --- | --- |
| Expansion speed | 95% of plain-text expansions under 50 ms from last keystroke | §4, §5 |
| Typed keys never leave memory | 0 keys written to disk, logs or network | §3, §8 |
| Idle footprint of the service | CPU under 0.1%, memory under 20 MB | §3 |
| A UI crash never stops expansion | Service keeps expanding with no UI process | §3 |
| One engine for desktop and mobile | Same Rust core on five platforms | §2 |

## 2. Components and crates

All Rust crates live in one Cargo workspace under `crates/`, all named `iamb-*`
([ADR-0002](decisions/0002-workspace-and-crate-names.md)). Arrows show "depends on".

```text
                 apps/desktop (Tauri 2, React + TypeScript)
                        │
   iamb-cli        iamb-desktop          iamb-sync           iamb-service
      │                 │                    │              │     │      │
      │                 └────── iamb-ipc ────┴──────────────┘     │      │
      │                            │                              │      │
      └────────► iamb-store ◄──────┼──────────────────────────────┘      │
                     │             │                              iamb-platform
                     └──► iamb-core ◄──────────────────────────────┘  (traits + mock)
                                                                     ├─ iamb-platform-windows
                                                                     └─ iamb-platform-macos
```

| Crate | Kind | Responsibility | May not contain |
| --- | --- | --- | --- |
| `iamb-core` | library | Data model, matcher, template parser and renderer, date math, engine pipeline (key events in, actions out) | I/O, clocks, threads, OS calls, `unsafe` |
| `iamb-store` | library | SQLite schema and migrations, queries, usage statistics, sync outbox, attachments on disk, JSON and CSV import and export | Network code, OS keyboard code |
| `iamb-ipc` | library | Message types, framing, local transport (Unix socket, Windows named pipe), client and server helpers | Business logic |
| `iamb-platform` | library | Traits `KeyListener`, `Injector`, `FocusInspector`, `ClipboardAccess`; shared event types; an in-memory mock used by tests on every OS | OS calls |
| `iamb-platform-windows` | library | Windows implementations of the platform traits | Anything not behind the traits |
| `iamb-platform-macos` | library | macOS implementations of the platform traits | Anything not behind the traits |
| `iamb-service` | binary | The background service: wires listener → engine → injector, owns the store, serves IPC, supervises itself | Network code of any kind |
| `iamb-sync` | binary | Sync agent: account session, push, pull, live updates, media transfer | Access to key events |
| `iamb-cli` | binary `iamb` | Command-line tool: add, list, remove, import, dry-run expand | — |
| `iamb-desktop` | binary | Tauri shell (`apps/desktop/src-tauri`): tray icon, windows, bridges UI commands to `iamb-ipc` | Engine or store logic |
| `iamb-testkit` | library (dev) | Scripted typing sessions, fake clock, fixture libraries, the spec example loader | — |
| `iamb-bench` | benches (dev) | Benchmark harness for the product KPIs | — |
| `xtask` | binary (dev) | Repository automation run by CI (banned-term check) | — |

`iamb-ffi` (bindings for iOS and Android) is added in Phase 6 and depends only on `iamb-core`,
`iamb-store` and the sync library; nothing in Phases 1 to 4 may make that impossible, which is why
`iamb-core` is pure ([ADR-0003](decisions/0003-pure-core.md)).

Rules enforced by CI:

- `unsafe` is forbidden workspace-wide and allowed only in the two `iamb-platform-<os>` crates.
- `iamb-service` must not depend, directly or transitively, on any networking crate. A CI job
  checks its dependency tree against a deny list ([ADR-0008](decisions/0008-no-network-in-service.md)).
- `iamb-core` must build with `--no-default-features` for every target, including mobile targets.

## 3. Processes

Three processes run on the desktop, each with one job
([ADR-0001](decisions/0001-three-processes.md)).

| Process | Lifetime | Sees keystrokes | Has network | Owns |
| --- | --- | --- | --- | --- |
| `iamb-service` | From login, always | Yes | No | Keystroke buffer, engine, store (single writer), IPC server |
| `iamb-desktop` (UI host) | From login, hidden; windows on demand | No | Update checks and opt-in crash reports only | Tray icon, library window, pre-warmed fill-in form and search palette |
| `iamb-sync` | Only while signed in | No | Yes | Account session (tokens in the OS keychain), sync loop |

The service never needs the other two. If the UI host or the sync agent crashes or is not
installed, plain-text expansion continues unchanged.

### 3.1 Service internals

The service uses plain threads and channels, no async runtime
([ADR-0009](decisions/0009-threads-not-async.md)).

| Thread | Does | Blocks on |
| --- | --- | --- |
| Listener | Runs the OS hook or event tap; translates each key to a `KeyEvent`; pushes it to a bounded channel; returns | The OS message loop |
| Engine | Owns the keystroke buffer and the in-memory matcher; turns events into actions; drives the injector | The event channel |
| Store | Owns the SQLite connection; serves reads and writes; publishes a new immutable library snapshot to the engine after each write | A request channel |
| IPC | Accepts local connections; one lightweight thread per client (at most 8 clients) | Sockets |

The listener is observe-only: it never blocks, delays or swallows a key
([ADR-0010](decisions/0010-observe-only-listener.md)). The one exception is the search-palette
hotkey, registered through the OS hotkey API rather than the hook. Work in the hook callback is
bounded: translate, push, return (budget: under 1 ms at the 99th percentile).

The engine reads the library through an immutable snapshot behind an atomically swapped pointer, so
a library edit never makes a keystroke wait.

### 3.2 Supervision and start-up

`iamb-service` runs as two processes from one binary ([ADR-0011](decisions/0011-supervision.md)):

- the supervisor (`iamb-service --supervise`), registered as a per-user login item (a LaunchAgent on
  macOS; a per-user Run entry on Windows). It holds no hooks and no data; it starts the worker and
  restarts it on abnormal exit with backoff (0 s, 1 s, 5 s, then every 30 s);
- the worker, which does everything in §3.1.

Single instance is guaranteed by an exclusive lock on `run/service.lock` taken by the supervisor,
and by the IPC endpoint name. The supervisor also starts the UI host hidden at login when "show
tray icon" is on (the default), and starts `iamb-sync` when an account is signed in.

### 3.3 Files on disk

| Item | macOS | Windows |
| --- | --- | --- |
| Data root | `~/Library/Application Support/Iamb/` | `%APPDATA%\Iamb\` |
| Library | `library.db` (SQLite, WAL mode) | same |
| Attachments | `attachments/<first 2 hex>/<sha256>` | same |
| Logs | `logs/service.log` (rotated, 5 × 1 MB) | same |
| Runtime | `run/service.sock`, `run/service.lock` | `\\.\pipe\iamb-service-<user SID>`, `run\service.lock` |

On Linux (development and the command-line tool only) the data root is `$XDG_DATA_HOME/iamb/`.
Setting `IAMB_DATA_DIR` overrides the data root on every OS; tests always set it.

## 4. The engine pipeline

`iamb-core` exposes one synchronous state machine ([ADR-0003](decisions/0003-pure-core.md),
[ADR-0004](decisions/0004-engine-events-and-actions.md)):

```rust
impl Engine {
    pub fn handle(&mut self, event: Event, env: &dyn Environment) -> Vec<Action>;
}
```

### 4.1 Events in

| Event | Meaning | Effect on the keystroke buffer |
| --- | --- | --- |
| `Char(char)` | A translated character was typed (after layout and dead-key handling) | Appended; then matched |
| `Backspace` | Backspace pressed | Last character removed |
| `Boundary(kind)` | Enter, Tab, Escape, arrow or navigation key, mouse click, a chord with Ctrl, Alt or Cmd | Cleared (Enter, Tab and Space also count as delimiters first) |
| `FocusChanged(app)` | Another window or app took focus | Cleared; per-app exclusion re-evaluated |
| `SecureInput(bool)` | A password field gained or lost focus | Cleared; engine suspended while true |
| `Paused(bool)` | Global pause toggled | Cleared; engine suspended while true |
| `ExpandRequest(id)` | The search palette or CLI asked to expand a snippet | None |
| `FormSubmitted(session, values)` / `FormCancelled(session)` | Result of a fill-in form | None |
| `LibraryChanged(snapshot)` | The store published a new library | Cleared |

The buffer lives only in memory, holds at most 64 characters (the maximum abbreviation length),
and is overwritten with zeros when cleared. While suspended, `Char` events are dropped without
being stored ([ADR-0012](decisions/0012-keystroke-privacy.md)).

### 4.2 Actions out

| Action | Meaning |
| --- | --- |
| `DeleteBack(n)` | Remove the `n` characters of the abbreviation (and delimiter) |
| `TypeText(text)` | Insert plain text |
| `Paste { plain, html, attachments }` | Insert rich content through the clipboard, then restore the clipboard |
| `PressKey { key, shift, times }` | Press a named key |
| `MoveCaretBack(n)` | Move the caret left `n` characters (the `{cursor}` macro) |
| `Wait(ms)` | Pause before the next action |
| `ShowForm(session, spec)` | Ask the UI host to show a fill-in form |
| `Diagnostic(entry)` | Record why something did or did not expand |

How `TypeText` reaches the target app (simulated Unicode keys, or clipboard paste for long text) is
the injector's choice, driven by a per-app strategy table in the platform crates. The engine does
not know.

### 4.3 The environment

Everything the core needs from the outside world comes through one trait, so the core stays pure
and every test is deterministic:

```rust
pub trait Environment {
    fn now(&self) -> LocalDateTime;          // captured once per expansion
    fn locale(&self) -> &Locale;
    fn clipboard_text(&self) -> Option<String>; // called only if the template uses {clipboard}
}
```

### 4.4 Matching

- The matcher is a trie over reversed abbreviations, built once per library snapshot.
- Trigger modes per snippet: `immediate` (fires on the last character of the abbreviation) and
  `delimiter` (fires when a delimiter follows). The delimiter set is a setting; the default is
  space, Enter, Tab and ``. , ; : ! ? ) ] } " '``.
- Case modes per snippet: `exact`, `insensitive`, and `adaptive` (insensitive match; the expansion
  follows the case pattern typed: `abc` → as written, `Abc` → first letter capitalised, `ABC` →
  upper case).
- When several abbreviations end at the caret, the longest wins. Ties cannot occur: the store
  rejects two enabled snippets with the same abbreviation in the same effective library; the UI
  warns earlier.
- Option per snippet `word_start` (default on for delimiter mode, off for immediate mode): the
  character before the abbreviation must be a boundary or the start of the buffer.

### 4.5 Expansion with a fill-in form

1. The engine matches a snippet whose template has fields. It emits `ShowForm` and nothing else.
   The abbreviation stays on screen.
2. The service records the focused window (through `FocusInspector`) and forwards the form to the
   UI host, launching it if needed. If no UI host answers within 2 seconds the expansion is
   abandoned with a `Diagnostic`.
3. On submit, the service restores focus to the recorded window and sends `FormSubmitted` to the
   engine, which emits `DeleteBack` and the rendered actions. On cancel nothing is changed.

Deleting only after submit means a cancelled form has no side effects
([ADR-0013](decisions/0013-form-flow.md)).

### 4.6 Latency budget (plain text, up to 500 characters)

| Step | Budget |
| --- | --- |
| Hook callback | 1 ms |
| Channel hand-off and match | 1 ms (lookup itself under 50 µs) |
| Template render | 1 ms |
| Injection, including app response | 40 ms |
| Reserve | 7 ms |

## 5. Platform layer

`iamb-platform` defines what the service needs from an operating system
([ADR-0005](decisions/0005-platform-traits.md)):

| Trait | Methods (summary) | Windows implementation | macOS implementation |
| --- | --- | --- | --- |
| `KeyListener` | `start(sink)`, `stop()`, `health()` | Low-level keyboard and mouse hooks on a dedicated message-loop thread; layout-aware translation | Event tap; re-enables itself when the system disables it |
| `Injector` | `delete_back`, `type_text`, `paste`, `press_key`, `move_caret` | Synthesised Unicode input; clipboard paste with restore | Synthesised key events; pasteboard paste with restore |
| `FocusInspector` | `focused_app()`, `is_secure_input()`, `remember_focus()`, `restore_focus()` | Foreground window, process name, password-field detection through UI Automation | Frontmost app, secure event input state, Accessibility focus |
| `ClipboardAccess` | `read_text`, `snapshot`, `restore` | Clipboard API | Pasteboard API |
| `Permissions` | `status()`, `open_settings(kind)` | Always granted | Accessibility and Input Monitoring |

Every event the injector synthesises carries a marker (the extra-info field on Windows, the
event-source user data on macOS) and the listener drops marked events, so Iamb never reacts to its
own output.

The mock implementation in `iamb-platform` records injected actions and replays scripted key
streams. All service logic is tested against it on Linux, Windows and macOS; only the thin OS
crates need real machines.

## 6. Messaging between processes

One protocol, three kinds of client ([ADR-0006](decisions/0006-ipc.md)).

- Transport: a Unix domain socket on macOS and Linux, a named pipe on Windows. Both are restricted
  to the current user (socket in a `0700` directory with mode `0600`; pipe DACL limited to the
  user's SID), and the server verifies the peer's user identity on connect.
- Framing: a 4-byte little-endian length, then one UTF-8 JSON document, at most 8 MB.
- Shape: `Request { id, body }` → `Response { id, result | error }`, plus `Event { topic, body }`
  pushed by the service to subscribed clients.
- Versioning: the first message is `Hello { protocol: 1, role, client_version }`. The service
  answers with its own version and refuses a different major protocol version with a clear error.

| Role | Used by | May call |
| --- | --- | --- |
| `ui` | `iamb-desktop` | Library, Engine control, Forms, Diagnostics, Settings, Sync status |
| `cli` | `iamb-cli` | Library, Engine control, dry-run expand |
| `sync` | `iamb-sync` | Sync only: read the outbox, apply remote changes, report status |

Message groups (contract tests in `iamb-ipc` cover every message):

| Group | Requests | Events |
| --- | --- | --- |
| Library | `ListFolders`, `ListSnippets(query)`, `GetSnippet`, `PutSnippet`, `DeleteSnippet`, `PutFolder`, `DeleteFolder`, `PutTag`, `Search(text)`, `Import`, `Export`, `Health` | `LibraryChanged(revision)` |
| Engine control | `GetStatus`, `SetPaused`, `ExpandSnippet(id)`, `DryRun(text)` | `StatusChanged` |
| Forms | `FormSubmit(session, values)`, `FormCancel(session)` | `FormRequested(session, spec)` |
| Diagnostics | `RecentDiagnostics` | `DiagnosticAdded` |
| Settings | `GetSettings`, `PutSetting`, `ListExclusions`, `PutExclusion` | `SettingsChanged` |
| Sync | `OutboxRead(limit)`, `OutboxAck(ids)`, `ApplyRemote(batch)`, `SetSyncStatus` | `OutboxChanged` |

The React UI never speaks IPC itself. It calls Tauri commands in `iamb-desktop`, which forwards
them over `iamb-ipc` and relays events back.

The service is the only process that writes `library.db` while it runs
([ADR-0007](decisions/0007-store.md)). The command-line tool connects to the service when the
endpoint exists and opens the store directly, under the store's own lock, when it does not.

## 7. Data model

Shared by the local store and the server ([ADR-0014](decisions/0014-data-model.md)). Every synced
row has `id` (UUID version 7, created on the device), `created_at`, `updated_at` (UTC
milliseconds), `rev` (assigned by the server; 0 until first sync) and `deleted_at` (soft delete).

```text
User ──< Membership >── Profile ──< Folder ──< Snippet >──< Tag
                           │           │           │
                           │           │           └──< SnippetAttachment >── Attachment
                           └──────< FolderShare >──┘ (folder shared to a profile)
```

| Entity | Fields beyond the common ones | Notes |
| --- | --- | --- |
| `User` | `email`, `display_name` | Server-side; the device stores only the signed-in user |
| `Profile` | `kind` (`personal`, `family`, `company`), `name`, `owner_user_id` | Each user has exactly one `personal` profile, created with the account. Before sign-in the device has a local personal profile that is adopted at first sign-in |
| `Membership` | `profile_id`, `user_id`, `role` (`owner`, `admin`, `member`) | Only for `family` and `company` profiles |
| `Folder` | `profile_id`, `parent_id` (nullable, at most 4 levels), `name`, `position`, `defaults` (trigger mode, case mode, abbreviation prefix) | A folder belongs to exactly one profile |
| `FolderShare` | `folder_id`, `profile_id` (the group it is shared to), `permission` (`view`, `edit`) | Sharing a folder shares its sub-folders. This is the only way content crosses profiles |
| `Snippet` | `folder_id`, `abbreviation`, `label`, `kind` (`plain`, `rich`), `body` (template source), `trigger` (`immediate`, `delimiter`), `case_mode`, `word_start`, `enabled` | `body` follows the [template language spec](template-language.md); rich bodies are Markdown |
| `Tag` | `profile_id`, `name`, `color` | |
| `SnippetTag` | `snippet_id`, `tag_id` | |
| `Attachment` | `sha256`, `mime`, `size`, `file_name` | Content-addressed; bytes live on disk and in object storage, not in the database |
| `SnippetAttachment` | `snippet_id`, `attachment_id` | |

Device-local tables, never synced:

| Table | Purpose |
| --- | --- |
| `usage` | Per snippet: expansion count, last used, characters saved |
| `setting` | Key and value: delimiters, hotkeys, date and time formats, pause state, start at login |
| `app_exclusion` | Apps where Iamb does not expand or buffer |
| `outbox` | Pending local changes for the sync agent, in order |
| `sync_state` | Per profile: last pulled revision |
| `schema_migrations` | Applied migration versions |

Access rules, enforced by the server and mirrored in the UI:

- A user reads and writes everything in their own personal profile.
- A member of a group profile reads the folders shared to that profile, and writes them when the
  share permission is `edit`. Only `owner` and `admin` manage members.
- Only the owner of a folder's profile (or an `admin` of a company profile that owns it) can share
  or unshare it.
- Nothing else is visible. There is no public or link sharing in version 1.

The effective library on a device is every non-deleted, enabled snippet in the user's personal
profile plus every folder shared to a profile they belong to. Abbreviation uniqueness is checked
over that effective library; when a shared folder brings in a conflict, the personal snippet wins
and the conflict is listed by library health.

Usage statistics stay on the device ([ADR-0015](decisions/0015-local-usage-statistics.md)).

## 8. Privacy and security rules

These hold for every phase and are what the Phase 2 privacy review checks
([ADR-0012](decisions/0012-keystroke-privacy.md)).

1. Typed characters exist only in the engine's 64-character buffer, in memory. No API returns the
   buffer. It is never serialised, logged or sent over IPC.
2. Log calls cannot receive typed text: the buffer and `Char` events are held in a `Sensitive<T>`
   wrapper that implements neither `Debug`, `Display` nor `Serialize`, and a lint forbids
   formatting it.
3. Diagnostics name the snippet and the reason ("excluded app", "secure field", "no word start"),
   never what was typed. They are kept in a 200-entry ring in memory.
4. `iamb-service` has no network code, checked in CI. Everything that talks to the network lives
   in `iamb-sync` or `iamb-desktop`, which never receive key events.
5. Account tokens live in the OS keychain (Keychain on macOS, Credential Manager on Windows) and
   are read only by `iamb-sync`.
6. Secure input (password fields, terminal secure entry) and excluded apps suspend the engine
   before a character is stored.
7. Crash reports are written locally by each process without heap contents; only `iamb-desktop`
   uploads them, and only after opt-in.
8. Templates cannot press modifier shortcuts other than Shift (see the template spec), so a shared
   snippet cannot close windows or trigger app commands.

Threat model: Iamb protects typed text from itself (bugs, logs, sync) and from other users on the
machine. It does not defend against malware already running as the same user, which can observe
the keyboard directly.

## 9. Sync

Design details and conflict scenarios belong to task 4.1; the architecture fixes the frame
([ADR-0016](decisions/0016-sync-frame.md)):

- The server is a Postgres database with row-level security, reached over HTTPS and a live-update
  channel, hosted on Supabase.
- Each synced row carries a server-assigned revision. Conflict rule: last write wins per row, with
  the overwritten version kept in a server-side history table.
- The store writes every local change to `outbox` in the same transaction as the change.
  `iamb-sync` pushes the outbox in order, pulls rows with a revision above `sync_state`, and hands
  them to the service with `ApplyRemote`. Offline work accumulates in the outbox.
- Deletes are soft (`deleted_at`) and are purged after 30 days on the server and on devices that
  have pulled past them.
- Attachments are uploaded to object storage by hash; rows sync first and bytes are fetched lazily
  and cached under `attachments/`.

## 10. Desktop UI

[ADR-0017](decisions/0017-ui-host.md) fixes the shape; task 3.1 designs the screens.

- `iamb-desktop` is a Tauri 2 app. It starts hidden at login and owns the tray or menu-bar icon.
- It keeps two small windows created but hidden, the fill-in form and the search palette, so they
  appear within their targets (150 ms and 100 ms). The library window is created on demand and
  destroyed when closed.
- Rich snippets are edited as Markdown and stored as Markdown plus attachments
  ([ADR-0018](decisions/0018-rich-content.md)). Rendering Markdown to HTML for injection happens in
  `iamb-core`, so desktop and mobile produce identical output.

## 11. Where each task lands

| Task | Component |
| --- | --- |
| 1.1 Data model | `iamb-core::model` |
| 1.2 Matcher | `iamb-core::matcher` |
| 1.3 Template parser and renderer | `iamb-core::template` |
| 1.4 Date math | `iamb-core::datetime` |
| 1.5 SQLite store | `iamb-store` |
| 1.6 Engine pipeline | `iamb-core::engine`, sessions in `iamb-testkit` |
| 1.7 Import and export | `iamb-store::transfer` |
| 1.8 Command-line tool | `iamb-cli` |
| 1.9 Review | `iamb-core`, `iamb-store`, `iamb-cli`, `iamb-testkit` |
| 2.1 Windows listener | `iamb-platform-windows::listener`, traits in `iamb-platform` |
| 2.2 Windows injector | `iamb-platform-windows::injector`, `::clipboard` |
| 2.3 macOS listener | `iamb-platform-macos::listener`, `::permissions` |
| 2.4 macOS injector | `iamb-platform-macos::injector`, `::clipboard` |
| 2.5 Secure fields, exclusions, pause | `FocusInspector` in both OS crates; suspension in `iamb-core::engine`; `app_exclusion` in `iamb-store` |
| 2.6 Background service | `iamb-service` (supervisor, worker, logging) |
| 2.7 Compatibility harness | `iamb-testkit::compat`, driven through `iamb-service` |
| 2.8 Privacy review | `iamb-service`, `iamb-platform*`, `iamb-core::engine` |
| 3.1 UX spec and design system | `docs/ux/`, `apps/desktop/src/design/` |
| 3.2 Tauri shell | `iamb-desktop`, `iamb-ipc` |
| 3.3 Library | `apps/desktop/src/library/`, Library messages |
| 3.4 Template editor | `apps/desktop/src/editor/`, `iamb-core::template` compiled to WebAssembly for parsing and preview |
| 3.5 Fill-in form | Form window in `iamb-desktop`; Forms messages; focus restore in the OS crates |
| 3.6 Search palette | Palette window in `iamb-desktop`; `Search` in `iamb-store`; hotkey in the OS crates |
| 3.7 Onboarding | `apps/desktop/src/onboarding/`, `Permissions` trait |
| 3.8 Rich text snippets | `iamb-core::rich` (Markdown to HTML), `Paste` action, attachments in `iamb-store` |
| 3.9 Settings and diagnostics | `apps/desktop/src/settings/`, Settings and Diagnostics messages |
| 3.10 Library health | `iamb-store::health`, `Health` message |
| 3.11 Accessibility and polish | `apps/desktop` |
| 3.12 Review | `apps/desktop`, `iamb-desktop` |
| 4.1 Sync design | `docs/sync.md`, extends §7 and §9 |
| 4.2 Server schema and policies | `server/migrations/`, `server/tests/` |
| 4.3 Security review | `server/` |
| 4.4 Sync client | `iamb-sync`, `outbox` and `sync_state` in `iamb-store`, Sync messages |
| 4.5 Account UI | `apps/desktop/src/account/`, commands in `iamb-desktop` that talk to `iamb-sync` |
| 4.6 Media sync | `iamb-sync::media`, attachments in `iamb-store` |
| 4.7 Billing readiness | `server/migrations/` (plan and entitlement tables), feature flags read by `iamb-sync` |

The account UI reaches `iamb-sync` through a second local endpoint (`run/sync.sock`, or
`\\.\pipe\iamb-sync-<user SID>`) with the same framing, carrying only `SignIn`, `SignOut`,
`AccountStatus`, `Profiles`, `Invite` and `SetFolderShare`. Credentials therefore never pass
through the service.
