# Security and correctness review of the beta-gap commits (tray menu, restart, UI host, shortcuts)

Date: 2026-11-22 · Commits reviewed: `73ec4cd..11d0f77` (`010ad61`, `838fe73`, `468f9e0`,
`11d0f77`) · Reviewer: independent, did not write the code and judged it from the code, not from
the commit messages

Scope: `crates/iamb-service/src/{main.rs,paths.rs,supervisor.rs}`,
`crates/iamb-service/src/worker/{mod.rs,engine.rs,handlers.rs}`, the three new service tests,
`apps/desktop/src-tauri/src/{onboarding.rs,menu.rs,settings.rs,palette.rs}`,
`apps/desktop/src-tauri/src/app/{tray.rs,onboarding_commands.rs,settings_commands.rs,palette_panel.rs,mod.rs}`,
`apps/desktop/src-tauri/tests/restart.rs`, the shortcut recorder
(`apps/desktop/src/library/settings/`), judged against `docs/architecture.md` §3, §4.5, §6.1, §8,
ADR-0008, ADR-0011, ADR-0021, ADR-0023 and `docs/ux/spec.md` §4.1, §5.1.

The review changed no file of the tree and added this one. Everything was run on Linux, against
the mock platform. Nothing here was run on macOS or Windows; each finding says whether it was
shown by a run or by reading.

## 1. Verdict

Not ready to ship on macOS; two fixes are needed before the beta, one more should go with them.

1. **High (macOS, by reading):** since `468f9e0` the helper app, which holds the user's
   Accessibility and Input Monitoring grants, starts another program as its own child: whatever
   `--ui-host` names, or whatever sits at `…/Contents/MacOS/iamb-desktop` relative to the helper.
   A child started this way is treated by the system's permission checks as the program that
   started it. Any process of the user can therefore borrow the helper's keyboard grants, and
   the real UI host (web view, network) runs with them too, which the process table of §3 and
   ADR-0021 rule out (finding 1).
2. **Medium (shown by a run):** the "fill in again" shortcut runs against the focus the engine
   last heard of, not the focus at the press. Pressed right after a switch into a password field
   or an excluded app, the form opens with the last answers, for the window the user just left
   (finding 2). The fix is three lines and was tried.
3. **Medium (by reading):** "Never expand in *App*" is named when the pointer reaches the icon,
   not when the menu opens, and is kept without a time limit; the label and the app acted on can
   differ (finding 3).

Seven low findings follow (4 to 10); one of them, 10, is a worker process that never exits,
found because the new UI-host test leaves one behind.

The rest held: the restart request file cannot be used by another user or a sandboxed app, the
supervisor's side of it survives a directory, a symbolic link and a FIFO in its place, the UI
host inherits neither the worker's pipe nor the lock, the two shortcuts are refused in password
fields, excluded apps and Iamb's own panels once the verdict is in, pause by shortcut clears the
buffer, and no new log line carries typed text or snippet content.

## 2. Findings

### High

#### 1. The helper starts an arbitrary program as its child, which then counts as the helper (macOS)

Where: `crates/iamb-service/src/main.rs:79-83` (`--ui-host PATH`), `main.rs:180-188`
(`ui_host`), `crates/iamb-service/src/paths.rs:167-174` (`ui_host_beside`),
`crates/iamb-service/src/supervisor.rs:305-310` (`Command::new(&program.exe)`).

Shown by: reading. Not run on a Mac (section 4).

What happens. On macOS the keyboard grants belong to the signed helper (ADR-0021). A program
that the helper starts with a plain spawn has the helper as its responsible process, and the
system's checks for Accessibility and Input Monitoring answer for the responsible process. The
commit knows this in the other direction: `Launch::Open` in
`apps/desktop/src-tauri/src/onboarding.rs:385-395` exists so that the helper "runs as itself …
rather than as a child of the UI host". The supervisor does not apply it to the program it
starts.

Scenarios, all from a process of the same user that has no keyboard grant of its own:

