# Iamb architecture

Status: approved by the owner on 2026-11-18; §4 amended the same day after the Phase 1 review (ADR-0024) · Task 0.1 ·
Decision records: [`docs/decisions/`](decisions/README.md) ·
Review record: [`docs/reviews/0.1-architecture-review.md`](reviews/0.1-architecture-review.md)

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
([ADR-0002](decisions/0002-workspace-and-crate-names.md)).

| Crate | Kind | Depends on | Responsibility | May not contain |
| --- | --- | --- | --- | --- |
| `iamb-core` | library | — | Data model, matcher, template parser and renderer, date math, Markdown rendering, fuzzy search, engine pipeline (events in, actions out) | I/O, clocks, threads, OS calls, `unsafe` |
| `iamb-store` | library | core | SQLite schema and migrations, queries, usage statistics, sync outbox, attachment index, JSON and CSV import and export, library health | Network code, OS keyboard code |
| `iamb-ipc` | library | core, platform (+os for the native endpoint) | Message types, framing, blocking local transport (Unix socket, Windows named pipe), peer verification, client and server helpers | Business logic, async code, network code |
| `iamb-platform` | library | core | Traits `KeyListener`, `Injector`, `FocusInspector`, `ClipboardAccess`, `Hotkeys`, `Permissions`, `LoginItem`, `MemoryLock`, `LocalEndpoint`; the listener's event type, injection steps, markers and the typed-characters hand-off; an in-memory mock used by tests on every OS | OS calls |
| `iamb-platform-windows` | library | platform | Windows implementations of the platform traits | Anything not behind the traits |
| `iamb-platform-macos` | library | platform | macOS implementations of the platform traits | Anything not behind the traits |
| `iamb-service` | binary | core, store, ipc, platform(+os) | The background service: listener → engine → injector, owns the store, serves IPC, supervises | Network code of any kind |
| `iamb-sync` | library + thin binary | core, ipc | Sync agent: account session, push, pull, live updates, media transfer, account endpoint | Access to key events |
| `iamb-cli` | binary `iamb` | core, store, ipc, platform | Command-line tool: add, list, remove, import, dry-run expand | — |
| `iamb-desktop` | binary | ipc, platform(+os) | Tauri shell (`apps/desktop/src-tauri`): tray icon, windows, focus hand-back, bridges UI commands to `iamb-ipc` | Engine or store logic |
| `iamb-testkit` | library (dev) | core, platform | Scripted typing sessions, fake clock, fixture libraries, the spec example loader, compatibility harness | — |
| `iamb-bench` | benches (dev) | core, store | Benchmark harness for the product KPIs | — |
| `xtask` | binary (dev) | — | Repository automation run by CI | — |

`iamb-ffi` (bindings for iOS and Android) is added in Phase 6 and depends on `iamb-core`,
`iamb-store` and the `iamb-sync` library. Nothing in Phases 1 to 4 may make that impossible, which
is why `iamb-core` is pure ([ADR-0003](decisions/0003-pure-core.md)) and `iamb-sync` is a library
with a thin binary. On mobile there is no service process: the app and the keyboard extension open
the store directly under the store lock (§6.3).

Rules enforced by CI:

- `unsafe` is forbidden workspace-wide and allowed only in the two `iamb-platform-<os>` crates.
- `iamb-service` contains no network code ([ADR-0008](decisions/0008-no-network-in-service.md)).
  Four checks together enforce it: (1) its resolved dependency tree, per target, is compared with
  a deny list of networking crates; (2) the enabled features of the `windows` bindings are compared
  with an allow list that excludes the networking namespaces; (3) a lint forbids `std::net` in
  `iamb-service`, `iamb-ipc`, `iamb-store` and the platform crates; (4) the release binary is
  built with `-p iamb-service` alone, so feature unification with other workspace members cannot
  add code to it.
- `iamb-core` builds for every desktop and mobile target with default features off.

## 3. Processes

Three kinds of process run on the desktop, each with one job
([ADR-0001](decisions/0001-three-processes.md)).

| Process | Lifetime | Sees keystrokes | Has network | Owns |
| --- | --- | --- | --- | --- |
| `iamb-service` (supervisor + worker) | From login, always | Worker: yes | No | Keystroke buffer, engine, store (single writer), service endpoint |
| `iamb-desktop` (UI host) | From login, hidden; windows on demand | No | Update checks and opt-in crash reports only | Tray icon, library window, pre-warmed fill-in form and search palette, focus hand-back |
| `iamb-sync` | From login; idle with no connections while signed out | No | Yes | Account session (tokens in the OS keychain), sync loop, account endpoint |

The service never needs the other two. If the UI host or the sync agent crashes or is not
installed, plain-text expansion continues unchanged.

### 3.1 Service worker internals

Plain threads and bounded channels, no async runtime
([ADR-0009](decisions/0009-threads-not-async.md)).

| Thread | Does |
| --- | --- |
| Listener | Runs the OS hook or event tap and the hotkey registration; translates each key; stamps it with a sequence number; pushes it to the engine channel; returns |
| Engine | Owns the keystroke buffer and the current snapshot; turns events into actions; sends a heartbeat to the supervisor every 500 ms |
| Injector | Executes action lists against the OS; reports start and end of each injection |
| Focus | Watches the focused app and window; answers "is this a secure field?" with a 100 ms timeout |
| Store | Owns the SQLite connection; performs every write; builds a new immutable snapshot after a change to library content or engine settings |
| Readers | One per IPC client (at most 8). Read requests, search and dry runs are answered from the snapshot without touching the store thread |

