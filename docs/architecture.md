# Iamb architecture

Status: approved by the owner on 2026-11-18; §4 amended the same day after the Phase 1 review (ADR-0024) · Task 0.1 ·
§4.4, §6.2, §6.4, §7, §8 rule 10 and §9 extended by the sync design of task 4.1, as revised
after its review, its follow-up review and its close-out review ([`sync-design.md`](sync-design.md);
ADR-0027, ADR-0028 and ADR-0029, proposed and awaiting the owner) ·
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
| `iamb-sync` | library + thin binary | core, ipc, platform (the service endpoint's name and the peer policy only); networking through `rustls`, `ureq` and `tungstenite` (task 4.4, `crates/iamb-sync/README.md`) | Sync agent: account session, push, pull, live updates, media transfer, account endpoint | Access to key events |
| `iamb-cli` | binary `iamb` | core, store, ipc, platform | Command-line tool: add, list, remove, import, dry-run expand | — |
| `iamb-desktop` | binary | core (model types only), ipc, platform(+os) | Tauri shell (`apps/desktop/src-tauri`): tray icon, windows, focus hand-back, bridges UI commands to `iamb-ipc` | Engine or store logic |
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
  `iamb-service`, `iamb-ipc`, `iamb-store` and the platform crates (clippy's `disallowed-types`
  and `disallowed-methods` in a `clippy.toml` of each, also for the socket calls of `libc`); (4)
  the release binary is built with `-p iamb-service` alone, so feature unification with other
  workspace members cannot add code to it. `docs/ci.md` says where each runs.
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
| Listener | Runs the OS hook or event tap and the hotkey registration; translates each key; stamps it with a sequence number and, where the OS says, the window it goes to; puts it in the key queue (§8 rule 1) and pushes a notice to the engine channel; returns |
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
translate, push, return (under 1 ms at the 99th percentile); the push writes into the fixed key
queue and the bounded engine channel and allocates nothing. If either is full the listener drops
the event and sets an overflow flag; the next event it delivers is `Overflow`, which clears the
buffer.

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
With every heartbeat the engine thread also asks whether the focus watcher still runs
(`FocusInspector::is_watching`); while it does not, the focus counts as unknown (the engine is
suspended, and a verdict for the focus the watcher last saw is taken as unknown) and the watcher
is started again, whose first report brings the next verdict.

Single instance is guaranteed by an exclusive lock on `run/supervisor.lock`.

"Start it again" and "Restart the helper" reach the supervisor through a file, not yet through
the control pipe of ADR-0023: the UI host creates the empty file `run/restart-worker` (created
new, so a link or a FIFO in its place is neither followed nor opened) and starts the supervisor,
which exits when one already runs. The running supervisor looks for the file every 200 ms,
removes it, asks the worker to stop by closing its standard input (killed after a second) and
starts a new one at once. The file has no peer verification: only the user can create it in
`run/` (§3.3), and it can do nothing a process of the user cannot already do by ending the
worker. It means "restart the worker" only; a stop, and what updates need, wait for the pipe
(`iamb_service::supervisor`, "Restart requests"; §8).

Packaging ([ADR-0021](decisions/0021-packaging-and-permissions.md)): on macOS the service is a
signed helper app with a stable bundle identifier inside `Iamb.app`, registered as a login item
through the system's login-item API by the main app, so that Accessibility and Input Monitoring
grants belong to the helper and survive updates. On Windows the supervisor is registered under the
per-user Run key. Updates are applied by the UI host, which asks the supervisor to stop the worker,
replaces the files, and asks it to start again ([ADR-0023](decisions/0023-updates.md)).

The UI host is supervised by its end, not by a heartbeat (`iamb_service::supervisor`, "The UI
host"). On macOS the supervisor never starts it as its own child: the system judges a program the
helper spawns as the helper, with the helper's keyboard permissions, so the supervisor has the
system open `Iamb.app` (`/usr/bin/open -g -W -a`), as the UI host has the system open the helper,
after checking that the app is the bundle around the helper (resolved path, no links, Iamb's
bundle identifier; a signature check follows with signed builds). `open` does not pass the app's
exit status on, so the UI host leaves `run/desktop.quit` when it ends on purpose (its "Quit", and
a quit by the system: a logout, a restart, a "quit" Apple event, which ends the process inside
AppKit, so the marker is left as the event loop ends), and there only an end without it is
restarted. Where the supervisor sees the UI host's own exit status (Windows, Linux) the status
decides and the marker is not read: status 0 is a quit, and a marker left beside a crash by
another process does not turn the crash into one. A UI host that steps aside for another one
already running for its data root exits with 0 and leaves no marker. Everywhere, a UI host that
exits with status 64 (a usage error), is ended with `SIGTERM`, or fails five starts in a row
within a second each is not started again. On macOS a `SIGTERM` (an installer, `killall`) cannot
be told from a crash behind `open`, so it is restarted there.

### 3.3 Files on disk

| Item | macOS | Windows |
| --- | --- | --- |
| Data root | `~/Library/Application Support/Iamb/` | `%APPDATA%\Iamb\` |
| Library | `library.db` (SQLite, WAL mode) | same |
| Attachments | `attachments/<profile id>/<sha256>` | same |
| Logs | `logs/<process>.log` (rotated, 5 × 1 MB) | same |
| Locks | `run/supervisor.lock`, `run/store.lock`, `run/desktop.lock` (one UI host per data root) | same |
| Signals between processes | `run/restart-worker` (UI host to supervisor: restart the worker, §3.2), `run/desktop.quit` (the UI host ended on purpose, §3.2); empty files, taken by removing them | same |
| Endpoints | `run/service.sock`, `run/sync.sock` | `\\.\pipe\iamb-service-<user SID>-<secret>`, `\\.\pipe\iamb-sync-<user SID>-<secret>`; each secret (128 random bits, new for every bind) in `run/service.pipe`, `run/sync.pipe`; `run/service.pipe.lock`, `run/sync.pipe.lock` |
| Pause | `run/paused` while Iamb is paused, so that a worker started afresh (a crash, a restart, a logout, a restart of the machine) starts paused, and says so (`ServiceStatus.paused_kept`: "Iamb is still paused"). Only a regular file counts, looked at without following a link; anything else there means not paused and is removed (a directory that is not empty is moved aside to `run/paused.aside-<n>`, so the pause can still be kept) (`iamb_service::worker::engine`, "Pause across a restart") | same |

On Linux (development and the command-line tool only) the data root is `$XDG_DATA_HOME/iamb/`.
Setting `IAMB_DATA_DIR` overrides the data root on every OS; tests always set it.

The data root is the user's alone wherever it is: the service creates it, or tightens an existing
one, before anything else (mode `0700` on macOS and Linux; on Windows a protected DACL with one
entry, full control for the user's SID, inherited by everything inside, set through
`iamb_platform_windows::restrict_to_current_user`; the 2.8 review's finding 20). The default under
`%APPDATA%` is private already; a root chosen with `--data-dir` or `IAMB_DATA_DIR` would otherwise
keep its parent's permissions. If that cannot be done (a file system without permissions), the
service does not start. The Windows endpoint restricts its `run` directory the same way before it
writes the pipe's secret there.

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
| `Boundary { kind, seq }` | A key or click that produced no text: Enter, Tab, Escape, arrows, navigation keys, mouse click, a shortcut chord | Cleared (Enter and Tab count as delimiters first only when the user added them to the set) |
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

- Where the OS tells it cheaply, the listener says which window a key went to (on Windows the
  window in front and its process, read in the hook; on macOS the process only: the event's
  `kCGEventTargetUnixProcessID` when the system has filled it in, otherwise the frontmost
  application as the focus watcher last noted it in an atomic, never asked of AppKit in the tap).
  A focus change reaches the engine later than the keys typed right after it (the watcher hears
  of it after the application, and it crosses the focus thread), so the thread that feeds the
  engine hands it no key whose window differs from the last `FocusChanged` it delivered: such a
  key is neither stored nor matched, and an expansion waiting for its start marker is cancelled.
  A move to another field of the same window is not seen this way. A key the listener cannot
  place is delivered without a target and handled as before; it is never dropped for that.
- Keys for an unreported window do not stop expansion silently: after three of them, or half a
  second of them, the service records `keys_for_unreported_window` and reads the focus from the
  inspector. Another window than the last report is judged as if the watcher had reported it.
  The same window means the listener and the inspector disagree about where keys go; once that
  has lasted half a second, keys with that stamp are handled as keys without a target until the
  focus changes.
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
| `Paste { plain, markdown_html, attachments }` | Insert rich content through the clipboard, then restore the clipboard. One per run of a rich snippet's rendered text between `key` and `wait` actions (task 3.8): `markdown_html` is the core's sanitised HTML (`iamb-core::rich`), whose images name attachments by hash, which the service resolves (§9); `plain` is the plain-text version, typed instead where the application must not be pasted into |
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
  this inside a profile for local edits; rows that arrive by sync are applied as they are, so two
  devices that each created the same abbreviation offline both keep their snippet
  ([ADR-0028](decisions/0028-data-model-for-sync.md)). A deterministic precedence decides which
  snippet holds an abbreviation: the user's personal profile first, and in it the snippets whose
  content the user wrote before those whose content someone else wrote (a member, in a folder
  the user shares); then the folders of other profiles by the time the server recorded the
  user's access to them (the earliest-created share), then the lower id of the shared folder;
  inside one of these the order in which the server first accepted the rows, then the lower id.
  No device's clock takes part, and a snippet written by someone else never takes an
  abbreviation from one the user wrote ([`sync-design.md`](sync-design.md) D13). The losers
  neither trigger nor resolve, and library health lists them.
- Case folding, used by the store's uniqueness check and by the matcher alike, is per-character
  lower-casing with final sigma folded to σ (`Abbreviation::fold_key`). There is no wider Unicode
  case folding: `ß` does not match `ss`.
- The matcher is a trie over reversed, case-folded abbreviations.
- Trigger modes per snippet: `immediate` (fires on the last character) and `delimiter` (fires when
  a delimiter follows). The delimiter set is a setting; the default is space and
  ``. , ; : ! ? ) ] } " '``. Enter and Tab are not in it (ADR-0026): the application acts on them
  before the expansion arrives, so it would land in the field or window they move to. A user can
  add them. When they are not in the set and an Enter or Tab follows an abbreviation that would
  have expanded after a delimiter, the engine records `not_a_delimiter` naming the snippet, so
  "Why didn't it expand?" can say so. In delimiter mode the delimiter is deleted with the abbreviation and typed again after
  the expansion, except when the snippet has a `{cursor}`.
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

A form session always ends (the Phase 3 UI review's finding 1). While one is open the engine
drops every typed match with `form_open`, so a session nobody can answer would stop all
expansion. Besides submit, cancel, the 2-second acknowledgement and the UI host's disconnection:
the UI host renews an acknowledged form with `FormAck` every 10 seconds, whether or not its
panel is on screen, and the service cancels a form whose lease (`FORM_LEASE`, 60 seconds) runs
out (`form_expired`), as the `PanelFocus` lease bounds a panel's hold; a form submit for a window
that no longer exists, or a parked submit that is dropped (its window never got the focus, or a
newer request replaced it), ends the session without typing and keeps its values for
`ReopenLastForm`, as a refused submit does (`form_target_gone`, `request_focus_timeout`,
`request_cancelled`). In the UI host, a panel the system closes is cancelled as Esc cancels it,
and a form whose window is gone (closed, or its application quit) ends with its answers kept:
the panel hands them in with `FormSubmit` and the service, seeing the window gone, types nothing.

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
window, once the UI host has handed focus back). The "Fill in the last form again" shortcut reads
the focus token at the press, as the palette's shortcut does, so it runs under the verdict of the
window it was pressed in, never under the one before. `ReopenLastForm` carries the token of the
window it is for when the UI host knows it (an optional `focus`, as `ExpandSnippet`'s: the tray
menu and the palette send the window they hand the focus back to), and then waits for that
window alone: on Windows the click on the icon can bring the shell's taskbar to the front, and
neither the taskbar reported last nor another window reported next decides it. One without a
token that arrives while the last focus delivered is Iamb's own window waits for the next focus
change to another window, for the same time limit, instead of being refused there.

Keys typed into Iamb's own windows are not expanded: the focus thread reports `own_window` for the
UI host's process. The one exception is the onboarding test field, which the UI host declares with
`SetTestField(window)`.

On macOS the form and the palette are non-activating panels, so the frontmost application stays
the focused one and the focus thread cannot see that a panel has the keyboard (the 2.8 review's
finding 16). The UI host says so: `PanelFocus { panel, shown }` (Forms group, `ui` only;
[ADR-0013](decisions/0013-form-flow.md), addendum). It sends `shown: true` and waits for the
answer before the panel takes the keyboard, renews it while the panel stays open (the lease is 30
seconds), and sends `shown: false` once it has given the keyboard back, before `FormSubmit` or
`ExpandSnippet`. While a panel has the keyboard, the engine thread drops every key (`Char`,
`Backspace`, `Boundary`) before the hand-off and the engine see it, and before the key-target
check of §4.1, and holds the engine in `own_window`: verdicts that arrive meanwhile are noted and
delivered as `own_window`, and explicit requests are refused (`refused_own_window`). When the
panel gives the keyboard back, the last verdict for the current focus is delivered again, on the
engine thread in queue order, so the request the UI host sends next finds it in force. A submit
or cancel ends a form's hold, a pick or `ReopenLastForm` the palette's; a lease that runs out
(diagnostic `panel_focus_expired`) and the UI host's disconnection end it too. On Windows the
panels take the foreground and the focus thread reports `own_window` itself; the message changes
nothing there.

The last submitted values of each form are kept so that a form can be reopened pre-filled
(`ReopenLastForm`). They are user-entered text and are treated like typed keys: held only in the
worker's memory, never written to disk or logs, and cleared after 30 minutes without use, on
screen lock, on sign-out and on restart. Screen lock and sign-out reach the worker through the
platform's `SessionEvents` (§5): every event sets a flag the engine thread reads before its next
message, so a full engine queue cannot lose it, and the engine thread forgets the values and
disturbs the typed-characters hand-off. Forgotten values are overwritten, not only freed: a
`FormValues` overwrites its values with zeros when it is dropped or wiped, the service overwrites
the copy a `FormSubmit` carried, and a clipboard snapshot overwrites its bytes when dropped.

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
keeps no copy of its own: it cannot know a trigger happened until the engine has matched, so a
copy there would have to hold every key all the time, including keys the engine drops in secure
fields. What it does hold is each key on its way to the engine thread, in the key queue, which
overwrites a character as soon as the engine thread has taken it (§8 rule 1). A boundary, an
over-deleting Backspace, a 65th character, composition, overflow, a focus change, a pause, or a
key the listener says went to another window than the last focus change (§4.1) cancels the
expansion. So does a dead key still waiting when the start marker arrives: the
listener reports it as a boundary just before `InjectionStarted`, because the target would spend
the first Backspace on it. The 2.8 privacy review checks this.

The hand-off cancels only for what reaches the engine thread before the start marker comes back,
and a window can take the focus before its watcher reports it. So the injector watches the focus
itself (`iamb_platform::FocusGuard`). Each request names the window its keys belong to: that of
the last `FocusChanged` the engine thread delivered, which every key it let through was compared
with (before the first report, the trigger key's own target). Right before the start marker the
injector reads where keys go and compares: when that is another window (on macOS, another
process), it sends nothing, not even the start marker, and answers `Cancelled(FocusMoved)` (the
2.8 follow-up review's N1). Otherwise that same reading is the guard's reference, and before
each batch (Windows) or each key operation (macOS) it compares again: when the window, its
process or the field it can see changed, it stops with `Cancelled(FocusMoved)` and posts no end
marker; the service answers with `InjectionCancelled`. What it compares never waits for the
target: on Windows the foreground window, its process and the focused control the system names
(a browser engine draws its fields in one window, so a move between them is not seen there); on
macOS the frontmost process, the secure-input flag and the count of the focus watcher's
notifications. Steps sent before the stop stay where they went. Once the expansion itself sends a
key that may move the focus (Tab, Enter, Escape, Space, an arrow other than Left, a line break
or a tab in text), the comparisons end for that expansion, because the move it causes cannot be
told from another. What none of this sees is a move to another field of the trigger's window
before the start check (the request names a window, not a field, and on macOS only the
process): a Tab or an Enter the user added to the delimiters moves the focus inside its own form
or dialog, which is why they are not delimiters by default
([ADR-0026](decisions/0026-enter-and-tab-not-default-delimiters.md)).

A paste never replaces clipboard content that its snapshot could not save whole (a format the
owner did not deliver, a handle the snapshot cannot copy, a format past the size or time limit),
because the restore could not put that part back (§8 rule 8). On Windows the snapshot copies an
enhanced metafile as its records and leaves out the bitmap, palette and metafile picture the
system makes again from a device-independent bitmap or an enhanced metafile, so the picture an
office program puts next to copied text does not make it incomplete (the follow-up review's N4).
Plain text is then typed with keys instead; content
that keys cannot carry (HTML, files) stops the expansion with `Cancelled(ClipboardNotRestorable)`.

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
| `FocusInspector` | `focused_app()`, `secure_state(target, timeout)`, `focus_token()`, `focus_now()` (the application and its window from one reading, for the service's own look at the focus; the close-out review's C3), `restore_focus(token)`, `allow_foreground(pid)`, `watch(sink)`, `unwatch()`, `is_watching()` (false once the watcher's thread has ended by itself) | service, UI host | Foreground window, its process and integrity level; window-event hooks for focus changes; password fields by the edit control's style, and through UI Automation `IsPassword` on a dedicated thread | Frontmost app and its front window; activation notifications and an Accessibility observer for focus changes; secure fields by the session's secure-input state, and by the focused element's role and subrole on a dedicated thread |
| `ClipboardAccess` | `read_text`, `snapshot`, `write(content, transient)`, `restore` | service | Clipboard API, with the formats that keep content out of clipboard history | Pasteboard API, with the transient and concealed markers, for this device only |
| `Hotkeys` | `register(id, chord)`, `unregister(id)` | service | OS hotkey API on the listener thread | OS hotkey API on the listener's run loop |
| `Permissions` | `permission_status()`, `open_settings(kind)` | service | Always granted | Accessibility and Input Monitoring, checked in the worker, which is the process that holds them |
| `LoginItem` | `enable()`, `disable()`, `login_item_status()` | UI host | Per-user Run key | Login-item registration of the helper app |
| `MemoryLock` | `lock(region)`, `unlock(region)` | service | Keeps the keystroke buffer and hand-off out of the page file | Same, out of swap |
| `LocalEndpoint` | `bind(name, policy)`, `connect(name, policy, timeout)`; streams carry the verified peer | service, UI host, CLI, sync agent | Named pipe whose name ends in a secret kept in the user-only `run` directory, first instance only, remote clients rejected, user SID checked | Unix socket in a user-only directory, peer uid checked |
| `SessionEvents` | `watch_session(sink)`, `unwatch_session()`; events `Locked`, `SwitchedOut`, `SigningOut`, `Sleeping` (a report too many only forgets a form's answers) | service | A hidden top-level window and a window-event hook on a thread of their own: `EVENT_SYSTEM_DESKTOPSWITCH` (the lock screen, a secure desktop), `WM_QUERYENDSESSION` and `WM_ENDSESSION`. `WM_WTSSESSION_CHANGE` needs `WTSRegisterSessionNotification`, which the bindings keep in the Remote Desktop namespace the service may not link (§2) | One observer on the main run loop: `com.apple.screenIsLocked` (distributed centre, delivered at once), `NSWorkspaceScreensDidSleepNotification`, `NSWorkspaceSessionDidResignActiveNotification`, `NSWorkspaceWillPowerOffNotification`, `NSWorkspaceWillSleepNotification` |

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
  clients. The pipe's name ends in a secret that only the user's `run` directory names (§3.3),
  so another local user cannot create it first (the 2.8 review's finding 12); a name that exists
  all the same makes the server draw another secret. Which server owns the endpoint of a data
  root is decided by an exclusive lock beside it (`<kind>.sock.lock`, `<kind>.pipe.lock`): a
  second one gets "the name is taken". The worker does not need its endpoint to expand (§3):
  when the bind fails it starts without it, logs why, records the diagnostic
  `endpoint_unavailable`, and tries again every 5 seconds until it can serve it.
- Client side: before sending anything, a client verifies that the server process runs as the same
  user and, in release builds, that its executable belongs to the same installation as the
  client (below). Servers verify clients the same way. The same-user check always comes first.
- **The installed-binary check** (release builds, `PeerPolicy::InstalledBinary`;
  `iamb_platform::installation`; the 2.8 review's finding 11, and its strict review,
  `docs/reviews/finding-11-security-review.md`). An installation is found from the process's own
  canonical executable: on macOS `<X>.app/Contents/MacOS/iamb-desktop` or
  `<X>.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service`, on Windows
  an Iamb executable whose canonical directory is the canonical per-user installation directory
  (the `LocalAppData` known folder from `SHGetKnownFolderPath`, not `%LOCALAPPDATA%`, and `Iamb`
  under it, with junctions, 8.3 names and redirection resolved). Its members are what the
  installer puts there: the UI host and the service (`iamb` and `iamb-sync` are added, with
  their bundle identifiers, when they ship). The process's own signing state is read once, first
  in `main`, from its running code (macOS: the dynamic code object checked against
  `anchor apple generic` and its team identifier; Windows: `WinVerifyTrust` on its image), or is
  compiled in by the release pipeline (`IAMB_SIGNER_TEAM` on macOS, `IAMB_SIGNER_THUMBPRINT` on
  Windows with a certificate; such a build never asks). It has three states: signed by a signer;
  unsigned (no signature, or ad hoc on macOS); and could not tell (any error), which trusts no
  peer. Every service and UI host logs which (`installed-binary check: …`).

  | This process | The peer | Trusted |
  | --- | --- | --- |
  | Not in an installation, or could not tell how it is signed | anything | no |
  | Signed | its canonical path is a member's, and it is signed by the same signer: macOS, the code its audit token names is that member and satisfies `anchor apple generic`, the team identifier and `org.iamb.Iamb` or `org.iamb.Iamb.Service` (`SecCodeCheckValidity`, no network access); Windows, the file at that path verifies with the same certificate thumbprint | yes |
  | Signed | its canonical path is a member's, but it is unsigned, ad hoc, invalid or signed by another signer | no |
  | Unsigned | its canonical path is a member's (macOS: and the code its audit token names is that member), and the file there is the member's file (macOS: device and inode; Windows: volume serial number and file index) | yes |
  | either | its canonical path is no member's: another folder, a copy of `iamb-desktop` elsewhere, a program that does not ship | no |

  Paths are compared by components with ASCII letters folded, as the file systems compare names.
  On Windows the process is opened and kept open while it is examined, and one created after it
  connected (a process id reused since) is refused. What the check rests on, and does not stop:
  the path is what the system reports for the running process (macOS: the executable's vnode,
  which holds no link, so a process started through a link to a member counts as that member;
  Windows: the image name), and the file identity and, on Windows, the signature are those of the
  file found at that path, not of the image the process was mapped from, which neither system
  lets another process ask for. So in an unsigned installation a process of the user that
  replaces a member's file is trusted as that member (it could replace the whole installation as
  well), and on Windows a process that runs other content from a member's path and puts the
  signed file back before it connects passes as that member; a process of the same user at the
  same integrity level can write into the service's memory on Windows anyway, which §8 leaves out
  of the threat model. A signed installation, its signer read at the start or compiled in, never
  falls back to the path check. Linux (development only) has no such check. The install smoke
  test (`docs/release.md`) connects the installed UI host to the installed service
  (`iamb-desktop --check-service`) on every OS and requires the service itself to refuse a copy
  of the UI host from another folder, and on a signed macOS run an unsigned UI host in place of
  the real one.
- The role a client states must be one its executable may take, by the file name the endpoint
  read from the operating system (§6.2; `iamb_ipc::roles`; the 2.8 review's finding 10): an
  Iamb client is held to its role in every build, and release builds accept no other program.
- Framing: a 4-byte little-endian length, then one UTF-8 JSON document, at most 8 MB. Bulk data
  (import, export, attachments) is passed as file paths, never inline.
- Shape: `Request { id, body }` → `Response { id, result | error }`, plus `Event { topic, body }`
  pushed by the server.
- The first message is `Hello { protocol: 3, role, client_version }`; a different major protocol
  version is refused with a clear error. The major version is raised whenever a build could
  misread what a build of the previous version sends: a message or field removed, renamed or
  given another meaning, a new required field, or a request answered with another response type.
  Additions an older peer can ignore (a new request, event or optional field) do not raise it.
  `PROTOCOL_VERSION` in `crates/iamb-ipc/src/protocol.rs` lists each step; 2 is `PutExclusion`
  answering `Exclusions` instead of `Done`; 3 makes the time of a diagnostic (`at`) required,
  gives `Preview` its parts, cursor and fields instead of the text, and turns a template error's
  `position` into `start` and `end` (template positions are UTF-16 offsets, the unit of the
  editor's strings).
- Message bodies are never logged. Logs record the message name, size and duration.

### 6.2 The service endpoint

| Role | Used by (executable) | May call |
| --- | --- | --- |
| `ui` | `iamb-desktop` (`iamb-desktop`, `iamb-desktop.exe`) | Library, Engine control, Forms, Templates, Diagnostics, Settings |
| `cli` | `iamb-cli` (`iamb`, `iamb.exe`) | Library, Engine control |
| `sync` | `iamb-sync` (`iamb-sync`, `iamb-sync.exe`) | Sync only |

The service refuses a hello whose role is not the one of the client's executable
(`not_permitted`): a taken-over sync agent cannot say `ui`. Names are compared without regard to
case. Under the release policy an executable with any other name may take no role; debug builds
and tests let other programs (test binaries, the harnesses) take any role. The UI host's
executable must therefore keep the name `iamb-desktop` in its bundle (Tauri's main binary name
left at the cargo name). On Linux the endpoint names no executable and the role is not checked.

| Group | Requests | Events |
| --- | --- | --- |
| Library | `ListFolders`, `ListSnippets(query)`, `ListSnippetSummaries(query, offset, limit)`, `GetSnippet`, `PutSnippet(snippet, tags)`, `DeleteSnippet`, `CheckAbbreviation(abbreviation, trigger, snippet)`, `PutFolder`, `DeleteFolder`, `PutTag`, `ListTags`, `DeleteTag`, `Search(text)`, `Import(path, format)`, `Export(path, format)`, `Health`, `RecentSnippets(limit)`, `ListPendingReviews`, `ApproveSnippet(id, rev)`, `PutAttachment(path, file_name, folder)` | `LibraryChanged(revision, changed rows)` |
| Engine control | `GetStatus`, `SetPaused`, `ExpandSnippet(id, focus)`, `DryRun(typed)`, `GetPermissions`, `OpenPermissionSettings(kind)`, `SetTestField(window)` | `StatusChanged`, `PaletteRequested(focus)` |
| Forms | `FormAck(session)`, `FormSubmit(session, values)`, `FormCancel(session)`, `ReopenLastForm(focus?)`, `PanelFocus(panel, shown)` | `FormRequested(session, spec, focus, last_values)` |
| Templates | `ParseTemplate(source)`, `RenderPreview(source, values, rich)` | — |
| Diagnostics | `RecentDiagnostics` | `DiagnosticAdded` |
| Settings | `GetSettings`, `PutSetting`, `ListExclusions`, `PutExclusion`, `ResetUsage` | `SettingsChanged` |
| Sync | `OutboxRead(limit)`, `OutboxAck(results)`, `ApplyRemote(batch, position, mode)`, `PurgeFolder(id, except)`, `AttachmentAvailable(profile, sha256)`, `MissingAttachments`, `SetSyncStatus`, `SetEntitlements`; added by the sync design (task 4.1, built in task 4.4): `SyncState`, `Adopt`, `AdoptMerge`, `ReconcileBegin`, `ReconcileEnd`, `SetAccess`, `SetKnownUsers`, `PutShare`, `DeleteShare`, `SetBlocked`, `SignedOut`, `AttachmentsToUpload`, `UploadedAttachment`, `NoteAccessRemoval`, `AccessRemovals` | `OutboxChanged` |

Names across the layers: the engine action `ShowForm` becomes the event `FormRequested`; the
requests `FormSubmit` and `FormCancel` become the engine events `FormSubmitted` and
`FormCancelled`. The engine event `SnapshotChanged` is internal to the worker; `LibraryChanged` is
what clients hear.

`DryRun(typed)` feeds a string to a fresh engine as if each character were typed and returns the
actions; nothing is injected. `ParseTemplate` returns the token list the editor turns into chips
(each macro with its attributes) and the first parse or resolve error with its span, with every
`{snippet}` looked up in the snapshot; `RenderPreview` returns what an expansion would do (text,
key presses and waits in order, with the ranges each fill-in field produced), the cursor
position and the form's fields, or the error. A text field without a value or default shows its
name as a sample, and the clipboard is never read (the preview shows the sample `Clipboard`).
The editor therefore uses the same parser as expansion and always sees the whole library, with
no second build of the core ([ADR-0017](decisions/0017-ui-host.md); `iamb_service::templates`).

The library window's other needs (`docs/ux/spec.md` §11.2): `PutSnippet` carries the snippet's
tags (absent: unchanged) and restores a deleted snippet when given its id, with its usage and
tags (Undo); `ListTags` and `DeleteTag` complete `PutTag`, whose colour must be one of the eight
names of the design system and whose name must be unique in the profile without regard to case;
`ListSnippetSummaries` pages the list without bodies (at most 1,000 per page, each with its first
line, its tags and why it is broken, if it is); `CheckAbbreviation` answers the editor's warnings
(the snippet that holds the abbreviation, the ones it would hide or be hidden by) from the
snapshot without writing; a `Search` hit says which field matched and where; `LibraryChanged`
names the snippets, folders and tags a change touched, or nothing when it may have touched any
(an import); `PutSetting` of a shortcut (`palette_hotkey`, and `reopen_form_hotkey` and
`pause_hotkey`, whose presses do what `ReopenLastForm` and `SetPaused` do) registers the chord
before it stores it and answers `conflict` when the system refuses it (the old chord stays); and
every diagnostic carries its time.

The Sync group as the sync design fixes it ([`sync-design.md`](sync-design.md) §7.2, §9): what
`OutboxRead` returns is one change per row, ready to send (the row as it is now, the base revision
of its oldest waiting entry, an id that makes a repeat harmless), and the store marks what it
returned as handed out, so that a row the server may have seen is never dropped unsent; `OutboxAck`
takes the server's answer for each change, and for each row a folder's delete took with it, and acts
on it (a refused change is kept for the user, one the server cannot store yet stays local), and
reports the rows that are still behind what a pull skipped; `ApplyRemote` never writes a pulled row
over an unsent change, notes what it skipped, answers per row, and stores the feed position in the
same transaction; its `mode` says whether the rows come from the feed, from a resync, or from a
resync after the server was restored from a backup, in which a row the device holds in a revision
the backup lacks is kept and sent again instead of being replaced; `PurgeFolder` keeps the folder's
unsent changes for the user before it removes the folder, leaves the sub-folders named in `except`,
and is also what a folder that arrives as "no longer there" does, which leaves in place the parts of
it that are shared by themselves; `NoteAccessRemoval` records a call by which the user took access
away from someone, before the call is sent and again when it is answered, so that the device can
make it again after such a restore, before it gives anything back; `SignedOut` says whether the
server confirmed the sign-out: only then are other people's folders removed, and otherwise they are
set aside, unused and unseen, until the account has signed in again and synced, for 30 days at most. The Library group gains `ListKeptChanges`, `CopyKeptChange` and `DismissKeptChange` for
those kept changes, and `ApproveSnippet` names the content it approves. No build answered the Sync
group before task 4.4, which raises the protocol to 4 with it.

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

The sync design adds what the account screens need ([`sync-design.md`](sync-design.md) §7.2):
`CreateProfile`, `RenameProfile`, `DeleteProfile`, `TransferOwnership`, `AcceptOwnership`,
`DeclineOwnership`, `Members`, `SetMemberRole`, `LeaveProfile`, `WithdrawInvitation`,
`DeclineInvitation`, `GetConflict`, `ResolveConflict`,
`Devices`, `RenameDevice`, `RevokeDevice`, `SignOutEverywhere`, `SetDisplayName`, `DeleteAccount`
and `SyncNow`. Groups, memberships and invitations change only through these calls and need a
connection; `SetFolderShare` and `RemoveFolderShare` are written to the store as local changes
(`PutShare`, `DeleteShare` of the Sync group) and so work offline.

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
| `folder_access`, `snippet_origin`, `kept_change`, `known_user`, `attachment_state`, `skipped`, `left_alone`, `access_removal` | Added by the sync design ([`sync-design.md`](sync-design.md) §7.1, built in task 4.4): the user's permission on each folder shared with them and when it began, as the server computed it; who wrote a snippet's content and when the server first accepted the row; changes the server refused, kept for the user; display names of authors and members; which attachment files this device has sent; pulled rows that were not applied because of an unsent change; rows a resync left in place for the same reason; the calls by which the user took access away from someone, which the device makes again after a restore of the server. `approval` also keeps the content that was approved, and `sync_state` the account the library belongs to, how far a first sign-in has come, and since when other people's folders are set aside after a sign-out the server did not confirm |

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

What the sync design adds to this model ([`sync-design.md`](sync-design.md) §3 to §5,
[ADR-0028](decisions/0028-data-model-for-sync.md)): the server sets `rev` from one growing
counter, `updated_at` from its own clock and `updated_by` from the caller, and for a snippet also
who last changed what the snippet does (`content_by`, `content_at`; the writer only when the
content differs from the version the write was based on), which travels beside the row with the
revision at which the server first accepted it and is kept in `snippet_origin`; `Profile`,
`Membership` and invitations are written by the server only, through the account endpoint, and
the ownership of a group changes only when the new owner accepts; a `FolderShare`'s id is
derived from its folder and group, every other id is made on the device; a folder never changes
its profile; a member with `edit` may move snippets and sub-folders only within the shared
folder, and may not rename, move or delete the shared folder itself, wherever it lies in its
owner's tree; a tag's name and colour can be changed by whoever may manage the profile.
`sync-design.md` §4 has the permissions row by row.

## 8. Privacy and security rules

These hold for every phase and are what the Phase 2 privacy review checks
([ADR-0012](decisions/0012-keystroke-privacy.md),
[ADR-0022](decisions/0022-shared-snippet-trust.md)).

1. Typed characters exist only in the places below. No API returns any of them; they are never
   serialised, logged or sent over IPC.
   - **Three places in locked memory, overwritten with zero:**
     - the key queue between the listener and the engine thread: one fixed allocation of 256
       slots; each character only until the engine thread takes it, when its slot is
       overwritten, and every slot is zeroed when the worker stops; the engine channel itself
       carries a notice without content. Every key passes through it, also keys typed into a
       secure field, an excluded app or under pause, which the engine then drops unstored;
     - the engine's 66-character keystroke buffer: ordinary fields only, never while the
       engine is suspended, cleared at every boundary, focus change, verdict, pause,
       composition, overflow and expansion;
     - while an expansion is in flight, the engine-feeding thread's copy of the characters
       typed after its trigger (the typed-characters hand-off: at most 64, for the injector to
       type again, §4.6), zeroed when the expansion ends. It is armed for every expansion, also
       for an explicit request (a palette pick, a form submit, `ExpandSnippet`,
       `ReopenLastForm`) made while paused: the keys typed between such a request's expansion
       and the injector's start marker are captured and typed again after it, so that they are
       not lost under the expansion, although §4.1 otherwise drops keys typed under pause
       unstored. A secure field and an excluded app refuse explicit requests, so nothing typed
       there is captured.
   - **One value each in ordinary memory, overwritten when it is replaced or used and when the
     listener stops:** the Windows listener's pending dead character and the first half of a
     character outside the Basic Multilingual Plane that another program is injecting
     (`VK_PACKET`, `listener/pipeline.rs`, `listener/deadkey.rs`); the macOS listener's 32-bit
     dead-key state, which says which accent waits (`listener/translate.rs`).
   - **Copies while one key or one expansion is handled, in ordinary memory:** the hook or tap
     callback's stack and the engine thread's stack hold the key being handled until later
     calls overwrite them; the injector's batch holds the characters it types again (zeroed
     after it is sent: on Windows when the batch is dropped, on macOS in fixed chunks); an
     expansion's steps hold the one delimiter that triggered it (typed again after the text)
     until the injection ends. The follow-up review found the last key handled in a few such
     stack copies, never the keys before it.
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
   engine before a character is stored, and an unknown state counts as suspended. "Unknown"
   means a question that got no answer, not "nothing to inspect": what the platform layers
   cannot see (a password prompt in a terminal, a field nobody describes, some input methods)
   counts as an ordinary field, and the remedy is the exclusion list or the pause. Those limits
   are listed under "Detection limits" below.
8. Content that Iamb puts on the clipboard to paste is marked so that clipboard history and cloud
   clipboard services skip it, and the previous clipboard content is restored afterwards. On
   Windows that is three formats on every write and restore; on macOS the nspasteboard.org
   transient and concealed markers, and the write is for this device only, so the system does not
   offer it to the user's other devices; a restore is for this device only too when the saved
   content carried either marker. A paste whose snapshot is incomplete is not made (§4.6). A
   restore makes Iamb the clipboard's owner and moves its change count, so an application that
   clears its own copy "if unchanged" leaves the restored copy in place.
9. Each process writes crash reports locally without heap contents and opts out of the operating
   system's own crash-dump collection where the OS allows. Only `iamb-desktop` uploads reports,
   and only after opt-in. Limit: an administrator who configures full-memory dumps for all
   processes can capture the typed-character buffers; Iamb cannot prevent that and says so in its
   privacy documentation. How the service does it is under "Crash handling" below.
10. Snippets written by other people are treated as untrusted input:
    - A template can type text, press the keys listed in the template spec (Shift is the only
      modifier), wait, and read the clipboard. That is enough to run a command in a terminal or
      to send clipboard content to a chat, so "cannot do harm" is not claimed.
    - A snippet created or changed by another user is held for review before its first expansion
      if its template contains `{key}`, `{wait}` or `{clipboard}`, or any line break. Until the
      user approves that revision (the review shows the author and the difference), the last
      approved revision keeps expanding, or nothing if there is none. Single-line plain text from
      other members expands without review. "By another user" is the author of the snippet's
      content as the server records it, not the row's last writer, and what is approved is the
      content, so that a move or a new label does not ask again and nobody's edit, also one sent
      from a device that was a moment behind, can make someone else's key presses count as the
      user's own; a held snippet cannot be copied before it is reviewed
      ([ADR-0028](decisions/0028-data-model-for-sync.md), [`sync-design.md`](sync-design.md) C17,
      D10). The sync design proposes to hold a `{snippet}` reference written by another user as
      well, because it types the reader's own snippets; that addition is the owner's to confirm
      (`sync-design.md` §13, question 10).
    - Rich snippets render Markdown with raw HTML disabled and links limited to `http`, `https`
      and `mailto`.
    - Downloaded attachments are verified against their SHA-256 before use.

Threat model: Iamb protects typed text from itself (bugs, logs, sync), from other users on the
machine, and from other members of shared folders. It does not defend against malware already
running as the same user, which can observe the keyboard directly.

On macOS a process of the same user cannot observe the keyboard until the user grants it, and the
grant belongs to the helper app (ADR-0021), so the helper must not lend it out:

- it never starts another program as its own child, which the system would judge as the helper
  (§3.2): the UI host is opened through the system, and a release build refuses `--ui-host`
  on either platform and `--platform mock` (the mock is for development builds; Linux, which
  has no native platform, keeps it);
- any process of the user can have the system start the helper with arguments of its choosing
  (`open -n … --args …`). A release build of the helper therefore runs for the default data root
  only (`--data-dir` and `IAMB_DATA_DIR` naming another are refused,
  `iamb_service::paths::choose_data_root`), so a process cannot point a granted helper at a
  library it wrote elsewhere and have it typed. The default is built from the home directory the
  user database gives for the account (`getpwuid_r`), not from `HOME`, which the caller of `open`
  sets (`open --env`), and is used whatever `HOME` says. A refusal (another root named, or no
  answer from the user database) is written to the default root's log as well as to standard
  error. This does not stop a process that writes into the
  default data root itself, the user's own folder: that, like other malware of the same user, is
  outside this threat model. Development builds accept any data root.

The UI host and the supervisor talk through two empty files in `run/` until the control pipe of
ADR-0023 exists (§3.2, §3.3): `run/restart-worker` and `run/desktop.quit`. Neither has the peer
verification of §6.1; both rest on `run/` being the user's alone, and on what they can do: restart
the worker, and keep a UI host that ended from being started again (on macOS; elsewhere the
supervisor reads the UI host's exit status instead). The same holds for `run/paused`, which the
worker keeps itself: a process of the user that creates or removes it changes the pause at the
next worker start, which is no more than `SetPaused` can do, without the endpoint's peer check. Neither may grow another
meaning (a stop, an update) without the pipe.

**Crash handling (rule 9).** The supervisor and the worker do two things first in `main`
(`iamb_service::crash`); the UI host does the same (`iamb_desktop::crash`, process name
`desktop`):

| | Opt-out of the system's dumps (before any thread starts) | Where |
| --- | --- | --- |
| Windows | `SetErrorMode` adds `SEM_NOGPFAULTERRORBOX` (the system does not invoke Windows Error Reporting for the process) and `SEM_FAILCRITICALERRORS`; `WerSetFlags(WER_FAULT_REPORTING_FLAG_NOHEAP)` so that a report WER still makes (a hang, a fail-fast exit) holds no heap. Inherited by the worker | `iamb_platform_windows::harden_process` |
| macOS | `RLIMIT_CORE` 0, soft and hard: no core file in `/cores`. The system's crash report (`~/Library/Logs/DiagnosticReports`) is still written; it holds backtraces, registers and the loaded images, not heap memory | `iamb_platform_macos::harden_process` |
| Linux (development and CI only) | `/proc/self/coredump_filter` set to 0: a core file still appears but holds no memory mapping. The core limit and `PR_SET_DUMPABLE` need `unsafe`, which the service forbids and for which Linux has no platform crate | `iamb_service::crash`, `iamb_desktop::crash` |

The worker logs which steps took effect (`crash dumps: … ok`), a warning when one did not. Then
the panic hook: a panic writes a local crash report `logs/crash-<process>-<unix ms>.txt` (user
only, the last 10 per process kept) with the process, version, system, time, process id, thread
name and source location, and nothing else: never the panic message, which can quote data, nor
a backtrace or memory. The hook then exits with status 70 (`EXIT_PANIC`) instead of aborting, so
a Rust panic never becomes an operating-system crash, and the supervisor restarts the worker as
after any abnormal exit. What still ends the process the system's way is a fault that is not a
panic (in the OS crates' `unsafe` code, or a signal); the opt-outs above apply to it. Not
prevented: a dump taken from outside the process (a debugger, Task Manager, a full-memory dump
policy), the limit of rule 9. `iamb-desktop` will read the local reports for its opt-in upload.
The UI host's web view processes are started by the system's web view library (WebView2's
browser processes, `WKWebView`'s XPC services, WebKitGTK's helpers), so the opt-outs above do
not reach them. They never receive key events; a form's values reach them only while the form
is shown.

**Detection limits (rule 7).** The platform layers suspend the engine for what they can
recognise. What they cannot is treated as an ordinary field, because refusing every window that
describes nothing would stop Iamb in most terminals and several editors. The remedy for each is
the exclusion list (or the pause), which is why an exclusion must never fail silently. The module
docs named here hold the details; `docs/compatibility-run.md` has the manual checks.

| Limit | Windows | macOS |
| --- | --- | --- |
| A password prompt in a terminal (`ssh`, `sudo`, `runas`) | Not detected: a console does not say that it stopped echoing (`focus.rs`) | Detected only while the terminal has Secure Keyboard Entry on (`focus.rs`) |
| An application that draws its own fields and describes none (games, remote-desktop and virtual-machine windows, Java without the accessibility bridge) | `Normal`, not `Unknown` | Not recognised unless it switches secure keyboard entry on or reports a secure text field; no focused element at all is `Unknown` |
| A field that becomes a password field, or stops being one, without a focus change (a "show password" switch) | Seen at the next focus change. For an application that answers UI Automation late, the answer used is the field as it was when the application answered, used at most 250 ms after it arrived (`focus.rs`, "A late answer") | Seen when secure keyboard entry changes, else at the next application or window change |
| Input methods | One on an ordinary layout handle (Vietnamese Telex, Indic transliteration) is not recognised; a switch the listener does not notice is seen one key late (`listener/ime.rs`). Otherwise fails closed: an unanswered question counts as composing | The layout is published by the main thread, so a key typed right after a switch is read with the old one; the press-and-hold accent menu is invisible (`listener/inputs.rs`). Otherwise fails closed: no key is text while an input method is selected |

In each case what is typed passes through the 66-character buffer in locked memory like any
typing (never stored, logged or sent, cleared at the next boundary), and an abbreviation in it
can expand.

**Open findings of the Phase 2 privacy review.** Three low findings of
[`2.8-phase2-privacy-review.md`](reviews/2.8-phase2-privacy-review.md) were left to the tasks that
can close them; each of those tasks must close its finding before it is done. 13 and 16 are closed
in the service and the platform layer; what is left of them is the UI host's part and checks on
real machines:

| Finding | What is open | Tracked in |
| --- | --- | --- |
| 11, the installed-binary check | **Closed** for unsigned builds; written, and not yet exercised, for signed ones (§6.1, "The installed-binary check"; `iamb_platform::installation`; the strict review, `docs/reviews/finding-11-security-review.md`, and its "Fixes"): the peer must be a member of the process's own installation; the process's signing state is read once at the start from its running code, or compiled in, and "could not tell" trusts no one; a signed installation requires the same signer (macOS: a requirement built from the team identifier and both bundle identifiers, checked on the code the audit token names, without network access; Windows: the same certificate thumbprint), an unsigned one the member's own path and file. Left: the signed path has never run; no signed build exists yet | The install smoke test (`iamb-desktop --check-service`, and the service's own refusal of a copy) on each OS; with the first signed builds, its signed run (on macOS also an unsigned UI host in place of the real one) |
| 13, screen lock and sign-out | **Done in the service** (§4.5, §5): `SessionEvents` in the platform traits, the mock and both OS crates; every event forgets the remembered values and disturbs the hand-off; `FormValues`, the wire copy of a submit and clipboard snapshots overwrite their bytes before they are freed. Tests on the mock (`tests/panels.rs`), a Windows unit test of the sign-out messages, a macOS main-loop test of the notifications (`tests/session.rs`). Left: a lock, user switch and sign-out on a real Windows and Mac (`docs/compatibility-run.md`); the IPC frames that carried values (serde's buffers) are freed without being overwritten | Task 3.5: the manual checks on both systems |
| 16, Iamb's own panels on macOS | **Done in the service** (§4.5, ADR-0013 addendum): `PanelFocus(panel, shown)`, with a lease and the UI-host-gone fallback; keys typed while a panel has the keyboard reach neither the engine nor the hand-off (`tests/panels.rs`, `tests/focus_order.rs`). **The form panel sends it** (task 3.5, `apps/desktop/src-tauri/src/form.rs`): before the panel takes the keyboard, renewed every 10 seconds, and `shown: false` when the panel resigns the keyboard (a click into the app behind it) and before `FormSubmit` and `FormCancel` (`apps/desktop/src-tauri/tests/form.rs`). **The palette sends it too** (task 3.6, `apps/desktop/src-tauri/src/palette.rs`): the same order, and before `ExpandSnippet` and `ReopenLastForm` (`apps/desktop/src-tauri/tests/palette.rs`). Left: the form panel and the palette must become non-activating panels on macOS (a function in `iamb-platform-macos`, `apps/desktop/README.md`), and a check on a Mac that keys typed into the panels do not expand (`docs/compatibility-run.md`, F2 and P3) | The macOS panel function, and the Mac check |

## 9. Sync

The architecture fixes the frame ([ADR-0016](decisions/0016-sync-frame.md)). Task 4.1 fills it in:
[`sync-design.md`](sync-design.md) has the server's tables and policies, the protocol, the rules for
every conflict (`C1` to `C25` on the server, `D1` to `D17` on the device), the threat model for
sharing and 92 conflict scenarios with their outcomes, which are also in `spec/sync/scenarios.toml`
for the simulation of task 4.4 ([ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md),
[ADR-0028](decisions/0028-data-model-for-sync.md),
[ADR-0029](decisions/0029-server-readable-content.md)). In short: the last write is the one the
server accepts last, by its own counter and clock; a write replaces the whole row, and a stale one
records a conflict that offers the other version back; a delete is a write like any other; a device
never writes a pulled row over an unsent change, and fetches a row it skipped once the change is
settled; a change the server refuses is kept on the device for the user; the writes of one profile
happen one after the other on the server; after a restore of the server from a backup, devices send
back every row they hold in a revision the backup lacks, whatever was written since, after what took
access away has been taken away again, and only the operator begins the new epoch; a device removes
other people's folders at a sign-out only on the server's word, and sets them aside otherwise,
because a restore can end its session before it has given back what it holds; a session is accepted
only when it was opened the way Iamb signs in; and the server stores synced content readably in
version 1 and never interprets it. The frame:

- The server is a Postgres database with row-level security, reached over HTTPS and a live-update
  channel, hosted on Supabase.
- **Change feed.** The server keeps one ordered change feed per user. An entry is either a row
  change the user may see, or a visibility change: `folder_visible(folder)` when a share or
  membership gives the user access, `folder_revoked(folder)` when access ends. `sync_state` is the
  user's position in that feed; the server issues each position and knows, per device, the last one
  it issued and the one presented for it, which is how it tells a device from a copy of its data
  directory without mistaking a repeated call for one.
- **Pull.** The agent reads the feed from its position. A row change is applied through
  `ApplyRemote`. `folder_visible` triggers a full fetch of that folder, whatever the rows'
  revisions, which is how a newly shared folder appears within the 10-second target.
  `folder_revoked` triggers `PurgeFolder`, which removes the folder, its snippets and its cached
  attachments from the device.
- **Push.** The store writes every local change to `outbox` in the same transaction as the change,
  with the revision it was based on. The agent pushes in order (one change per row, parents before
  what refers to them). The server applies last write wins per row and keeps the overwritten version
  in a history table. When the base revision is older than the server's row and another device
  changed the row's content in between, the server also records a conflict; the device lists
  conflicts (`Conflicts`) so the user can restore the other version.
- Deletes are soft and are purged after 30 days on the server and on devices that have read past
  them.
- **Attachments.** Rows sync first. The agent then downloads every attachment of the effective
  library that the device lacks (`MissingAttachments`), within the size limits set in task 4.6, so
  snippets work offline after the first sync. It writes the bytes to `attachments/` itself
  (temporary file, hash check, rename) and tells the service with `AttachmentAvailable`. Uploads go
  the other way under the same path scheme.
- When a rich snippet is expanded, the service resolves `attachment:` references while it builds the
  clipboard content: HTML with images embedded as data URIs, plus plain text; a snippet that is only
  one media file is also offered as a file (a copy named after it in `run/paste/`, emptied when the
  worker starts). A missing attachment falls back to the plain-text rendering with a `Diagnostic`
  (`attachment_missing`). Task 3.8 keeps the bytes in `attachments/<profile id>/<sha256>`
  (`iamb-store::attachments`: types read from the bytes, images up to 5 MiB, media up to 25 MiB,
  checked against the hash before use), links each rich snippet to the attachments it references,
  and carries them in a JSON export; the editor adds one with `PutAttachment` (`iamb_service::rich`;
  `docs/paste-fidelity.md`).
- Plan features reach the device as `Entitlements` from the agent, stored by the service
  (`SetEntitlements`) and read by the UI.

## 10. Desktop UI

[ADR-0017](decisions/0017-ui-host.md) fixes the shape; task 3.1 designs the screens.
The screens, flows, copy and design tokens are in [`docs/ux/spec.md`](ux/spec.md).

- `iamb-desktop` is a Tauri 2 app. It starts hidden at login and stays resident whether or not the
  tray icon is shown, because the fill-in form and the palette depend on it.
- It keeps two small windows created but hidden, the fill-in form and the search palette, so they
  appear within their targets (150 ms and 100 ms). On macOS they are non-activating panels, which
  needs a small amount of native window code in `iamb-desktop`. The library window is created on
  demand and destroyed when closed. All three load one page; the front end picks its view by the
  window's label. On Linux (development only) the library window and the form share the
  palette's running web content process, without which a cold open spent about 600 ms starting
  one (`docs/test-strategy.md` §8.1).
- One bridge thread holds the connection to the service endpoint with the role `ui`, reconnects
  with backoff (200 ms, 500 ms, 1 s, then every 2 s) while the service is away, and reports the
  state to the tray menu and the front end. The front end calls a small set of typed Tauri
  commands (`apps/desktop/src-tauri/src/contract.rs`) and hears two events, `iamb:status` and
  `iamb:event`; focus tokens and form values the panels do not show stay on the Rust side.
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
| 3.5 Fill-in form | Form panel in `iamb-desktop`; Forms messages; focus tokens in the OS crates; form memory in `iamb-service`; sending `PanelFocus` from the form panel, and the manual lock and sign-out checks (findings 13 and 16 of the 2.8 review, §8 "Open findings"; the service's part is done) |
| 3.6 Search palette | Palette panel in `iamb-desktop`; `iamb-core::search`; `Hotkeys`; `PaletteRequested`; sending `PanelFocus` from the palette (finding 16 of the 2.8 review, §8 "Open findings"; the service's part is done) |
| 3.7 Onboarding | `apps/desktop/src/onboarding/`, `GetPermissions`, `SetTestField` |
| 3.8 Rich text snippets | `iamb-core::rich`, `Paste` action, clipboard building in `iamb-service`, attachments in `iamb-store` |
| 3.9 Settings and diagnostics | `apps/desktop/src/settings/`, Settings and Diagnostics messages |
| 3.10 Library health | `iamb-store::health` (conflicts, shadowing, broken references, unused), `Health` message |
| 3.11 Accessibility and polish | `apps/desktop` |
| 3.12 Review | `apps/desktop`, `iamb-desktop` |
| 4.1 Sync design | `docs/sync-design.md`, `spec/sync/scenarios.toml` (checked by `iamb-testkit::sync_scenarios`); extends §7 and §9 |
| 4.2 Server schema and policies | `server/migrations/`, `server/tests/` |
| 4.3 Security review | `server/` |
| 4.4 Sync client | `iamb-sync` (library), `outbox` and `sync_state` in `iamb-store`, Sync messages |
| 4.5 Account UI | `apps/desktop/src/account/`, the account endpoint, review of received snippets |
| 4.6 Media sync | `iamb-sync::media`, `attachments/`, `MissingAttachments`, `AttachmentAvailable` |
| 4.7 Billing readiness | `server/migrations/` (plan and entitlement tables), `Entitlements`, `entitlement` table |
