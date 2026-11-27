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

## 5. Re-verification of the fixes

Date: 2026-11-22 · Commits reviewed: `11d0f77..50c66da` (ten commits, "Fix (A)" 1 to 6 and
"Fix (B)" 7 to 10) · Same reviewer, same limits: everything was run on Linux, nothing on macOS
or Windows; each line says whether it was shown by a run or by reading.

### 5.1 Verdict

Not ready yet. Seven of the ten findings are closed (2, 3, 4, 5, 7, 9, 10) and three are partly
closed (1, 6, 8). Finding 1 is still open in one of its three scenarios, and the fixes brought
two new problems that should be fixed before the beta:

1. **Finding 1, scenario a, is still open in a release build (shown by a run).** The switch
   `--ui-host` is refused "in a release build on the native platform", but a release build
   also accepts `--platform mock`, and with it the switch is allowed. A release build of
   `iamb-service`, started as `--supervise --platform mock --ui-host <program>`, started the
   program as its child. **The condition is the one section 2 of this review proposed
   ("only with `--platform mock` or in debug builds"); that proposal was wrong**, and the fix
   followed it faithfully.
2. **New, medium (by reading): the fix for finding 3 can freeze the UI host.** A lock is held
   while menu calls wait for the main thread, and the main thread takes the same lock in the
   tray icon's pointer events (5.3, N1).
3. **New, medium (shown by a run): a quit marker left by another UI host turns a later crash
   into "quit".** A second start of Iamb while it runs (the Start menu, on Windows) leaves
   `run/desktop.quit`; when the supervised UI host crashes afterwards it is not started again
   (5.3, N2).

### 5.2 The ten findings

| # | Finding | Now | Evidence |
| --- | --- | --- | --- |
| 1 | The helper starts a program as its child | **Partly** | b and c closed by reading: on macOS the installed helper has the system open the app (`ui_host_plan` and `ui_host_program`, `main.rs:231-292`) and never spawns what is beside it; unit tests pass. a **open**: see below |
| 2 | "Fill in again" shortcut and the late focus report | **Closed** for the shortcut; the tray's request partly | Scratch tests re-run: in all three races (password field, excluded app, another window) no form is sent while the report is late; `tests/shortcuts.rs` 11 passed. Tray: N4 |
| 3 | "Never expand in *App*" | **Closed** in its rules, with a new problem | `menu.rs` capture rules read and its unit tests run (23 passed in the desktop unit-test binary built from this tree): one capture, time limit, click refused when the label is not the target's. New: N1 |
| 4 | Chords | **Closed** | `Chord::new` refuses the editing chords and the chords that type; `PutSetting` answers `invalid` (test passes); recorder tests 81 passed. Notes in 5.4 |
| 5 | Restart request and what a restart does | **Closed** | a, e: `create_new`, link, FIFO and directory cases pass in the desktop's `restart` test binary (6 passed). b: started anyway (test). c: `open`'s status is read (reading). d: the worker stops in order (test) and the pause is kept (test; edge cases N5). f: Unix shown, Windows by reading. g: the lock is tried for 1 s (unit test). Docs: §3.2, §3.3, §8 and a note in ADR-0023 are there |
| 6 | Supervising the UI host | **Closed** where the exit status is the UI host's own; partly on macOS | Status 64, SIGTERM and five quick failures end the supervision (unit tests, and `tests/supervisor_ui_host.rs` 6 passed). Behind `open` none of the three statuses can be seen, only the marker and the five quick failures; on Windows an end from the task manager is still restarted at once. N2 and N6 |
| 7 | `PATH` and relative paths | **Closed** | `/usr/bin/open`, `explorer.exe` in the Windows directory, `current_exe()` resolved in both programs, `--ui-host` resolved and must be a file (tests pass) |
| 8 | Smaller points | First **closed**, second **open** | A refused or empty "fill in again" no longer renews the 30 minutes (test). The pause toggle still has no sign when the icon is hidden; no decision recorded, and it matters more now that a pause outlives a login (N5) |
| 9 | Missing tests | **Closed** for the rows listed | All rows of the table have a test now, in the fix commits or in `cd55085` and `50c66da`. What is still untested is in 5.4 |
| 10 | A worker that never exits | **Closed** on Linux; macOS by reading | By hand again: socket file removed, then the supervisor killed, and the whole data root removed, then the supervisor killed: the worker logged "worker stopped" and was gone within 1.5 s both times. `a_worker_whose_socket_file_is_gone_still_stops` and the endpoint's unit test pass. No worker was left behind by the test runs |