The snapshot is immutable and shared through an atomically swapped pointer. It holds the matchable
set, the reference set (§4.4), the search index and the engine settings (delimiters, date and time
formats, locale, excluded apps). Usage-statistics writes and sync bookkeeping do not produce a new
snapshot.

On macOS the worker's main thread runs the application event loop, because the listener's
keyboard-layout reads and the hotkey registrations must happen on the main thread and hotkey
presses are dispatched by that loop; the threads above run beside it.

The listener is observe-only: it never blocks, delays or swallows a key
([ADR-0010](decisions/0010-observe-only-listener.md)). Work in the hook callback is bounded:
translate, push, return (under 1 ms at the 99th percentile). If the channel is full the listener
drops the event and sets an overflow flag; the next event it delivers is `Overflow`, which clears
the buffer.

### 3.2 Supervision and start-up

`iamb-service` runs as two processes from one binary ([ADR-0011](decisions/0011-supervision.md)):

- the supervisor (`iamb-service --supervise`), the per-user login item. It holds no hooks and no
  user data. It starts the worker, the UI host and the sync agent, and restarts each on abnormal
  exit with backoff (0 s, 1 s, 5 s, then every 30 s);
- the worker, which does everything in §3.1.

The supervisor also restarts a worker that is alive but stuck: it kills and restarts the worker
when no heartbeat arrives for 3 seconds. The heartbeat comes from the engine thread, so a
deadlocked engine is caught. Every 30 seconds the worker posts a marked no-op key event and checks
that its own listener sees it within 1 second; on failure it reinstalls the hook or tap, and after
two failures it exits for a restart. This catches a hook the operating system removed silently.

Single instance is guaranteed by an exclusive lock on `run/supervisor.lock`.

Packaging ([ADR-0021](decisions/0021-packaging-and-permissions.md)): on macOS the service is a
signed helper app with a stable bundle identifier inside `Iamb.app`, registered as a login item
through the system's login-item API by the main app, so that Accessibility and Input Monitoring
grants belong to the helper and survive updates. On Windows the supervisor is registered under the
per-user Run key. Updates are applied by the UI host, which asks the supervisor to stop the worker,
replaces the files, and asks it to start again ([ADR-0023](decisions/0023-updates.md)).

### 3.3 Files on disk