- a) `open -n "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app" --args
  --supervise --ui-host /path/to/program --data-dir /tmp/other`. A data root of its own means a
  lock of its own, so the running supervisor does not stop it (`supervisor.rs:347`). The helper
  starts `/path/to/program` (`main.rs:182` takes the path as it is: no check that it is a file,
  that it is Iamb's, or that the build is a development build). The program reads the keyboard
  and posts keys under the helper's grants, with no prompt.
- b) No flag needed: copy the helper bundle (its signature stays valid) to
  `X.app/Contents/Library/LoginItems/Iamb Service.app`, put any executable at
  `X.app/Contents/MacOS/iamb-desktop`, start the copy. `is_login_item_helper` is true, so it
  supervises by default, and `ui_host_beside` (`paths.rs:168-172`) resolves to the planted file,
  which `main.rs:184` only tests with `is_file()`.
- c) The intended case: the real `iamb-desktop` is started as the helper's child and runs with
  the helper's grants. §3's table says the UI host sees no keystrokes and is the process with
  network access; §8 rule 5 and ADR-0021 ("a different program with its own, irrelevant, trust
  state") rest on the two being separate to the system. A flaw in the web view's content would
  then sit one step from the keyboard.

The threat model (§8: "does not defend against malware already running as the same user, which
can observe the keyboard directly") does not cover this on macOS: there a process of the same
user cannot observe the keyboard until the user grants it, and the helper hands its grant on.

On Windows and Linux there is no grant to borrow; a) and b) give a same-user process nothing it
does not have.

Fix.

1. `--ui-host` is a test and development switch (the three tests that use it pass
   `--platform mock`). Accept it only with `--platform mock` or in debug builds; in a release
   build with the native platform answer with the usage error. In `ui_host()`:
   `UiHostChoice::Path(p) if platform == PlatformChoice::Mock || cfg!(debug_assertions)`.
2. On macOS never spawn the UI host from the helper. Start it through the system, as the UI host
   starts the helper: `/usr/bin/open -g -W -a <Iamb.app> --args --data-dir <root>` (absolute
   path of `open`; `-W` lets the supervising thread wait for the end). The app is then its own
   responsible process, so neither the real UI host nor a planted one inherits anything. `open`
   does not pass the exit status on, so tell a quit from a crash another way: the UI host leaves
   `run/desktop.quit` on a deliberate exit (tray "Quit", "another UI host is running") and the
   supervisor restarts only when the marker is missing; remove the marker before each start.
3. Before starting it, check that the outer bundle is Iamb's: in `iamb-platform-macos` (where
   `unsafe` lives), a static code check of `Iamb.app` against "same team identifier as the
   running helper". Without signed builds, at least require `exe` to be absolute and the outer
   bundle's identifier to be the main app's.
4. Add to `docs/compatibility-run.md`: from the installed helper, a child that calls
   `CGPreflightListenEventAccess()` must get `false` when it has no grant of its own. This is
   also the check that confirms or refutes this finding.
5. Add a test that could fail for this reason: `--ui-host` with `--platform native` in a release
   build is refused.

Outside the diff, for the owner: scenario a) without `--ui-host` already lets a same-user
process start a second, granted helper on a data root of its choosing (`--data-dir`,
`IAMB_DATA_DIR`), whose library it wrote. That helper types what the library says. It reads
nothing back (rule 1 holds), but it is synthetic input under the helper's grant. Worth a
decision: the installed helper on macOS could refuse a data root other than the default unless
the build is a development build.

### Medium

#### 2. The "fill in again" shortcut runs against the last reported focus, not the focus at the press

Where: `crates/iamb-service/src/worker/engine.rs:669-673`
(`self.on_request(Explicit::Reopen, None)`); compare the palette's shortcut at
`engine.rs:658-668`, which reads `platform.focus_token()`.

Shown by: a scratch test on the mock (kept outside the tree), with `hold_focus_reports(true)`,
the same device `tests/focus_order.rs` uses for the 2.8 review's finding 5.

What happens. A hotkey event carries no key target (`listener/thread.rs:346` on Windows,
`listener/core.rs:296` on macOS, `mock/mod.rs:754`), so the check of `engine.rs:634-644` that
keeps typed keys away from a window not yet reported does not apply to it. With no focus token
the request is not parked (`engine.rs:818-827`) and runs under the verdict of the window before.

Run: fill in a form in the editor, hold the focus reports, focus a password field (or a window
of an excluded app, or another ordinary window), press the shortcut. Result in all three cases:
`FormRequested` is sent with `last_values`, `focus` naming the editor (the window just left),
and no `refused_secure_field` or `refused_excluded_app`. After a submit nothing was typed into
the password field (the submit is parked for the old window, then judged by that window's own
verdict), so no text reaches a secure field. What goes wrong:

- the form opens although the user is in a password field or an excluded app, where the same
  press a moment later is refused; the last answers are sent to the UI host and shown;
- the answers are typed into the window the user left (the UI host hands the focus back to the
  form's token), not "in the app you are in" (Settings help text, spec §5.1); in the mock run
  the submit waited for the old window and timed out with nothing typed.

This is the shortcut's version of the known race of the tray's "Fill in again"
(`ReopenLastForm` carries no focus token).

Fix (tried: the three race cases then send no form while the report is late, and
`tests/shortcuts.rs` still passes):

```rust
Routed::Hotkey(id) if id == REOPEN_FORM_HOTKEY => match self.shared.platform.focus_token() {
    Ok(token) => self.on_request(Explicit::Reopen, Some(token)),
    Err(_) => self.shared.diagnostic(coded("refused_unknown_focus")),
},
```

The request is then parked until the `FocusChanged` for the window of the press was delivered
and held by the engine for its verdict, the order duty §4.5 already describes. Add the three
cases to `tests/shortcuts.rs` or `tests/focus_order.rs`. The same token closes the tray's race
if `ReopenLastForm` is given an optional `focus`, as `ExpandSnippet` has.

#### 3. "Never expand in *App*" is not taken when the menu opens, and the label can differ from the app acted on

Where: `apps/desktop/src-tauri/src/app/tray.rs:308-330` (`refresh_exclusion`,
`show_exclusion`), `tray.rs:338-349` (`toggle_exclusion`), `tray.rs:366-384` (`on_icon_event`).

Shown by: reading. The tray needs Windows or macOS.

What happens. The target is read on `TrayIconEvent::Enter` only, off the main thread, after one
request to the service, and is kept in `Tray::target` until the next `Enter`. The palette's
target for the same menu has a time limit and a second look on `Move`
(`palette.rs:430-443`, `TRAY_CAPTURE_TTL`); the exclusion item has neither.

- Stale target: the menu opened without a new `Enter` (by keyboard; or the pointer rested on the
  icon while the user switched apps, which gives `Move`, not `Enter`) offers the app of the last
  hover, however long ago. The label names that app, so the user can see it, but it is not "the
  app that was in front when the menu opened" (spec §4.1). In the direction "Expand in *App*
  again" a click lifts an exclusion the user set for privacy.
- Label and action apart: `show_exclusion` sets the text, then stores the target; the refresh
  may finish after the menu is drawn. A click then toggles the newly stored app while the open
  menu still shows the old name (whether an open native menu repaints was not checked). The
  window is the one request to the service: short, but §8 asks that an exclusion never fail
  silently.
- Wrong app on Windows: when the taskbar or the overflow flyout already has the foreground at
  `Enter`, the front application is the shell, and the item reads "Never expand in explorer".
  `tray_pointer` knows this case ("a press on the icon may already have brought the taskbar to
  the front"); the exclusion item does not.

Iamb itself is not offered: the UI host's own process is filtered by pid (`tray.rs:313`) and
Iamb's other processes by `own` (`menu.rs:188-190`). That part holds.

Fix.

1. One capture for the whole menu: take the front app in the same place and under the same
   rules as `tray_pointer` (on `Enter`, and on `Move` when the capture is older than
   `TRAY_CAPTURE_TTL`), and hide the item when the capture is older than the limit at the click.
2. Make the click act on what the label showed: keep the label text with the target and, in
   `toggle_exclusion`, do nothing (and log the code) when the item's current text is not
   `target.label()`; or set text and target under one lock and refuse a click that arrives
   while a refresh is in flight.
3. On Windows hide the item when the front window is the shell's tray or overflow window.
4. Tests for `menu.rs` cover naming only. Add one for a stale capture and one for a refresh
   that finishes between drawing and click (the controller can take a clock, as the palette's
   does).

### Low

#### 4. A shortcut may be a chord that Iamb itself sends, an everyday editing chord, or a character

Where: `crates/iamb-platform/src/hotkeys.rs:133-151` (`Chord::new`),
`apps/desktop/src/library/settings/model.ts:227-240` (`recordChord`),
`crates/iamb-service/src/worker/handlers.rs:974-1011` (`register_hotkey`). The rule is older
than these commits; they put two more settings behind it, one of which toggles a privacy
control.

By reading. Any letter, digit or named key with Ctrl, Alt or Meta is accepted, by the recorder
and by the service: `Ctrl+V`, `Meta+V`, `Ctrl+C`, `Ctrl+Z`, `Ctrl+Backspace`. The system hotkey
API swallows a registered chord everywhere.

- The paste chord: an expansion that pastes sends that chord itself. Registered as
  `pause_hotkey`, each such expansion would toggle the pause and its paste would not arrive
  (whether a registered hotkey also catches synthetic keys was not checked on either system).
- AltGr and Option: on Windows AltGr is Ctrl+Alt, so a press of AltGr+Q in the recorder is
  stored as `Ctrl+Alt+Q` (`e.ctrlKey && e.altKey`), after which `@` cannot be typed on a German
  layout, `ą` (`Ctrl+Alt+A`) on a Polish one; on macOS `Alt+letter` types a character. The
  doc comment "a valid chord cannot be typed by accident" does not hold for these.

Fix: refuse in `Chord::new` (so that the service, not only the recorder, enforces it) the
clipboard and undo chords (Ctrl or Meta alone with C, V, X, Z, A, Y; Shift+Insert and
Ctrl+Insert on Windows), or at least in `register_hotkey` the chord the injector uses for paste
on this system; on Windows require a modifier besides Ctrl+Alt for letters and digits, or
refuse `Ctrl+Alt+key` when the layout gives that key a character; in `recordChord` refuse when
`getModifierState("AltGraph")` is true, and on macOS Option alone with a letter or digit. Add
the refusals to `tests/shortcuts.rs` and the recorder's tests.

#### 5. The restart request: the UI host's side, and what a restart does to the worker

Where: `apps/desktop/src-tauri/src/onboarding.rs:482-520`,
`crates/iamb-service/src/supervisor.rs:200-202, 251-278`.

Shown by a run for the supervisor's side, by reading for the UI host's.

- a) `std::fs::write(run.join(RESTART_REQUEST), b"")` (`onboarding.rs:491`) follows a symbolic
  link and truncates what it points to, waits for ever on a FIFO, and fails on a directory. Only
  the user can put those into `run/` (0700), so this is robustness, not a boundary. The
  supervisor's side is safe: `remove_file` removed a link (target untouched) and a FIFO, and
  ignored a directory, in a run.
- b) When writing the request fails for any reason (a directory in its place, a full disk), the
  function returns before it starts the supervisor (`onboarding.rs:490-494`). With no
  supervisor running, starting it is all that is needed. Start it anyway and report the write
  error only when a supervisor already runs.