**Finding 1, what is still open.**

- Where: `crates/iamb-service/src/main.rs:199-201` (`ui_host_switch_allowed`: `platform ==
  Mock || debug`), `main.rs:240` (`UiHostChoice::Path` is always `UiHostPlan::Child`),
  `crates/iamb-service/src/platform.rs:31-36` (a release build parses `mock`). The unit test
  `the_ui_host_switch_is_refused_by_a_release_build_on_the_native_platform` asserts that the
  mock platform with a release build is allowed.
- Shown: `cargo build -p iamb-service --release --locked` from this tree, then
  `iamb-service --supervise --platform native --ui-host <program> --data-dir <dir>` is refused
  with the usage error, and the same line with `--platform mock` logs "ui host started" and
  the program runs as the supervisor's child.
- On macOS the release helper also has to pass the data-root rule
  (`paths.rs:290-297`). That rule compares with a default built from the environment variable
  `HOME` (`paths.rs:104-107`). The caller of `open` chooses the helper's environment
  (`open --env HOME=…`), so "the default data root" is a directory of the caller's choosing
  with a free lock (by reading). Ending the running supervisor first frees the real one too.
- Fix: allow `--ui-host` in debug builds only, whatever the platform (the tests that use it
  run debug builds); refuse `--platform mock` in the installed helper of a release build, the
  mock being "development and tests only"; take the home directory for the data-root rule from
  the user database (`getpwuid`), not from `HOME`, in `iamb-platform-macos`. Change the unit
  test to assert the refusal for both platforms, add a test of the release binary
  (`cargo test --release` in CI, or a build-profile check), and add `--platform mock` to check
  H4 of `docs/compatibility-run.md`, which today passes with the hole open.

### 5.3 New problems

#### N1. Medium: the tray's capture lock is held across calls that wait for the main thread

Where: `apps/desktop/src-tauri/src/app/tray.rs:334-338` (`refresh_exclusion`), `359-360` and
`374-377` (`toggle_exclusion`), `288-290` (`render`), against `tray.rs:313-314` called from
`on_icon_event` (`tray.rs:424`). By reading; the tray needs Windows or macOS.

`MenuItem::set_text`, `text`, `Menu::insert` and `remove` run their work on the main thread and
wait for the answer (`run_item_main_thread!` in the menu module of the UI toolkit:
`run_on_main_thread(task)` then `rx.recv()`). The three functions above hold `Tray::capture`
while they make those calls from a worker thread. Tray icon events are delivered on the main
thread, and `on_icon_event` takes the same lock there. When a pointer event is handled between
the worker taking the lock and its last menu call, the main thread waits for the lock and the
worker waits for the main thread: the UI host stops for good (menu, form panel, palette,
library). The supervisor does not see it, since it watches the UI host by its end only. The
moment is likely, not rare: the capture finishes a few milliseconds after the pointer reached
the icon, while move events keep arriving. Before the fix the main thread took no lock that a
worker held across a menu call.

Fix: never hold `capture` across a menu call. Decide under the lock, release it, apply the text
and the insert or removal, then take the lock again and keep the result only if the ticket is
still the newest (the click already checks the label against the target, which covers the
gap). Or take the ticket inside the spawned task instead of in `on_icon_event`, so that the
main thread never takes the lock. A test with a host whose "menu call" waits for a thread that
takes the lock would have caught it.