| Item | macOS | Windows |
| --- | --- | --- |
| Data root | `~/Library/Application Support/Iamb/` | `%APPDATA%\Iamb\` |
| Library | `library.db` (SQLite, WAL mode) | same |
| Attachments | `attachments/<profile id>/<sha256>` | same |
| Logs | `logs/<process>.log` (rotated, 5 × 1 MB) | same |
| Locks | `run/supervisor.lock`, `run/store.lock` | same |
| Endpoints | `run/service.sock`, `run/sync.sock` | `\\.\pipe\iamb-service-<user SID>`, `\\.\pipe\iamb-sync-<user SID>` |

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
| `Char { ch, seq }` | A key produced text (after layout and dead-key handling), whatever modifiers were held | Appended; then matched |
| `Backspace { seq }` | Backspace pressed | Last character removed |
| `Boundary { kind, seq }` | A key or click that produced no text: Enter, Tab, Escape, arrows, navigation keys, mouse click, a shortcut chord | Cleared (Enter and Tab also count as delimiters first) |
| `FocusChanged(app)` | Another window or app took focus | Cleared; engine suspended until `SecureState` arrives |
| `SecureState(state)` | The focus thread's verdict: `normal`, `secure`, `excluded`, `own_window` or `unknown` | Cleared; engine suspended unless `normal` |
| `Composing(bool)` | An input method started or finished composing text | Cleared; engine suspended while true |
| `Paused(bool)` | Global pause toggled | Cleared; engine suspended while true |
| `Overflow` | The listener dropped events | Cleared |
| `InjectionStarted { expansion, seq }` / `InjectionFinished { expansion, seq }` | The injector's own start and end markers for one expansion, in listener order | See §4.6 |
| `InjectionCancelled { expansion }` | The injector gave up on an expansion before or during its output | Cleared; see §4.6 |
| `ExpandRequest { snippet }` | The search palette or CLI asked to expand a snippet (the service keeps the focus token) | None; see §4.5 for when it is refused |
| `FormSubmitted { session, values }` / `FormCancelled { session }` | Result of a fill-in form | None |
| `ReopenLastForm` | Reopen the most recently submitted form, pre-filled | None |
| `SnapshotChanged(snapshot)` | New library content or engine settings | Kept; matching continues against the new snapshot |

Rules for the listener that the events rely on:

- A key is a `Char` whenever translation yields text. AltGr combinations on Windows and Option
  combinations on macOS are therefore characters, not boundaries.
- Translation never disturbs the target application's keyboard state. The Windows listener
  translates with the foreground thread's layout in the mode that leaves kernel dead-key state
  untouched and tracks dead keys itself; the macOS listener does the equivalent.
- Input-method composition is reported as `Composing`. Version 1 does not expand while an input
  method is composing.

The buffer lives only in memory, holds at most 66 characters (the 64-character maximum abbreviation,
the character before it for the word-start rule, and one delimiter), is held in a page locked
against swapping (the engine keeps the buffer in its own heap allocation and exposes that
allocation's address range, `Engine::buffer_region()`, so the service can lock it), and is
overwritten with zeros when cleared. While the
engine is suspended, `Char` events are dropped without being stored
([ADR-0012](decisions/0012-keystroke-privacy.md)). After a focus change the engine stays suspended
until the focus thread reports; `unknown` (timeout, or a window Iamb may not inspect, such as an
elevated one) keeps it suspended. Iamb fails closed.

### 4.2 Actions out

| Action | Meaning |
| --- | --- |
| `DeleteBack(n)` | Press Backspace `n` times: one per `Char` event consumed by the abbreviation and delimiter |
| `TypeText { text, verbatim }` | Insert plain text. `verbatim` marks ranges that came from field values or the clipboard |
| `Paste { plain, markdown_html, attachments }` | Insert rich content through the clipboard, then restore the clipboard (emitted from task 3.8; until then rich snippets expand as plain templates) |
| `PressKey { key, shift, times }` | Press a named key |
| `MoveCaretBack(n)` | Press Left `n` times: one per extended grapheme cluster after the `{cursor}` position |
| `Wait(ms)` | Pause before the next action |
| `ShowForm { session, snippet, spec, last_values }` | Ask the UI host to show a fill-in form; the service adds the focus token |
| `Expanded { expansion, snippet, chars_saved }` | Always the last action of an expansion. Opens the expansion's injection protocol (§4.6); the service records usage when the injection finishes |
| `Diagnostic(entry)` | Record why something did or did not expand |

How `TypeText` reaches the target app (simulated Unicode keys, or clipboard paste for long text),
and any per-app adjustment of how many presses a delete or caret move needs, is the injector's
choice, driven by a per-app strategy table in the platform crates with user overrides stored in
settings. The engine does not know.

### 4.3 The environment

Everything the core needs from outside comes through one trait, so the core stays pure and every
test is deterministic. Settings are not here; they are part of the snapshot.

```rust
pub trait Environment {
    fn now(&self) -> LocalDateTime;             // read once, at the match
    fn clipboard_text(&self) -> Option<String>; // read once, at the match, only if needed
}
```

### 4.4 Matching

- Two sets are built per snapshot. The **reference set** is every non-deleted snippet in the
  effective library (§7); `{snippet}` lookups use it. The **matchable set** is the enabled subset
  whose templates parse; typing matches against it. A snippet whose template parses but fails to
  resolve (a missing or looping `{snippet}`, a field conflict) stays matchable: typing it deletes
  nothing and records a `Diagnostic` with the error code, so a shorter abbreviation cannot fire in
  its place.
- Abbreviations are unique in the reference set under case-folded comparison. The store enforces
  this inside a profile. Across profiles (shared folders) a deterministic precedence decides which
  snippet holds an abbreviation: the user's personal profile first, then the earliest-created
  share, then the lower folder id. The losers neither trigger nor resolve, and library health
  lists them.
- Case folding, used by the store's uniqueness check and by the matcher alike, is per-character
  lower-casing with final sigma folded to σ (`Abbreviation::fold_key`). There is no wider Unicode
  case folding: `ß` does not match `ss`.
- The matcher is a trie over reversed, case-folded abbreviations.
- Trigger modes per snippet: `immediate` (fires on the last character) and `delimiter` (fires when
  a delimiter follows). The delimiter set is a setting; the default is space, Enter, Tab and
  ``. , ; : ! ? ) ] } " '``. In delimiter mode the delimiter is deleted with the abbreviation and
  typed again after the expansion, except when the snippet has a `{cursor}`.
- Case modes per snippet: `exact`, `insensitive`, and `adaptive` (insensitive match; the expansion
  follows the case pattern typed: `abc` → as written, `Abc` → first letter capitalised, `ABC` →
  upper case). Adaptive case changes never touch `verbatim` ranges.
- When several abbreviations end at the caret, the longest wins. An `immediate` abbreviation that
  is a suffix-start of a longer one fires first and so hides it (`ad` hides `add`); the editor
  warns and library health reports this as shadowing.
- Option per snippet `word_start` (default on for delimiter mode, off for immediate mode): the
  character before the abbreviation must be a boundary or the start of the buffer.
- A folder's abbreviation prefix is a convenience of the editor: it is put in front of the
  abbreviation when a snippet is created, and the stored abbreviation is always complete.

### 4.5 Fill-in forms, the palette and focus

The process that holds the focus is the one that gives it back
([ADR-0013](decisions/0013-form-flow.md)).

1. The engine matches a snippet whose template has fields. It reads the clock and, if needed, the
   clipboard, stores them in a form session, and emits `ShowForm`. The abbreviation stays on
   screen. Only one form session exists at a time; a second trigger while one is open is dropped
   with a `Diagnostic`.
2. The service attaches a focus token for the target window (from `FocusInspector`) and sends
   `FormRequested` to the UI host. If no UI host is connected the expansion is abandoned with a
   `Diagnostic` and the supervisor is asked to start one; if a connected host does not acknowledge
   within 2 seconds the expansion is abandoned.
3. The UI host shows the pre-warmed form. On macOS the form and the palette are non-activating
   panels, so the target app stays the active app. On Windows the service grants the UI host the
   right to take the foreground when it sends the request.
4. On submit the UI host hides the form, returns focus to the token's window, confirms that the
   window is focused, and only then sends `FormSubmit`. The engine emits `DeleteBack` and the
   rendered actions. On cancel the UI host returns focus and nothing else changes.

The search palette uses the same mechanism: the hotkey reaches the listener thread, the service
captures a focus token and sends `PaletteRequested { focus }`; choosing a snippet returns focus and
then sends `ExpandSnippet { id, focus }`.

Explicit requests (a palette pick, a form submit, `ReopenLastForm`) never type into a field Iamb
has not confirmed as normal (owner decision, 2026-11-18). After a focus change the request waits
for the focus thread's verdict: `normal` runs it; `secure`, `excluded`, `own_window` or `unknown`
refuses it with a `Diagnostic`, and a refused form submit keeps its values for `ReopenLastForm`. A
request made when the state is already known and not normal, or while an input method is
composing, is refused at once. Pause is different: it blocks typed abbreviations but allows a
deliberate palette pick or form submit in a normal field. A request belongs to one
application (for a form submit, the one where the form was opened; for a palette pick or reopen,
the one focused when it was made): a focus change to another application drops it while it
waits, and a request that arrives while another application has the focus is dropped at once. A newer request replaces a waiting one, and drops it even when the newer
one is refused; cancelling a form drops its waiting submit; `Overflow` drops any waiting request.

The engine sees focus events and requests in the order the service delivers them, and the two
arrive from different threads. The service therefore forwards an explicit request only after it
has delivered the focus change for the window the request will type into (the focus token's
window, once the UI host has handed focus back).

Keys typed into Iamb's own windows are not expanded: the focus thread reports `own_window` for the
UI host's process. The one exception is the onboarding test field, which the UI host declares with
`SetTestField(window)`.

The last submitted values of each form are kept so that a form can be reopened pre-filled
(`ReopenLastForm`). They are user-entered text and are treated like typed keys: held only in the
worker's memory, never written to disk or logs, and cleared after 30 minutes without use, on
screen lock, on sign-out and on restart.

### 4.6 Typing while Iamb is injecting

Because the listener only observes, a fast typist's next keys can reach the application before
Iamb has finished ([ADR-0019](decisions/0019-typing-during-injection.md)).

- Injection runs on its own thread, so the engine keeps receiving events. The injector posts a
  marked start event and a marked end event around its output; the listener reports them as
  `InjectionStarted` and `InjectionFinished` in the true order of the key stream.
- Real keys that arrive after the trigger and before `InjectionStarted` are already in the
  application, after the abbreviation. Immediately before it sends anything, the injector reads
  how many there are (`k`) and deletes `n + k` instead of `n`, types the expansion, then types the
  `k` characters again. A boundary key among them cancels the expansion with a `Diagnostic`.
- Where the OS can deliver a batch of synthetic input without interleaving (Windows), the delete
  and the text are sent as one batch. Otherwise real keys that arrive between the two markers
  cannot be repaired: they stay where the application put them, the buffer is cleared, and a
  `Diagnostic` is recorded.
- The rate of such interleaved expansions is measured in the 10,000-expansion reliability run at
  realistic typing speeds and counts against the 99.9% target.

The protocol between the engine and the injector, per expansion:

| Engine state | Entered on | Real keys arriving | Leaves on |
| --- | --- | --- | --- |
| Idle | — | Normal matching | `Expanded` action → Pending |
| Pending | `Expanded { expansion }` | Stored but not matched; a boundary, a clearing event or a Backspace past the trigger marks the expansion disturbed (the injector cancels in the same cases) | `InjectionStarted` → Injecting (the buffer now holds the keys typed since the trigger, as they appear after the expansion); `InjectionCancelled` → Idle, buffer cleared |
| Injecting | `InjectionStarted` | The first real key clears the buffer and records `TypedDuringInjection`; later ones in the window are dropped | `InjectionFinished` or `InjectionCancelled` → Idle |

Every `Expanded` must be answered by `InjectionStarted` then `InjectionFinished`, or by
`InjectionCancelled`, even for an empty expansion. Only one expansion is in flight; a palette pick
or form submit during one is refused with `InjectionBusy`. `Overflow` ends the flight, since the
markers may have been dropped; a late `InjectionFinished` or `InjectionCancelled` for an expansion
forgotten this way is ignored silently.

The injector needs the characters typed after the trigger (`k` of them) to type them again. The
engine never hands typed text out, so the thread that feeds listener events to the engine keeps
them instead (`iamb_platform::TypedHandoff`). It is armed when the engine emits `Expanded`, copies
the real `Char`s it forwards from then until the start marker into a separate `Sensitive` buffer
of at most 64 characters in one fixed, lockable allocation, and is zeroed when the expansion ends.
Only the injector thread reads it, after it has seen its own start marker. The hook or tap thread
itself never copies keys: it cannot know a trigger happened until the engine has matched, so a
copy there would have to hold every key all the time, including keys the engine drops in secure
fields. A boundary, an over-deleting Backspace, a 65th character, composition, overflow or a focus
change cancels the expansion. So does a dead key still waiting when the start marker arrives: the
listener reports it as a boundary just before `InjectionStarted`, because the target would spend
the first Backspace on it. The 2.8 privacy review checks this.

### 4.7 Latency budget (plain text, up to 500 characters)

| Step | Budget |
| --- | --- |
| Hook callback | 1 ms |
| Channel hand-off and match | 1 ms (lookup itself under 50 µs) |
| Template render | 1 ms |
| Injection, including app response | 40 ms |
| Reserve | 7 ms |

## 5. Platform layer

`iamb-platform` defines what Iamb needs from an operating system
([ADR-0005](decisions/0005-platform-traits.md)):

| Trait | Methods (summary) | Used by | Windows | macOS |
| --- | --- | --- | --- | --- |
| `KeyListener` | `start(sink)`, `stop()`, `self_test(timeout)` | service | Low-level keyboard and mouse hooks on a message-loop thread; stateless layout-aware translation | Event tap; re-enables itself when the system disables it |
| `Injector` | `inject(request, handoff)`: one call per expansion, which posts the start marker, applies the compensation of §4.6 and the steps, and posts the end marker; `configure(settings_json)`: the user's per-app settings (default: accepts only the empty string) | service | Synthesised Unicode input in atomic batches; clipboard paste with restore; per-app strategy table keyed by executable name, settings add `paste_keys` and `batch_delay_ms` | Synthesised key events; pasteboard paste with restore; per-app strategy table keyed by bundle identifier, settings add `chunk_units` and more line breaks |
| `FocusInspector` | `focused_app()`, `secure_state(target, timeout)`, `focus_token()`, `restore_focus(token)`, `allow_foreground(pid)`, `watch(sink)`, `unwatch()` | service, UI host | Foreground window, its process and integrity level; window-event hooks for focus changes; password fields by the edit control's style, and through UI Automation `IsPassword` on a dedicated thread | Frontmost app and its front window; activation notifications and an Accessibility observer for focus changes; secure fields by the session's secure-input state, and by the focused element's role and subrole on a dedicated thread |
| `ClipboardAccess` | `read_text`, `snapshot`, `write(content, transient)`, `restore` | service | Clipboard API, with the formats that keep content out of clipboard history | Pasteboard API, with the transient and concealed markers |
| `Hotkeys` | `register(id, chord)`, `unregister(id)` | service | OS hotkey API on the listener thread | OS hotkey API on the listener's run loop |
| `Permissions` | `permission_status()`, `open_settings(kind)` | service | Always granted | Accessibility and Input Monitoring, checked in the worker, which is the process that holds them |
| `LoginItem` | `enable()`, `disable()`, `login_item_status()` | UI host | Per-user Run key | Login-item registration of the helper app |
| `MemoryLock` | `lock(region)`, `unlock(region)` | service | Keeps the keystroke buffer and hand-off out of the page file | Same, out of swap |
| `LocalEndpoint` | `bind(name, policy)`, `connect(name, policy, timeout)`; streams carry the verified peer | service, UI host, CLI, sync agent | Named pipe, first instance only, remote clients rejected, user SID checked | Unix socket in a user-only directory, peer uid checked |

All traits are object safe and take `&self`, so the service holds one `Arc<dyn Platform>`. The
OS crates are the only crates allowed `unsafe`, and every `unsafe` block carries a safety comment
(lint-enforced).

Every event the injector synthesises carries a marker (the extra-info field on Windows, the
event-source user data on macOS) and the listener drops marked events, apart from reporting the
injection markers, so Iamb never reacts to its own output.

Injection into a window running at a higher privilege level than Iamb is refused by Windows; the
focus thread detects such windows, reports `unknown`, and a `Diagnostic` says why.

The user's per-app injection settings are one JSON string in the `injection_overrides` setting,
in one format on both systems ([ADR-0025](decisions/0025-per-app-injection-settings.md)): shared
fields (`method`, `paste_threshold`, `line_break`, `key_delay_ms`, `paste_wait_ms`) plus a few
that only one system has. The format is parsed in one place, `iamb_platform::injection_settings`,
which both OS crates call; unknown fields, out-of-range values and the other system's fields or
line breaks are errors. The service hands the setting to `Injector::configure` at start and
whenever it changes. A value the injector refuses is answered to the client that wrote it with
an `invalid` error and is not stored; one already in the store (written while the service was
stopped) is logged, without its text. Either way the previous settings stay in force.

The mock implementation in `iamb-platform` records injected actions and the settings strings it
was configured with, and replays scripted key streams, including keys interleaved with
injection. All service logic is tested against it on
Linux, Windows and macOS; only the thin OS crates need real machines.

## 6. Messaging between processes

### 6.1 Transport

One transport and framing for both endpoints ([ADR-0006](decisions/0006-ipc.md)).

- A Unix domain socket on macOS and Linux, a named pipe on Windows.
- Server side: the socket lives in a directory owned by the user with mode `0700`; a pipe is
  created as the first instance of its name, restricted to the user's SID, and rejects remote
  clients. A server that cannot create the first instance refuses to start and reports that the
  name is taken.
- Client side: before sending anything, a client verifies that the server process runs as the same
  user and, in release builds, that its executable is the installed Iamb binary with a valid
  signature. Servers verify clients the same way.
- Framing: a 4-byte little-endian length, then one UTF-8 JSON document, at most 8 MB. Bulk data
  (import, export, attachments) is passed as file paths, never inline.
- Shape: `Request { id, body }` → `Response { id, result | error }`, plus `Event { topic, body }`
  pushed by the server.
- The first message is `Hello { protocol: 1, role, client_version }`; a different major protocol
  version is refused with a clear error.
- Message bodies are never logged. Logs record the message name, size and duration.

### 6.2 The service endpoint

| Role | Used by | May call |
| --- | --- | --- |
| `ui` | `iamb-desktop` | Library, Engine control, Forms, Templates, Diagnostics, Settings |
| `cli` | `iamb-cli` | Library, Engine control |
| `sync` | `iamb-sync` | Sync only |

| Group | Requests | Events |
| --- | --- | --- |
| Library | `ListFolders`, `ListSnippets(query)`, `GetSnippet`, `PutSnippet`, `DeleteSnippet`, `PutFolder`, `DeleteFolder`, `PutTag`, `Search(text)`, `Import(path, format)`, `Export(path, format)`, `Health`, `ListPendingReviews`, `ApproveSnippet(id, rev)` | `LibraryChanged(revision)` |
| Engine control | `GetStatus`, `SetPaused`, `ExpandSnippet(id, focus)`, `DryRun(typed)`, `GetPermissions`, `OpenPermissionSettings(kind)`, `SetTestField(window)` | `StatusChanged`, `PaletteRequested(focus)` |
| Forms | `FormAck(session)`, `FormSubmit(session, values)`, `FormCancel(session)`, `ReopenLastForm` | `FormRequested(session, spec, focus, last_values)` |
| Templates | `ParseTemplate(source)`, `RenderPreview(source, values)` | — |
| Diagnostics | `RecentDiagnostics` | `DiagnosticAdded` |
| Settings | `GetSettings`, `PutSetting`, `ListExclusions`, `PutExclusion` | `SettingsChanged` |
| Sync | `OutboxRead(limit)`, `OutboxAck(ids)`, `ApplyRemote(batch)`, `PurgeFolder(id)`, `AttachmentAvailable(profile, sha256)`, `MissingAttachments`, `SetSyncStatus`, `SetEntitlements` | `OutboxChanged` |

Names across the layers: the engine action `ShowForm` becomes the event `FormRequested`; the
requests `FormSubmit` and `FormCancel` become the engine events `FormSubmitted` and
`FormCancelled`. The engine event `SnapshotChanged` is internal to the worker; `LibraryChanged` is
what clients hear.

`DryRun(typed)` feeds a string to a fresh engine as if each character were typed and returns the
actions; nothing is injected. `ParseTemplate` returns the token list the editor turns into chips;
`RenderPreview` returns the rendered output or the error code and position. The editor therefore
uses the same parser as expansion and always sees the whole library, with no second build of the
core ([ADR-0017](decisions/0017-ui-host.md)).

The React UI never speaks IPC itself. It calls Tauri commands in `iamb-desktop`, which forwards
them and relays events back.

### 6.3 Who may open the store

The worker is the only process that writes `library.db` while it runs
([ADR-0007](decisions/0007-store.md)). Whoever opens the database for writing first takes an
exclusive OS lock on `run/store.lock` and holds it while the database is open. The worker waits up
to 5 seconds for it at start. The command-line tool first tries the service endpoint; if nothing
answers it tries the lock for up to 3 seconds and opens the store directly; if it cannot get the
lock it stops with "the service is starting, try again". The same lock rule applies on mobile
between the app and the keyboard extension.

### 6.4 The account endpoint

`iamb-sync` serves a second endpoint with the same transport, for the UI host only
([ADR-0020](decisions/0020-account-endpoint-and-sign-in.md)):

| Requests | Events |
| --- | --- |
| `AccountStatus`, `BeginSignIn(provider)`, `CancelSignIn`, `SignOut`, `Profiles`, `Members(profile)`, `Invite(profile, email, role)`, `Invitations`, `AcceptInvitation(id)`, `RemoveMember`, `SetFolderShare(folder, profile, permission)`, `RemoveFolderShare`, `Conflicts`, `Entitlements` | `AccountChanged`, `SyncProgress`, `ConflictRecorded` |

Sign-in runs in the system browser with the authorisation-code flow with PKCE; the browser returns
to a loopback address that `iamb-sync` listens on for the duration of the sign-in. Email magic
links and third-party identity providers both arrive this way. No password, code or token passes
through the UI host or the service. Invitations are accepted inside the app: after sign-in the
invitee's pending invitations are listed by `Invitations`; the email only tells them to open Iamb.

## 7. Data model

Shared by the local store and the server ([ADR-0014](decisions/0014-data-model.md)). Every synced
row has `id` (UUID version 7, created on the device), `created_at`, `updated_at` (UTC
milliseconds), `updated_by` (user id), `rev` (assigned by the server; 0 until first sync) and
`deleted_at` (soft delete).

```text
User ──< Membership >── Profile ──< Folder ──< Snippet >──< Tag
                           │           │           │
                           │           │           └──< SnippetAttachment >── Attachment
                           └──────< FolderShare >──┘ (folder shared to a group profile)