- c) The exit status of `open` is thrown away (`onboarding.rs:516-518`): a helper the system
  refuses to open gives "Start it again" that does nothing and says nothing. Wait for `open`
  (it returns at once) and turn a non-zero status into `unavailable`.
- d) The worker is killed outright (`supervisor.rs:268`). Killed during an expansion that
  pastes, the clipboard is not restored (rule 8) and a held modifier is not released. Killed at
  any time, the pause is lost: `paused` lives only in the engine thread (`engine.rs:301`), so a
  user who paused Iamb gets a worker that expands again, after "Restart the helper", and as
  before after a crash. Fix: end the worker by closing its standard input (it then stops in
  order, `main.rs:306-318`), kill only after one second; the supervisor asked, so it can tell
  this exit from "stopped for good". Keep the pause across a restart (a file in `run/`, or a
  setting).
- e) Use `OpenOptions::new().write(true).create_new(true)` (ignore `AlreadyExists`) with
  `O_NOFOLLOW` on Unix for the request, so that a) cannot happen.
- f) The desktop's `create_private_dir` (`apps/desktop/src-tauri/src/paths.rs:123-136`) does
  not tighten a directory that exists and sets no access list on Windows; the service's does
  both. Older than these commits; the UI host can be the first to create `run/`. With the
  default root nothing is exposed. Use the service's rule in both.
- g) Rare: a request written while the old supervisor is exiting (its worker stopped with
  status 0) meets a new supervisor that finds the lock still held and exits; nothing runs and
  the request stays. A second click works. A new supervisor could retry the lock for a second.