#### N2. Medium: `run/desktop.quit` from another UI host hides a crash

Where: `apps/desktop/src-tauri/src/main.rs:165-168` (the UI host that finds the instance lock
taken leaves the marker), `crates/iamb-service/src/supervisor.rs:217-220` (`judge`: the marker
wins over every status). Shown by two runs.

- The UI host binary of this tree, started for a data root whose `desktop.lock` was held,
  logged "another ui host is running; exiting" and left `run/desktop.quit`.
- A supervisor with a stand-in UI host: `run/desktop.quit` created while the UI host ran, the
  UI host then killed with SIGKILL. Log: `ui host … ended (Quit); not restarted`.

So after any second start of Iamb while it runs (the Start-menu entry on Windows, where a
second process is started; a second copy or `open -n` on macOS), the next crash of the
supervised UI host is taken for a quit. The marker is removed only before a start. The same
holds for any process of the user that creates the file, and removing it at the right moment
brings a quit UI host back; both are same-user actions and no more than starting or ending the
UI host by hand.

Fix: the UI host that steps aside leaves no marker (status 0 is enough where the supervisor
sees it; behind `open` no second process arises, the system brings the running app forward).
Where the status is the UI host's own, read the status first and let the marker matter only
for a launcher. To make the marker say who quit, write the process id into it and, behind a
launcher, compare with the holder of `desktop.lock` noted at the start.

#### N3. Low: what is left of the data-root rule and of the bundle check (macOS, by reading)

- `HOME`: see finding 1 above.
- `check_main_app` (`paths.rs:249-275`) looks for `Contents/MacOS/iamb-desktop`, but `open -a`
  starts whatever `CFBundleExecutable` names; `plist_string` takes the first textual
  `<key>CFBundleIdentifier</key>`, also inside a comment, and reads no binary property list
  (the app would then never be started at login: check the real bundle in the smoke test).
  None of this is a boundary: a program the system opens is its own responsible process
  whatever the check says, and the comment says it is not a signature check. Until the
  signature comparison exists the check should not be counted as a protection anywhere.
- `-W` with an app that already runs waits for that app, which is what the supervision needs
  (the manual of `open`; not run).

#### N4. Low: the tray's "Fill in again" still runs for the wrong window when the shell was in front

Where: `crates/iamb-service/src/worker/engine.rs:895-911`. Shown by a scratch test.

The request is parked only when the last focus delivered is one of Iamb's windows. When it is
another program's (on Windows the click on the icon can bring the shell's taskbar to the front
before the menu), the request runs at once: with the taskbar's window reported last and the
hand-back to the editor not yet reported, `FormRequested` named the taskbar's window. Parked,
the request is released by the next window that is not Iamb's, whichever it is: a password
field next was refused (`refused_secure_field`), an unrelated third window got the form. No
text reaches a secure field in any of these; the form can open for a window the menu was not
for. The one-second wait itself does no harm: a request that nobody releases ends with
`request_focus_timeout` instead of `refused_own_window`. The fix is still the optional `focus`
on `ReopenLastForm` (the UI host knows the tray's target), which the commit left out to avoid
a protocol change; an optional field does not raise the protocol version (§6.1).

#### N5. Low: the pause file

Where: `engine.rs:349-371` (`keep_pause`), `engine.rs:376` (the start). Shown by scratch tests.

- Who can create it: only the user's processes (`run/` is 0700; the file is created new with
  mode 0644 inside it). A process of the user that creates or removes it changes the pause at
  the next worker start, and with a restart request at once. That is no more than `SetPaused`,
  but it does not pass the endpoint's peer check; architecture §8 should list `run/paused`
  with the two signal files.
- A dangling link in its place: pausing counts it as kept (`AlreadyExists`), but the start
  looks with `exists()`, which follows the link: the worker started **not paused** after a
  pause. A directory in its place: the worker starts paused, a resume cannot remove it (only a
  warning in the log), and the next start is paused again although the user resumed. Fix: at
  the start count anything that is there (`symlink_metadata`); when pausing and something is
  already there that is not a regular file, remove it and create the file; when resuming,
  remove a directory too.