```

| Entity | Fields beyond the common ones | Notes |
| --- | --- | --- |
| `User` | `email`, `display_name` | Server-side; the device stores only the signed-in user |
| `Profile` | `kind` (`personal`, `family`, `company`), `name`, `owner_user_id` | Each user has exactly one `personal` profile. Before sign-in the device has a local personal profile that is adopted at first sign-in |
| `Membership` | `profile_id`, `user_id`, `role` (`owner`, `admin`, `member`) | Group profiles only. The rules are the same for `family` and `company` |
| `Folder` | `profile_id`, `parent_id` (nullable, at most 4 levels), `name`, `position`, `member_permission` (`view`, `edit`; group-owned folders only), `defaults` (trigger mode, case mode, abbreviation prefix) | A folder belongs to exactly one profile, personal or group |
| `FolderShare` | `folder_id`, `profile_id` (the group it is shared to), `permission` (`view`, `edit`) | Shares a personally owned folder, with its sub-folders, to a group |
| `Snippet` | `folder_id`, `abbreviation`, `label`, `kind` (`plain`, `rich`), `body` (template source), `trigger`, `case_mode`, `word_start`, `enabled` | `body` follows the [template language spec](template-language.md); rich bodies are Markdown |
| `Tag` | `profile_id`, `name`, `color` | Belongs to the profile that owns the folder; visible to everyone who can see a snippet that carries it |
| `SnippetTag` | `snippet_id`, `tag_id` | |
| `Attachment` | `profile_id`, `sha256`, `mime`, `size`, `file_name` | Bytes live on disk and in object storage under `<profile id>/<sha256>`, so storage rules can follow profile and share rules |
| `SnippetAttachment` | `snippet_id`, `attachment_id` | |

Device-local tables, never synced:

| Table | Purpose |
| --- | --- |
| `usage` | Per snippet: expansion count, last used, characters saved |
| `setting` | Delimiters, hotkeys, date and time formats, locale, pause state, start at login, per-app injection overrides (`injection_overrides`, ADR-0025) |
| `app_exclusion` | Apps where Iamb does not expand or buffer |
| `approval` | Per snippet received from someone else: the revision the user approved (§8) |
| `outbox` | Pending local changes for the sync agent, in order, each with the revision it was based on |
| `sync_state` | The user's position in the server's change feed |
| `entitlement` | The plan features last reported by the sync agent |
| `schema_migrations` | Applied migration versions |

Who can do what:

| Content | Read | Create, edit and delete snippets and sub-folders inside | Rename, move, delete, share or unshare the folder itself |
| --- | --- | --- | --- |
| Folder in my personal profile | Me | Me | Me |
| Personal folder shared to a group I belong to | Every member of the group | Members, when the share permission is `edit` | Its owner only |
| Folder owned by a group | Every member | `owner` and `admin`; members when `member_permission` is `edit` | `owner` and `admin` |

- `owner` and `admin` of a group manage its members and invitations.
- A member with `edit` cannot move a snippet out of a shared folder; they can copy it.
- A shared sub-folder whose parent the recipient cannot see is shown as a top-level folder.
- Nothing else is visible. There is no public or link sharing in version 1.

The effective library on a device is every non-deleted snippet the user can read under these
rules. Usage statistics stay on the device
([ADR-0015](decisions/0015-local-usage-statistics.md)).

## 8. Privacy and security rules

These hold for every phase and are what the Phase 2 privacy review checks
([ADR-0012](decisions/0012-keystroke-privacy.md),
[ADR-0022](decisions/0022-shared-snippet-trust.md)).

1. Typed characters exist in exactly two places, both in locked memory: the engine's
   66-character buffer, and, only while an expansion is in flight, the engine-feeding thread's
   copy of the characters typed after its trigger (at most 64, for the injector to type again,
   §4.6), zeroed when the expansion ends. No API returns either. They are never serialised, logged or sent over
   IPC.
2. User-entered values (form values, clipboard content read for `{clipboard}`, palette queries,
   dry-run input) are never written to disk or logs. Form values are remembered only as described
   in §4.5.
3. Both classes are held in a `Sensitive<T>` wrapper that implements neither `Debug`, `Display`
   nor `Serialize` (form values serialise only inside the IPC frame that carries them), and a lint
   forbids formatting it. IPC message bodies are never logged.
4. Diagnostics name a snippet and a reason only when a match happened ("no word start", "template
   error E_…", "typed during injection") or when an explicit request for a named snippet was
   refused (§4.5; the entry names the snippet, the app and the reason). While the engine is
   suspended nothing typed is matched, so a suspension entry names only the app and the reason
   ("suspended: secure field"). Diagnostics are kept in a
   200-entry ring in memory.
5. `iamb-service` has no network code, checked in CI (§2). Everything that talks to the network
   lives in `iamb-sync` or `iamb-desktop`, which never receive key events.
6. Account tokens live in the OS keychain and are read only by `iamb-sync`.
7. Secure fields, excluded apps, input-method composition and Iamb's own windows suspend the
   engine before a character is stored, and an unknown state counts as suspended.
8. Content that Iamb puts on the clipboard to paste is marked so that clipboard history and cloud
   clipboard services skip it, and the previous clipboard content is restored afterwards.
9. Each process writes crash reports locally without heap contents and opts out of the operating
   system's own crash-dump collection where the OS allows. Only `iamb-desktop` uploads reports,
   and only after opt-in. Limit: an administrator who configures full-memory dumps for all
   processes can capture the typed-character buffers; Iamb cannot prevent that and says so in its
   privacy documentation.
10. Snippets written by other people are treated as untrusted input:
    - A template can type text, press the keys listed in the template spec (Shift is the only
      modifier), wait, and read the clipboard. That is enough to run a command in a terminal or
      to send clipboard content to a chat, so "cannot do harm" is not claimed.
    - A snippet created or changed by another user is held for review before its first expansion
      if its template contains `{key}`, `{wait}` or `{clipboard}`, or any line break. Until the
      user approves that revision (the review shows the author and the difference), the last
      approved revision keeps expanding, or nothing if there is none. Single-line plain text from
      other members expands without review.
    - Rich snippets render Markdown with raw HTML disabled and links limited to `http`, `https`
      and `mailto`.
    - Downloaded attachments are verified against their SHA-256 before use.

Threat model: Iamb protects typed text from itself (bugs, logs, sync), from other users on the
machine, and from other members of shared folders. It does not defend against malware already
running as the same user, which can observe the keyboard directly.

## 9. Sync

Task 4.1 specifies the conflict scenarios; the architecture fixes the frame
([ADR-0016](decisions/0016-sync-frame.md)):

- The server is a Postgres database with row-level security, reached over HTTPS and a live-update
  channel, hosted on Supabase.
- **Change feed.** The server keeps one ordered change feed per user. An entry is either a row
  change the user may see, or a visibility change: `folder_visible(folder)` when a share or
  membership gives the user access, `folder_revoked(folder)` when access ends. `sync_state` is the
  user's position in that feed.
- **Pull.** The agent reads the feed from its position. A row change is applied through
  `ApplyRemote`. `folder_visible` triggers a full fetch of that folder, whatever the rows'
  revisions, which is how a newly shared folder appears within the 10-second target.
  `folder_revoked` triggers `PurgeFolder`, which removes the folder, its snippets and its cached
  attachments from the device.
- **Push.** The store writes every local change to `outbox` in the same transaction as the change,
  with the revision it was based on. The agent pushes in order. The server applies last write
  wins per row and keeps the overwritten version in a history table. When the base revision is
  older than the server's row, the server also records a conflict; the device lists conflicts
  (`Conflicts`) so the user can restore the other version.
- Deletes are soft and are purged after 30 days on the server and on devices that have read past
  them.
- **Attachments.** Rows sync first. The agent then downloads every attachment of the effective
  library that the device lacks (`MissingAttachments`), within the size limits set in task 4.6,
  so snippets work offline after the first sync. It writes the bytes to `attachments/` itself
  (temporary file, hash check, rename) and tells the service with `AttachmentAvailable`. Uploads
  go the other way under the same path scheme.
- When a rich snippet is expanded, the service resolves `attachment:` references while it builds
  the clipboard content: HTML with images embedded as data URIs, plus plain text; a snippet that
  is only one media file is also offered as a file. A missing attachment falls back to the
  plain-text rendering with a `Diagnostic`.
- Plan features reach the device as `Entitlements` from the agent, stored by the service
  (`SetEntitlements`) and read by the UI.

## 10. Desktop UI

[ADR-0017](decisions/0017-ui-host.md) fixes the shape; task 3.1 designs the screens.

- `iamb-desktop` is a Tauri 2 app. It starts hidden at login and stays resident whether or not the
  tray icon is shown, because the fill-in form and the palette depend on it.
- It keeps two small windows created but hidden, the fill-in form and the search palette, so they
  appear within their targets (150 ms and 100 ms). On macOS they are non-activating panels, which
  needs a small amount of native window code in `iamb-desktop`. The library window is created on
  demand and destroyed when closed.
- Search runs in the worker, in memory over the snapshot (fuzzy matching on abbreviation, label,
  content and tag), on the reader thread of the requesting client.
- Rich snippets are edited as Markdown and stored as Markdown plus attachments
  ([ADR-0018](decisions/0018-rich-content.md)). Rendering Markdown to HTML happens in
  `iamb-core`, so desktop and mobile produce identical output.

## 11. Where each task lands

| Task | Component |
| --- | --- |
| 1.1 Data model | `iamb-core::model` |
| 1.2 Matcher | `iamb-core::matcher` |
| 1.3 Template parser and renderer | `iamb-core::template` |
| 1.4 Date math | `iamb-core::datetime` |
| 1.5 SQLite store | `iamb-store`, including `run/store.lock` |
| 1.6 Engine pipeline | `iamb-core::engine` (including the injection and form states), sessions in `iamb-testkit` |
| 1.7 Import and export | `iamb-store::transfer` |
| 1.8 Command-line tool | `iamb-cli` (store directly in Phase 1; the endpoint is added with task 2.6) |
| 1.9 Review | `iamb-core`, `iamb-store`, `iamb-cli`, `iamb-testkit` |
| 2.1 Windows listener | `iamb-platform-windows::listener`, `::hotkeys`; traits in `iamb-platform` |
| 2.2 Windows injector | `iamb-platform-windows::injector`, `::clipboard` |
| 2.3 macOS listener | `iamb-platform-macos::listener`, `::hotkeys`, `::permissions` |
| 2.4 macOS injector | `iamb-platform-macos::injector`, `::clipboard` |
| 2.5 Secure fields, exclusions, pause | `FocusInspector` in both OS crates; focus thread in `iamb-service`; suspension in `iamb-core::engine`; `app_exclusion` in `iamb-store` |
| 2.6 Background service | `iamb-service` (supervisor, heartbeat, worker threads, logging), `iamb-ipc` |
| 2.7 Compatibility harness | `iamb-testkit::compat`, driven through `iamb-service` |
| 2.8 Privacy review | `iamb-service`, `iamb-ipc`, `iamb-platform*`, `iamb-core::engine` |
| 3.1 UX spec and design system | `docs/ux/`, `apps/desktop/src/design/` |
| 3.2 Tauri shell | `iamb-desktop`, `iamb-ipc` client, `LoginItem` |
| 3.3 Library | `apps/desktop/src/library/`, Library messages |
| 3.4 Template editor | `apps/desktop/src/editor/`, Templates messages |
| 3.5 Fill-in form | Form panel in `iamb-desktop`; Forms messages; focus tokens in the OS crates; form memory in `iamb-service` |
| 3.6 Search palette | Palette panel in `iamb-desktop`; `iamb-core::search`; `Hotkeys`; `PaletteRequested` |
| 3.7 Onboarding | `apps/desktop/src/onboarding/`, `GetPermissions`, `SetTestField` |
| 3.8 Rich text snippets | `iamb-core::rich`, `Paste` action, clipboard building in `iamb-service`, attachments in `iamb-store` |
| 3.9 Settings and diagnostics | `apps/desktop/src/settings/`, Settings and Diagnostics messages |
| 3.10 Library health | `iamb-store::health` (conflicts, shadowing, broken references, unused), `Health` message |
| 3.11 Accessibility and polish | `apps/desktop` |
| 3.12 Review | `apps/desktop`, `iamb-desktop` |
| 4.1 Sync design | `docs/sync.md`, extends §7 and §9 |
| 4.2 Server schema and policies | `server/migrations/`, `server/tests/` |
| 4.3 Security review | `server/` |
| 4.4 Sync client | `iamb-sync` (library), `outbox` and `sync_state` in `iamb-store`, Sync messages |
| 4.5 Account UI | `apps/desktop/src/account/`, the account endpoint, review of received snippets |
| 4.6 Media sync | `iamb-sync::media`, `attachments/`, `MissingAttachments`, `AttachmentAvailable` |
| 4.7 Billing readiness | `server/migrations/` (plan and entitlement tables), `Entitlements`, `entitlement` table |