The channel itself is a file without the peer verification that ADR-0023 asks of the control
pipe. The module docs say so and the argument holds for what the file can do today (a restart).
It is still a departure from an accepted ADR and is written down only in a module comment:
§3.2, §3.3 (the table of `run/`) and §8 do not mention `run/restart-worker`. Add it there and
add a note to ADR-0023, and do not let the file grow a second meaning (stop, update) without
the pipe.

#### 6. Supervising the UI host: a start that can never work, and a UI host that is ended on purpose

Where: `crates/iamb-service/src/supervisor.rs:295-339`.

Shown by a run (the restart at once after SIGTERM), the rest by reading.

- A UI host that fails the same way every time is started again every 30 s for ever: a missing
  web view runtime, or a data root that is not UTF-8 (`apps/desktop/src-tauri/src/main.rs:57-59`
  turns it into a usage error, status 1, while the supervisor passes the root as it got it).
  Give usage errors a status of their own (64) that ends supervision like 0, and stop after,
  say, five starts that each lived under a second.
- Any ending but status 0 restarts at once (the first delay is 0): ending it from the task
  manager, and an installer or updater that ends `iamb-desktop` to replace it. The Windows
  installer stops the service tree first (`release/windows/hooks.nsh`, `taskkill /T`), which
  now includes a UI host the supervisor started; whether the bundler's own step for
  `iamb-desktop.exe` runs before or after that hook was not checked. ADR-0023's update flow
  will need "stop supervising the UI host" as a message.
- macOS: see finding 1; a UI host started through the system is not a child, so this loop
  changes there anyway.

#### 7. Programs found through `PATH`, and relative paths

Where: `apps/desktop/src-tauri/src/settings.rs:417-426` (`"open"`, `"explorer"`, `"xdg-open"`),
`crates/iamb-service/src/main.rs:182` (`--ui-host` as given),
`apps/desktop/src-tauri/src/onboarding.rs:409-423, 465-477`.

By reading. "Open the log folder" starts `open`, `explorer` or `xdg-open` by name, while the
restart code two files away uses `/usr/bin/open`. The folder is one argument and no shell is
involved, which holds. Use `/usr/bin/open`, `%SystemRoot%\explorer.exe` (from
`GetWindowsDirectoryW`), and keep `xdg-open` for Linux. `launch_candidates` trusts
`current_exe()` to be absolute; on macOS it is the path the program was started with, which can
be relative or a link, and `Launch::Direct("iamb-service")` without a directory would be looked
up in `PATH`. Canonicalize `current_exe()` once in `LaunchSupervisor::installed` and in
`main.rs:198`, and refuse a result that is not absolute.

#### 8. Smaller points about the shortcuts

- A refused or empty "fill in again" still renews the 30 minutes the answers are kept
  (`engine.rs:837-840` sets `values_touched` before the engine decides). With a shortcut that is
  one key press, in a password field too. Renew only when the engine opened the form. Older than
  these commits for `ReopenLastForm`.
- The pause shortcut is a toggle whose only sign is the icon. With the icon hidden
  (`ui.tray_icon`) or no UI host, a second, accidental press resumes without a sign. Spec §4.11
  allows no notice for it; an owner decision (a separate "pause" and "resume", or a notice for
  this one case).
- A press with a panel of Iamb's up toggles the pause (run: `paused = true`). Harmless, and as
  the module docs say ("hotkeys go on as before").

#### 9. What the new tests do not exercise

Each new test passes and each can fail for the thing it names. What is missing is the risky
half:

| Test file | Not exercised |
| --- | --- |
| `tests/shortcuts.rs` | A press in a password field, an excluded app, with a panel up; a press before the focus report (finding 2); that pause by shortcut clears a half-typed abbreviation; that expansion works again after the resume; a chord from finding 4 |
| `tests/supervisor_restart.rs` | The real flow: a second supervisor started while one runs must leave the request to the first; a directory, link or FIFO in the request's place; Unix only, so nothing on Windows |
| `tests/supervisor_ui_host.rs` | The default path on the native platform (only `ui_host_beside`'s arithmetic has a unit test); `--ui-host` refused in a release build (finding 1); a UI host ended by a signal; that the UI host inherits no descriptor and that the worker still follows a killed supervisor while the UI host lives (the comment in `Drop` says so, nothing asserts it, and finding 10 is the case where it does not) |
| `apps/desktop/…/tests/restart.rs` | `Launch::Open`; a `run/` that exists with wider permissions; a failed write (finding 5b); the stale request left by `a_supervisor_that_cannot_start_is_unavailable` |
| `menu.rs` unit tests | Only naming; nothing on when the exclusion target is taken (finding 3) |

All of the "not exercised" rows for the service were run by hand or as scratch tests for this
review (section 3) and behave as section 3 says, except where a finding says otherwise.

#### 10. A worker whose socket file is gone never finishes stopping; the new UI-host test leaves such workers behind

Where: `crates/iamb-service/tests/supervisor_ui_host.rs:103-114` (`Drop`),
`crates/iamb-ipc/src/linux.rs:209-220` and `crates/iamb-platform-macos/src/endpoint.rs:319-329`
(`Listener::close`), `crates/iamb-service/src/worker/mod.rs:866-893` (`Worker::stop`). The
cause is older than these commits; the new test file is what shows it.

Shown by a run, twice: after the first run of the three new test files one
`iamb-service --worker` of a deleted temporary data root was still alive twelve minutes later;
and by hand: start the supervisor, remove `run/service.sock`, kill the supervisor.

What happens. `Listener::close` wakes the thread blocked in `accept` by connecting to the
socket's path. When the path is gone the connect fails, the `iamb-accept` thread stays blocked,
and `Worker::stop` waits for it for ever. The worker's log ends with "the supervisor is gone;
stopping" and "store closed", never "worker stopped"; the process stays, with no supervisor
left to kill it. Its listener is stopped first (`platform.stop()`, `mod.rs:869`) and the store
and endpoint locks are released, so it reads no keys and does not block the next worker; what
is skipped is the zeroing of the hand-off and the key queue at `mod.rs:890-891`.

- In the tests: `supervisor_ui_host.rs` kills the supervisor and lets the temporary directory
  go at once ("the worker, which follows the supervisor by itself"). When the directory is
  removed before the worker closes its endpoint, the worker hangs. `supervisor_restart.rs`
  waits until the endpoint stops answering and does not leak. Each run can leave a process
  behind on a developer's machine and in CI.
- In the product: a data folder or `run/` removed while Iamb runs (a reset by hand, a cleanup
  tool), then any stop that is not a kill: the supervisor gone, a `Shutdown`, and the orderly
  stop finding 5d proposes.

Fix. In `Drop` of the test's `Supervisor`, wait for the worker to be gone before the directory
is (as `supervisor_restart.rs` does, by the endpoint, or by the worker's pid from the log). In
`Listener::close`, do not depend on the path: accept with a timeout and look at `closed`, or
wake the thread through a pipe it polls beside the socket. In `main.rs`, bound the stop: when
`worker.stop()` has not returned after a few seconds, zero the two buffers and exit the
process.

## 3. What held

Shown by a run unless marked (reading).

- **The run directory and the request.** `run/` and the data root are 0700, the request file
  sits inside. Another local user cannot reach it; a sandboxed or low-integrity process cannot
  write there (reading, for macOS and Windows). The file carries nothing and is only removed.
- **Two supervisors.** A second supervisor finds the lock taken and exits with status 0 before
  it looks at the request (`supervisor.rs:347-350`); the running one took the request within
  200 ms and replaced the worker. A request left from before is dropped at start and restarts
  nothing.
- **Odd things in the request's place.** A directory: never taken, worker untouched, no stall.
  A symbolic link to a file: link removed, file untouched, one restart. A dangling link and a
  FIFO: removed, one restart. Thirty requests in three seconds: about fifteen restarts, no backoff,
  the supervisor stayed up; this is a same-user action, no more than killing the worker.
- **`watch` after the change.** The heartbeat deadline is still enforced; a request is looked
  for between beats; no busy loop when the deadline is reached (reading).
- **What the UI host inherits.** Standard input and output are the null device; no descriptor
  of the worker's pipes or of the lock reaches it. Killing the supervisor with the UI host
  alive, the worker logged "the supervisor is gone; stopping" and stopped. On Windows the same
  rests on the standard library not marking the parent's pipe ends inheritable (reading).
- **Who can pass `--ui-host`.** Only the command line of `iamb-service`. The UI host passes a
  fixed list (`--supervise --no-ui-host --data-dir <root>`), the data root as one argument
  after its flag (and after `--args` for `open`), no shell anywhere. The Windows login item is
  `"<path>" --supervise`. No environment variable or file feeds it. (What a process of the user
  can do with the command line on macOS is finding 1.)
- **Path arithmetic.** `ancestors().nth(6)` is the main bundle's `Contents`. On Linux
  `current_exe()` resolves links: started through a link from a directory holding a planted
  `iamb-desktop`, the supervisor started the one beside the real file.
- **Exit codes.** Status 0 ends supervision (quit, or another UI host holds `desktop.lock`), a
  panic exits with 70 and is restarted, SIGTERM is restarted at once.
- **The shortcuts and the engine.** With the verdict in, "fill in again" by shortcut is refused
  in a password field (`refused_secure_field`), in an excluded app (`refused_excluded_app`) and
  with a panel up (`refused_own_window`). Pause by shortcut goes through the same function as
  `SetPaused`, clears the buffer (`br`, pause, resume, `b␠` did not expand) and expansion works
  again after the resume. "Fill in again" under pause opens the form, as §4.5 intends for
  explicit requests. Only the UI role may set a shortcut (`not_permitted` for the other two
  roles). A chord another id holds is `conflict`, the old chord stays. The hotkeys use the
  system's hotkey API; the listener stays observe-only (reading).
- **Logging.** The new lines carry process ids, exit statuses, setting names, error codes and
  I/O error kinds; no chord, no application id, no typed text, no snippet content (reading:
  `supervisor.rs:311-336, 376-415`, `worker/mod.rs:925-931`, `handlers.rs:1001`,
  `tray.rs:397-469`, `onboarding.rs`).
- **Commands.** `app_restart_service` answers the library, palette and setup windows only;
  `settings_login_item`, `settings_about` and `settings_open_log_folder` the library window
  only. The folder opened is the UI host's own log folder, not an argument of the caller.
- **Iamb is never offered for exclusion** (finding 3, last paragraph).
- **Known and unchanged:** "Quit Iamb" ends the UI host only; the service goes on reading keys
  (`docs/user/coverage.md`, gap 7). Supervising the UI host does not change that: status 0 ends
  the supervision and nothing else.

Tests run, all passing: `cargo test -p iamb-service --test shortcuts --test supervisor_restart
--test supervisor_ui_host` (4, 3, 3), `cargo test -p iamb-service --lib -- supervisor paths`
(8), `cargo test -p iamb-desktop --test restart` (3). Ten scratch tests on the mock for the
shortcuts (secure field, excluded app, late focus report in three variants, buffer, pause,
panel, roles) and the hand runs above; none is in the tree.

## 4. What could not be checked

- **Anything on macOS.** That a child of the helper answers the system's keyboard-permission
  checks as the helper (finding 1 rests on the documented behaviour, not on a run); whether
  `current_exe()` there returns a link or a relative path; `open -g` when the helper already
  runs, when only a worker of it runs, and when the system refuses the helper; whether the
  system counts the worker or the supervisor as "the helper app is running"; what a UI host
  started as the helper's child does at logout.
- **Anything on Windows.** Handle inheritance into the UI host; `CREATE_NO_WINDOW` and the
  console of the supervisor and worker; the order of the installer's hook and the bundler's own
  stop of `iamb-desktop.exe` now that the supervisor restarts it; removing the request while a
  scanner holds it. Noticed while reading, outside the diff: `iamb-service` has no
  `windows_subsystem` attribute, so the login item may show a console window at every login;
  closing it would end the supervisor. Needs a look on a Windows machine.
- **The tray.** `Enter` and `Move` events, whether an open native menu repaints after
  `set_text`, and what the front application is while the taskbar or the overflow flyout has the
  foreground (finding 3).
- **Real hotkeys.** Whether a registered chord also fires for the injector's synthetic keys
  (finding 4); whether the listener sees the chord's key as a character on AltGr or Option
  layouts before the system swallows it, which would leave a character in the engine's buffer
  that the application never got (on the mock a press leaves the buffer as it is: `br`, press,
  `b␠` expanded). If it does, clear the buffer on every hotkey press.
- **Not run for lack of disk space:** the desktop crate's unit tests (`menu.rs`,
  `onboarding.rs`, `settings.rs`) and the front-end tests (`*.test.tsx`). Their sources were
  read.
- **The front end** was read for the recorder and the three new commands only; the React views
  were not reviewed for anything else.