- A stale file and a user paused without knowing: the file outlives a quit, a logout and a
  restart of the machine, which a pause did not before. With the icon shown the menu says
  "Iamb is paused". With the icon hidden (`ui.tray_icon`) or no UI host nothing says so and
  nothing expands; "Why didn't it expand?" has no entry for a pause. This is the fail-safe
  direction and what finding 5d asked for, but it is a behaviour the user guide does not
  mention (`docs/user/`), and it makes the open half of finding 8 (no sign of the toggle) an
  owner decision that should be taken now.
- When creating the file fails (a full disk) the pause is lost at the next restart with a
  warning in the log only; the status event could carry it.

#### N6. Not checked: the end of the UI host that the supervisor cannot read (macOS)

Behind `open -W` the supervisor sees only the marker. The marker is left when `app::run`
returns 0. Whether a quit by the system (logout, restart, a "quit" Apple event) returns through
that path was not checked; if it does not, the supervisor opens Iamb again during logout, at
once (the first delay is 0), which can hold a logout up. A SIGTERM from an installer or
`killall` cannot be told from a crash there either, so on macOS the "ended on purpose" rule of
finding 6 does not apply. Add both to the Mac checks beside H3.

### 5.4 Answers to the points asked about, and smaller notes

- **`create_new` for the restart file.** Holds: a link is neither followed nor truncated, a
  FIFO is not opened, a directory is an error and the supervisor is started anyway (run).
  "Something already there counts as given" is right, since the supervisor takes it by
  removing it.
- **The wake in `Listener::close`.** Linux: the `O_PATH` descriptor and the connect through
  `/proc/self/fd` work with the name and the directory gone (run). The descriptor is opened by
  path right after the bind, so a process of the user that swaps the file in between gets the
  wake-up; the 5 s limit in `main.rs` then ends the process. macOS poll pair, by reading: the
  listening socket is non-blocking, an accepted stream is made blocking again, the wake byte is
  never drained so every later `accept` returns "closed", and an error on the socket leads to
  `accept`'s own error, not to a loop. Its test (`tests/endpoint_socket.rs`) was not run.
- **The chord rules.** Both paste chords the injector can send are covered: Ctrl+V and Cmd+V
  are refused, Shift+Insert was never a valid chord. AltGr: letters and digits are covered on
  Windows (Ctrl+Alt with or without Shift, unless the Windows key is held) and in the recorder
  (`AltGraph`); the punctuation keys that AltGr also uses cannot be named in a chord at all;
  Ctrl+Alt+Space stays allowed, which is a no-break space on a few layouts. What a user may
  miss: on Windows every Ctrl+Alt+letter is refused, also on layouts without AltGr, where it
  is the customary pattern; the message says what to use instead. Alt with a letter or digit
  is refused on both systems, rightly on macOS; Option+Space stays allowed. Not refused, by
  choice: Cmd+Q, Cmd+W, Ctrl+S and the like. A chord stored before the rule and refused now is
  logged at the start and not registered, while Settings still shows it; there are no users
  yet, so a note is enough.
- **Windows without a console (`73c5872`).** By reading only. `iamb-service --version` and its
  usage errors print nothing in a console window now; the smoke check with redirected output
  still works.
- **Smaller.** `reopen_waiting` (`engine.rs:923-926`) stays set after a "fill in again" that
  ended with `request_cancelled` or `form_open`, so the next form with remembered answers
  renews the 30 minutes once; clear it on every outcome. `GetWindowsDirectoryW` gives a
  per-user directory to a program that is not marked as aware of Terminal Services;
  `GetSystemWindowsDirectoryW` has no such case.
- **Still untested.** The glue in `tray.rs` (N1 is there); a release build's refusal of
  `--ui-host` (the unit test asserts the opposite for the mock); the marker left by a second
  UI host (N2); a link or a directory in the pause file's place (N5); the tray's request with
  another program's window reported last (N4).

### 5.5 What was run

All passing: `cargo test -p iamb-service --test shortcuts --test supervisor_restart --test
supervisor_ui_host --test supervisor` (11, 6, 6, 6); `cargo test -p iamb-service --lib --bins`
filtered to the supervisor, paths and `main` tests (50 and 4); `cargo test -p iamb-ipc --lib --
linux` (1); `cargo test -p iamb-platform --lib -- hotkeys` (8); the desktop's `restart` test
binary (6) and its unit-test binary filtered to `menu`, `paths`, `onboarding`, `settings` (23),
both as built from this tree at 07:50, not rebuilt; the recorder's front-end tests (81). The
ten scratch tests of the first review again, five new ones (the tray's request, the pause
file), a release build of `iamb-service` for finding 1, and the hand runs named above. The
scratch tests are outside the tree; the release binary and the scratch test binaries were
removed from the shared target directory afterwards.

Not run: anything on macOS or Windows; the other desktop integration tests; the macOS
endpoint test.


## 6. Round 2 re-verification

Date: 2026-11-27 · Commits reviewed: `3817188` (first part) and `166c52b` (the rest; the working
tree named in the request had been committed as `166c52b` when this was read, and the checkout
was clean) · Same reviewer, same limits: everything was run on Linux, nothing on macOS or
Windows; each line says whether it was shown by a run or by reading.

### 6.1 Verdict

Nothing found here stands in the way of the beta any more. All six items left open by section 5
are closed, on Linux by a run wherever a run was possible, and by reading for the parts that
only exist on macOS and Windows. The fixes brought no new medium or high problem. What is left
is low (6.4), plus the checks on real machines that sections 4 and 5 already list (the Mac
checks H1 to H5, the tray on Windows and macOS): the macOS claims of this round rest on reading
the code and the window toolkit's source, not on a Mac.

### 6.2 The open items of section 5

| Item | Now | Evidence |
| --- | --- | --- |
| Finding 1, scenario a (`--ui-host` with `--platform mock` in a release build; the default root from `HOME`) | **Closed** | Run: a release build made from this tree refuses `--supervise --platform mock --ui-host <program>` and the same with `--platform native` (exit 1, "--ui-host is for development builds"), creates nothing in the data root and starts nothing; `tests/release_switches.rs` passes in a release build (1 test on Linux; the second is for systems with a native platform). Reading: `main.rs:158-168`, `209-220`: `--ui-host` needs a debug build on any platform, `--platform mock` needs a debug build where a native platform exists; `paths.rs:293-298` builds the default from the home directory `getpwuid_r` gives (`iamb-platform-macos/src/account.rs`), and with no answer from the user database the released helper serves no root. Unit tests pass (5 in `main.rs`, 18 for paths and supervisor). H4 in `docs/compatibility-run.md` now tries the mock platform and `open --env HOME=…` |
| N1, the tray lock held across menu calls | **Closed** (by reading, and its unit test) | `menu.rs:363-438`: `SharedCapture` changes the capture under its lock, lets the lock go, then calls the item. `tray.rs:257-271`: every change to the menu and the icon runs on the main thread (`on_main`: at once when already there, queued otherwise, never waited for). `render` no longer holds `shown_icon` across `set_icon` (`tray.rs:300-322`). The main thread still takes the capture lock in the pointer events, but nothing holds that lock while it waits for the main thread. The bridge lets its own lock go before it calls the sink (`bridge/mod.rs:138-161`), so the main thread's `bridge.status()` cannot be caught the same way. The test `the_capture_lock_is_never_held_while_the_menu_waits_for_the_main_thread` uses an item that takes the lock on each call and passes (31 desktop unit tests for `menu`, `app`, `paths`, `onboarding` pass in the binary built from this tree) |
| N2, a quit marker from another UI host hides a crash | **Closed** | Run: `second_instance` passes (the real UI host steps aside with status 0 and leaves no marker); `a_marker_created_while_the_ui_host_runs_does_not_turn_a_kill_into_a_quit` and `a_usage_error_ends_the_supervision_and_a_marker_does_not_hide_a_crash` pass. Reading: `supervisor.rs:222-250`, the marker counts behind a launcher only; `apps/desktop/src-tauri/src/main.rs:168-172` |
| N4, the tray's "Fill in again" with another program's window reported last | **Closed** | Scratch test re-run with the token: taskbar reported last, hand-back to the editor late, request naming the editor: no form before the editor's report, then the form for the editor. `the_trays_fill_in_again_with_its_window_waits_for_that_window_alone` passes (a password field or another window reported in between decides nothing). The tray and the palette send their window (`form.rs`, `palette.rs`). A request without the field still behaves as in section 5 (shown again: the form goes to the taskbar's window); that is what an older UI host gets |
| N5, the pause file | **Closed**, one case left (6.4, R1) | Scratch tests re-run: an empty directory in its place is removed and means not paused; a link to a file of the user's is removed, the file untouched, not paused; a dangling link planted before a pause is replaced by a regular file and the pause is there after a restart with `paused_kept`; a resume and a new pause clear `paused_kept`. `only_a_regular_file_in_its_place_keeps_the_pause` passes. Architecture §8 lists `run/paused` with the two signal files |
| Finding 8, no sign of a pause when the icon is hidden | **Closed** as far as the specification allows | Reading: the status carries `paused_kept`; the menu says "Iamb is still paused", the status line of the library window and the top of "Why didn't it expand?" say that Iamb is paused, and the user guide has "Nothing expands at all" (`docs/user/troubleshooting.md`). There is still no notice at the moment of an accidental press of the pause shortcut, which §4.11 rules out; the places a user looks when nothing expands now answer it |

Also re-checked from 5.2: finding 6 on macOS. The marker is now left when the event loop ends
without a requested status or with status 0 (`app/mod.rs:305-318`). In the window toolkit's
source a quit by the system (a logout, a restart, a "quit" Apple event) goes through
`applicationWillTerminate` to the loop's end without an exit request, so it leaves the marker
and Iamb is not opened again during a logout (reading; N6 of section 5 closed on that basis,
check H3 on a Mac). An end by SIGTERM behind `open` is still restarted; the module docs and
§3.2 say so.

### 6.3 The points asked about

- **The `focus` field of `ReopenLastForm`.** It gives a client no new reach. Only the UI role
  may send the request (run: the command-line and sync roles get `not_permitted`).
  Naming a window does not move the focus and does not type: the request waits until that
  window's focus change is delivered, and then that window's own verdict decides. Run: a named
  password window that never gets the focus ends with `request_focus_timeout`, nothing shown,
  nothing typed; named and then focused, a password window gives `refused_secure_field`, a
  window of an excluded app `refused_excluded_app`, one of Iamb's own `refused_own_window`;
  the focused window named while its field is a password field gives no form; a window that
  does not exist gives nothing. `ExpandSnippet` has had the same field, for the UI and the
  command-line roles, since Phase 2. The field is optional and absent on the wire when empty.
- **The marker on `RunEvent::Exit`.** A crash does not pass there: a panic leaves through the
  panic hook with status 70, a fault or a kill ends the process without the event loop's end,
  and an exit requested with another status leaves no marker. What does pass, besides a quit:
  an end of the event loop that some other process of the user asked for politely (a quit
  Apple event, a posted quit message), which is "on purpose". A marker written by a process of
  the user can still suppress a restart behind `open`; that is a same-user action and is
  stated in §8.
- **`on_main` ordering and staleness.** Each queued call renders from what the tray holds when
  it runs (the last status, the capture as it is), so order between two calls does not matter
  and the last state wins. A click reads the item's text through the main thread and compares
  it with the label of the target under the lock: a text that lags behind the capture refuses
  the click (`exclusion_label_changed`). One case was not checked on a real system: a capture
  that finishes after the menu was drawn (up to the two seconds it may take) changes the item
  while the menu is open; the click then acts on the application just captured, which is the
  one in front, while a menu that does not repaint could still show the older name.
- **Removing what is not a file at `run/paused`.** Only the entry itself is removed: a link is
  unlinked and never followed (run: the file it pointed to was untouched), a directory only
  when empty (run: a full one stays). The path is `<data root>/run/paused`; whoever chooses
  the data root is the user, and the worker has no rights the user lacks, so nothing can be
  removed that the user's process could not remove itself. A `run` that is itself a link is
  followed, as it is for every other file there.

### 6.4 What is left, all low

- **R1. A directory that is not empty in the pause file's place loses a pause at the next
  start.** `crates/iamb-service/src/worker/engine.rs:227-266`, `436-445`. Run: with such a
  directory there, pausing works (`paused: true`) but cannot be kept, the log has "cannot keep
  the pause for a restart", and the next worker starts not paused. Only a process of the user
  can make the directory. Fix: carry "the pause could not be kept" in the status so the menu
  can say it, or move the directory aside (rename) and create the file.
- **R2. A released helper whose `HOME` is not the user database's home, or that gets no
  answer from the user database, does not start, and says so only on standard error.**
  `main.rs:166-173`, `paths.rs:293-298`. By reading. It fails closed, which is right, but at
  login nobody reads standard error. Fix: when neither `--data-dir` nor `IAMB_DATA_DIR` is
  given, have the released helper build its root from the account's home instead of comparing
  and refusing; write the refusal to the supervisor log of the default root when there is one.
- **R3. The bundle check is still not a protection** (N3 of section 5, unchanged, and the
  comment says so): it looks for `iamb-desktop` while `open` starts what `CFBundleExecutable`
  names, and it reads no binary property list. Add to the macOS smoke test that the installed
  helper's log says the UI host was opened, so that a bundler change cannot silently stop the
  start at login.
- **R4. Smaller, unchanged from 5.4:** `reopen_waiting` stays set after `request_cancelled`
  or `form_open` (`engine.rs:1011`, `1073`, `1083`); `GetWindowsDirectoryW` instead of
  `GetSystemWindowsDirectoryW` (`iamb-platform-windows/src/files.rs:38`).
- **Not a finding, a limit to keep in mind:** the data-root rule does not stop a process of
  the user that writes into the default data root itself; §8 now says so.

### 6.5 What was run

All passing: `cargo test --locked -p iamb-service --test shortcuts --test supervisor_restart
--test supervisor_ui_host --test supervisor` from the checkout (13, 6, 7, 6);
`release_switches` in a release build (1); the service's unit-test binaries as built from this
tree, filtered to the supervisor, paths and `main` tests (18 and 5); the desktop's unit-test
binary filtered to `menu`, `app`, `paths`, `onboarding` (31), `second_instance` (1) and
`restart` (6), as built from this tree, not rebuilt. Four scratch tests of this round (the named
window, the roles, the pause file's cases, the tray's request with and without the token) and
a hand run of the release binary. The ten scratch tests of section 2's finding 2 were not run
again: their cases are in `tests/shortcuts.rs` now, which was.

How it was run, for whoever reads the build logs: the checkout was not edited. The scratch tests
were built from an exported copy of `166c52b` in the scratchpad with the shared target
directory; a copy shares the artifact names of the checkout, so the service crates were
rebuilt in place once, and the first attempt (01:20, a compile error in the scratch test)
overlapped the start of another test run in the checkout. While the disk was down to under
100 MB the scratch binary (16 MB, stripped) was moved to memory-backed storage and run there,
and the release build was made and run entirely in memory-backed storage; both are removed and
nothing of this round is left in the shared target directory.

Not run: anything on macOS or Windows (the mock-platform refusal test for systems with a
native platform, the home lookup, the tray, the quit by the system); the front-end tests; the
other desktop integration tests.
