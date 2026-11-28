# Security review of the cached probe, the custom-element rule and the focus diagnostic

Date: 2026-11-27 · Commit reviewed: `b632eb4` ("Ask a slow application once, believe a focused
web element, and diagnose focus") · Reviewer: independent, did not write the code and judged it
from the code, not from the commit message

Scope: `crates/iamb-platform-windows/src/focus.rs` and
`focus/{uia,classify,secure,os,inspect,diagnose,os_tests}.rs`, `crates/iamb-platform/src/focus.rs`
(`FIRST_LOOK_TIMEOUT`), `crates/iamb-service/src/worker/threads.rs` (the focus thread),
`crates/iamb-service/src/main.rs` (`--diagnose-focus`), and the one changed line of
`docs/architecture.md`.

The review changed no file of the tree and added this one. What was run:

- On Linux, from an exported copy of the commit with a build folder outside the tree: the 59
  unit tests of `focus::` pass, and four scratch tests against the same fakes were added outside
  the tree. A finding that says "shown by a run" means one of these.
- The method-table slots were counted by a script from the interface definitions of the
  `windows` crate 0.62.2 in the local registry, independently of the numbers in the code.
- The Windows leg of CI for this commit (run 46066040415, job `test (windows-latest)`, desktop
  required) was read: `focus::os_tests::ui_automation_tells_a_password_field_from_a_text_field`
  and the four other Windows-only tests of the module ran and passed there.

Nothing else was run on Windows. The watcher, the real probe, `EnumChildWindows` and the
diagnostic's run were read. No browser engine was available: everything said below about what an
engine reports is from reading or is marked as not checked.

## 1. Verdict

The mechanical parts are right. The hand-declared slots are the right ones, a cache miss on one
of the three facts that decide gives `Unknown`, the page root and the window are still never
believed, a password element is `Secure` before anything else is looked at, the 300 ms first
look adds no exposure, and the diagnostic reads and writes no title, name, value or typed text
and cannot disturb the running service.

One thing should be settled before this ships:

- **Medium, by reading:** the new rule believes a custom element of a browser-engine page, and
  the two facts it adds do not separate a built page from an unbuilt one. The diagnostic's own
  sample line shows the page root with the same two facts. So the rule stands on one premise,
  "an unbuilt page never reports a custom element", which was measured for one application on
  one engine and now covers both engine families in every version. Where the premise fails, a
  focused password field is `Normal`, settled, and nothing looks again (finding 1). The
  framework name is already fetched and would keep the rule as narrow as the measurement.

The rest is low: a page-drawn password box with an unusual role moves from `Unknown` to
`Normal` (finding 2); a late answer's age leaves out the call that produced it, so "a quarter
of a second" is no longer the bound (finding 3); the process rule for late answers is met by any
process that owns any window inside the target (finding 4); the diagnostic's files hold a timed
trace of applications and are never pruned (finding 5) and print program text unescaped
(finding 6); and the Windows test does not pin the two new getters the rule depends on, nor
`has_window_in` (finding 7).

I could not make a password field or another application's field `Normal` through the slots,
the cache or the child-window rule alone. Finding 1 is not a demonstrated break: it is an
unverified premise on the path that decides, where the code before this commit failed closed.

## 2. Findings

### Medium

#### 1. The custom-element rule rests on one unverified premise; its two extra facts add nothing against an unbuilt page

`classify.rs:225-227`, `classify.rs:242-249`; module docs `classify.rs:24-36`. By reading; the
decision table shown by a run.

Before this commit a custom element under a browser-engine control was one of the four kinds
that are not believed (`is_shell`, there since the secure-field check was written). Now:

```rust
control_type == control_type::CUSTOM && detail.has_focus && !detail.window_root
```

is believed, in time or late, and the verdict is settled (`Normal`, no look again). The scratch
run:

| Focused element under a browser-engine control, `IsPassword` false | Verdict |
| --- | --- |
| custom, focus yes, no window handle, framework the engine's | `Normal`, settled |
| custom, focus yes, no window handle, framework **another** or the system's own | `Normal`, settled |
| custom, focus no | `Unknown` |
| custom, a window's own | `Unknown` |
| document, pane or window, focus yes, no window handle | `Unknown` |

Three things make the premise carry more than it can:

1. **The two facts are not evidence of a built tree.** The docs call them "facts the engine
   gives for a node of its tree". The page root of an unbuilt page gives them too: the
   diagnostic's own test line is `document (50030) password=no focus=yes window's own=no`
   (`diagnose.rs:338-339`). They tell a node from a *window*, not a built page from an unbuilt
   one. What is left is the control type alone.
2. **A missing property is the believing value for one of them.** A provider that does not
   supply `NativeWindowHandle` yields 0 from the cache, which reads as "not a window's own"
   (`uia.rs:286-298`). The code is careful when a getter *fails* (`uia.rs:344`,
   `unwrap_or(true)`), but an absent property is not a failure.
3. **The measurement is narrower than the rule.** The evidence is one chat application on the
   system's web view. The rule applies to every control whose class starts with `Chrome_` or
   `Mozilla` (`classify.rs:205-209`): two engine families, every version an application ships,
   and both ways an engine can answer (its own UI Automation tree, or the older accessibility
   interface translated by the system, where a role the system does not know becomes a custom
   element). The framework name, which tells these apart, is fetched and then used only by the
   diagnostic: the second row of the table.

Scenario, if the premise fails anywhere: an application built on an engine restores the focus
into a login form when its window comes to the front. Its page is still being built, or the
field sits in a frame whose part of the tree has not arrived, and the engine reports the focus
on a placeholder with no standard kind: custom, focused, no window handle, not a password
field. The verdict is `Normal` and settled. The user types a password; an abbreviation in it
expands, and nothing asks again until the next counted focus event. Before this commit the same
answer was `Unknown` and the window was looked at again after 150 ms, 400 ms, 1 s, 2 s and 4 s.

I do not know that any engine does this, and could not test one. That is the point: nobody has
shown that none does, and this is the path where the old code failed closed.

Fix, in order of cost:

- Believe a custom element only when its framework is the engine's own (`Framework::Chrome`),
  the case that was measured. The `Mozilla*` family and the translated interface then stay
  `Unknown` as before, until there is data for them. Check first that the real-machine line
  says `framework=Chrome`; if it does, this costs nothing.
- Or believe it only at the second look: the first custom answer for a subject passes
  (`NotInspectable`, look again after 150 ms) and the same answer again settles. A transient
  placeholder is exactly what the looks again were built for.
- Add a manual case per engine family to `docs/compatibility-run.md`, and use the new
  diagnostic for it: cold start with the focus restored into a password field, a password
  field inside a frame from another origin, and a reload with the field focused. Fail when a
  `custom … password=no` line with verdict `Normal` appears for a control where a later line
  says `password=yes` with no `focus` line between them.
- Correct `classify.rs:24-36`: the two facts tell a node from a window; they say nothing about
  whether the tree is built.

### Low

#### 2. A password box a page draws itself, with an unusual role, is now `Normal`

`classify.rs:225-249`; `docs/architecture.md:980` (detection limits). By reading.

The docs say "a password field is never one: an `<input type="password">` reports
`IsPassword`". True for that element. A page that builds its own masked box (an editable
region with a role the engine maps to no standard kind, masked by style or script) reports
custom, focused, not a password. That was `Unknown` in an engine window and is `Normal` now.

Scenario: a banking or single-sign-on page with a scripted PIN box. Before: nothing expands
there. After: an abbreviation typed inside the PIN expands.

This is the documented limit "an application that draws its own fields and describes none",
which until now did not reach into engine pages for this element kind. Fix: say so in the
limits table (row at `docs/architecture.md:980`), so that the remedy (the exclusion list) is
named for it; the fences of finding 1 do not remove it.

#### 3. A late answer's age leaves out the call that produced it

`uia.rs:422-441` (`chosen` is taken after the one call returns), `inspect.rs:399-405` (the
bound), docs `uia.rs:35`, `inspect.rs:40-45`, `docs/architecture.md:981`. Shown by a run on the
fake, which stamps the answer the way the real probe now does; the real probe by reading.

The commit message says "a late answer is as old as it looks", and the docs say the element "is
chosen when the answer arrives". The element is chosen somewhere inside the call; the stamp is
taken after it. Scratch run: a call that took 730 ms and answered "plain field" 700 ms after the
question gave up was taken as "0 ms old", verdict `Normal`. Before this commit the stamp sat
after the first of four calls, so the age covered the three property calls; now it covers none
of the round trip. The window in which a field can change unseen is the duration of the call
plus 250 ms, and the call has no limit of its own.

Scenario: a page that is busy starting answers in 1.5 s. Inside that time its script moves the
focus from the name field to the password field; an engine that is still switching its
accessibility on may raise no focus event for it, and then the watcher's count does not move. If the
application's side read the focus early in the call, the answer says "plain field", arrives
"fresh", and settles `Normal` on a password field.

In most slow calls the time goes before the application gets round to the request, and the
answer is then fresh when it arrives. So this is low, the same class as the remainder left open
in the late-answer review, but it is wider than the docs now say. Fix: take a late answer only
when its call took at most a stated limit (the diagnostic already records `took`; one or two
seconds would still cover the measured application), and correct the three doc passages and the
limits row: "at most 250 ms after the answer arrived; the answer is as old as its call took".

#### 4. The process rule for late answers is met by any process with any window inside the target

`secure.rs:152-163`, `os.rs:322-351`. Shown by a run on the fake (a late element from process
77: `Unknown` with no window inside, `Normal` when 77 owns one); the real enumeration by reading.

The late-answer review's finding L4 added a backstop: a late answer about an element provided
by a process other than the window's is not believed. It is there for the case the other
fences miss, a focus change that was not counted. The new third alternative accepts a process
that owns *any* descendant window of the window in front, at any depth, visible or not.

Tried: a same-user process creates a child window inside the target window, so its elements
pass the rule. Alone this does nothing. For the focused element to be that process's, its
window must have the keyboard focus, and then the keys go to it; giving the focus back is a
focus event, the count moves, and the late answer is dropped by its subject (window, focused
control, kind, count) before the process is ever looked at. A local process of the same user
also gains nothing from a wrong `Normal` that it cannot have directly. So this is not an
exploit; it is the backstop getting thinner. After this change it stops only processes with no
window at all inside the target.

Without an attacker: an application that hosts a second process's window (a preview pane, a
plug-in, a video surface) beside its web view. A focus change between the two that the watcher
misses used to be caught by the process; now it is not.

Fix: accept what was measured and no more. A process is known when it owns the focused control,
the window in front, or a window on the focused control's own line (its ancestors up to the
window in front, or its descendants: `EnumChildWindows` of the control, not of the top-level
window). If the real case needs more, restrict the extra windows to those of an engine class.

One thing that held here: `EnumChildWindows` with a null parent walks every top-level window,
which would accept every process with a window. The window in front cannot be 0 at this point
(`os.rs:83-94`: a window without an owner is no front), and a stale handle enumerates nothing.
A line asserting it would keep that from depending on code elsewhere.

#### 5. The diagnostic's files are a timed trace of applications, kept for ever and uncapped

`main.rs:155-180`, `diagnose.rs:136-148`, `diagnose.rs:234-237`. By reading.

What a file holds was checked line by line: the executable's file name, the two window class
names, the control kind, process ids, the control type, yes or no for three facts, one of
seven fixed framework words, durations and the verdict. No title, no element name, no value,
no typed text: the method tables have no slot for them and the cache request names none
(`uia.rs:73-81`). That claim of the commit holds.

What it does hold is exactly what the service's own log refuses to write
(`worker/threads.rs:82-85`: "no application, no window … a line per look would be a timed
trace of the user's moves between fields", the late-answer review's finding L5): for up to ten
minutes, which application was in front at which millisecond, including applications on the
exclusion list. For a diagnostic the user starts, that is the purpose. The handling of the
result is the finding:

- `logs/focus-diagnosis-<unix ms>.txt` is never rotated, pruned or capped. The logs are
  5 × 1 MB per process and crash reports keep the last 10; these files stay until the user
  deletes them, in the folder the settings page opens and that users send for support. A run is
  bounded by its 600 s, not by size: a window that raises focus events quickly writes three
  lines per event.
- The first line says "No text is read." It does not say that the file names applications and
  times, which is what a user should know before sending it.
- In a release build the service has no console (`main.rs:59`), so "writing to …" and "done"
  are not seen unless the output is redirected; the user is not told where the file went.

Scenario: a user runs the diagnostic for a problem with one application, as asked. Months
later they send their logs folder for an unrelated problem; it still holds which applications
(a password manager, a bank's client, a medical portal's shell) they used that day, to the
millisecond.

Fix: keep the last few diagnosis files (the crash reports' rule), stop writing at a size
limit and say so in the last line, state in the first line what the file contains, and say in
the documentation where it is written. Consider leaving out or masking applications on the
exclusion list.

Beside the service: any process of the user can start the diagnostic. It gets nothing from it
that it could not get by watching the focus itself, and no keys. See "What held".

#### 6. Window class names are written unescaped: a program can write lines of its own into the file

`diagnose.rs:136-148`, `diagnose.rs:261-262`, `os.rs:122-131`. Shown by a run on the formatter;
that the system accepts a line break in a class name was not checked on Windows.

A class name is up to 256 units of text chosen by the program that registered it. The scratch
run put a line break into one: one focus change came out as four lines, among them a forged
`uia … edit (50004) password=yes …` and a forged `verdict Secure after 2 ms`. The file exists to
be sent to the maintainers as "what the check sees"; the application under suspicion can decide
what it says. The service's log has the rule already (short and plain, the 2.8 review's finding
10); the diagnostic does not apply it.

Fix: pass the application name and both class names through the same filter as log text
(printable characters only, a fixed maximum, a mark when cut) before formatting.

#### 7. The Windows test does not pin the two getters the new rule depends on, nor the child-window rule

`os_tests.rs:23-29`, `os_tests.rs:59-103`; `uia.rs:341-344`; `os.rs:322-351`. By reading.

The slots are right (section 3). What would notice if a later edit moved one:

- `HasKeyboardFocus` (slot 58) is only ever asserted `true`. Its neighbours 59, 60, 65 and 66
  (`IsKeyboardFocusable`, `IsEnabled`, `IsControlElement`, `IsContentElement`) are also true
  for every control of the test.
- `NativeWindowHandle` (slot 68) is only ever asserted "a window's own", and a getter that
  fails also reads as that (`unwrap_or(true)`). The test passes with any slot whose method
  fails for these arguments or writes something other than 0. The case that matters, an
  element with no window, is never seen.
- `SystemDesk::has_window_in` has no test on Windows at all; only the fake is tested.
- No test feeds a real engine's custom element through the probe.

The compile-time `offset_of!` checks (`uia.rs:163-174`) compare the code with its own numbers,
so they catch a miscount of padding, not a wrong number.

Fix: in the Windows test, compare the cached window handle with the test control's real handle,
and focus one element that is not a window (an item of a list box) to see `false`; add a
Windows test of `has_window_in` with a window's own child and with a process that owns none.

## 3. What held

- **The slots.** Counted from the `windows` crate 0.62.2: `IUIAutomation`
  `GetFocusedElementBuildCache` 12 and `CreateCacheRequest` 20; `IUIAutomationCacheRequest`
  `AddProperty` 3; `IUIAutomationElement` `get_CachedProcessId` 52, `get_CachedControlType` 53,
  `get_CachedHasKeyboardFocus` 58, `get_CachedIsPassword` 67, `get_CachedNativeWindowHandle` 68,
  `get_CachedFrameworkId` 72. The same count reproduces the earlier, Windows-confirmed slots
  (8, 20, 21, 35). Argument types match (32-bit number, `BOOL`, a handle, a string). The
  property ids are the bindings' constants, not numbers typed here. On the real library, in
  CI for this commit, system controls came back through the cached getters as password yes and
  no, edit and button, the test's own process id, and framework `Win32`: a wrong slot for any
  of those four would have failed there.
- **Wrong slots reading the wrong property.** Not found. The nearest thing is finding 7: two
  of the six are right but not pinned by a test.
- **Cache misses.** A getter that fails for `IsPassword`, the control type or the process id
  makes the whole answer `Failed`, which is `Unknown` (`uia.rs:329-349`). A failing detail
  getter gives the value that believes nothing more (no focus; a window's own). A cache request
  that cannot be built means no session, which is `Unknown`. An *absent* property gives the
  system's default: not a password (inherent, as before this commit), no focus (not believed),
  no window handle (the believing value: finding 1, point 2).
- **A password element is `Secure` whatever its kind and detail**, and the document, the pane
  and the window are never believed whatever they say (shown by a run).
- **A custom element outside an engine window** is `Normal`, as before: unchanged.
- **One call.** The element and its properties come from one request, so the properties
  cannot be those of a different element than the one chosen, which four separate calls could
  not promise.
- **300 ms first look.** `FocusChanged` goes to the engine before the question
  (`worker/threads.rs:65-74`), so the engine is suspended for the whole wait whatever its
  length; a further change waits behind it, still suspended. The longer wait changes which
  answers count as in time, nothing else. Moves inside the same window keep 100 ms.
- **An unbuilt page reporting the document or the window** is still `Unknown` with looks again.
- **The diagnostic beside the service.** It starts a platform of its own with nothing started
  (`lib.rs:105-110`), installs no keyboard hook, opens no endpoint, takes no lock and touches
  no file under `run/`. Its watcher has its own counts and its own wake message, so it cannot
  move the service's count or mark. It runs 1 to 600 s and refuses other values; on other
  systems it refuses. The data root is made private before it runs (`main.rs:249`) and the
  logs folder inherits that (`files.rs:80`), so the file is the user's alone although the
  folder is made with the plain call. The file is created new and never opened over an
  existing name. What running it, or several of it, does cost is more questions into the
  application in front, so more late answers and timeouts for the service: `Unknown`, the safe
  direction, and nothing a process of the user could not cause without it.
- **The service's log** is unchanged: the reason only, no application, no window.

## 4. What could not be checked

- Anything about real engines: whether an unbuilt or half-built page, a frame from another
  origin, or the translated older interface ever reports a custom element with the focus
  (finding 1), and what a page-drawn password box reports (finding 2). No engine was available.
- The real-machine data behind the rule was not in the tree; I saw only the sample lines in
  the tests.
- Whether the cached call returns the same element as the call it replaces. A cache request
  has a tree filter of its own; if the system normalises the focused element to that view, an
  element outside it would come back as its parent. The CI test shows system controls are
  unaffected; an engine's elements were not seen.
- `EnumChildWindows` against real windows of another process, and `has_window_in` on Windows
  at all (finding 7).
- Whether the system accepts a line break in a registered class name (finding 6); the
  formatter was run, the system was not asked.
- Where, inside a slow call, an engine reads the focus (finding 3): the run shows what the
  bound counts, not what a real engine does within the call.
- The diagnostic's run on Windows: its lines were tested as formatting; the loop was read.

## 5. Fixes

Made after the review, on top of `7eb13e2`, not committed. Run on Linux; the Windows code was
compiled and linted for Windows (`clippy --target x86_64-pc-windows-msvc`, with and without
`probe`), not run. "Fails without the fix" was checked by taking each fix out again in the tree
and running the test.

| Finding | Fix | Test (fails without the fix) |
| --- | --- | --- |
| 1 (medium) | A custom element under a browser-engine control is believed only when **all** hold (`classify::custom_candidate`): its framework is the engine's own (`FrameworkId` "Chrome", which the system's web view reports too; `Mozilla*` controls and translated older-interface elements stay `Unknown`), it has the keyboard focus, it names no window, and it is **not the first look**: the first such answer for a subject (window, focused control, kind, count) is `NotInspectable` and looked at again; only the same answer at a report the watcher made again, with no change counted between, settles (`Inspector::custom_again`, `Watching::again`). The window handle can now only refuse: a non-zero handle, or one that cannot be read, refuses; its absence is not relied on as evidence. The module docs of `classify.rs` say that none of the facts proves a built tree: two tell a node from a window, the framework keeps the rule to what was measured, the second look gives a placeholder time to change. `focus.rs`'s limit on editable regions says the same. The manual cases per engine family (cold start into a password field, a cross-origin frame, a reload) are not added to `docs/compatibility-run.md` beyond the diagnostic's section: they need an engine to write the pass/fail lines against | `classify`: `a_custom_element_of_the_engine_is_believed_only_at_a_second_look` (another framework, the system's own, no focus, a window's own: never; the first look: not believed). `secure`: `a_focused_custom_element_of_the_engine_is_believed_at_the_second_look_only` (first look `NotInspectable`, look again `Normal`, a counted change starts again, a re-judge does not settle, no watcher never) |
| 2 | A row in architecture §8's detection limits: a password box a page draws itself, with an unusual role and no `IsPassword`, is an ordinary field at the second look inside an engine window; the remedy is the exclusion list | Docs only |
| 3 | A late answer is taken only when its call took at most `LATE_CALL_LIMIT` (250 ms) as well as arriving at most 250 ms before; the mailbox keeps the call's duration, and a refused one is traced (`LateTooSlow`) and shown by the diagnostic. The docs of `focus.rs`, `inspect.rs` and the §8 row say what the age is: as long as its call took plus the wait after it, at most half a second | `inspect`: `a_late_answer_whose_call_took_too_long_is_not_taken` |
| 4 | `Desk::has_window_in` is replaced by `on_control_line(front, control, pid)`: a process is accepted when it owns the focused control's window, one of its ancestors up to the window in front, or one of its descendants (`EnumChildWindows` of the control, not of the top-level window). A control outside the window in front has no line; a null control or a null window in front gives `false`, so the enumeration can never walk the desktop | Fake: `a_late_answer_from_another_process_with_a_window_inside_is_believed` (renamed in spirit, the fake's set stands for the line). Windows: `os_tests::the_line_of_the_focused_control_is_its_own_window_tree` |
| 5 | `iamb_service::diagnosis`: before a run, older `focus-diagnosis-*.txt` files are removed so that five remain with the new one; a file stops at 2 MB and its last line says so; its first line says that it names the applications that were in front, their window classes and the times, and holds no text. The path is printed at the start on standard output and standard error. `docs/compatibility-run.md` has a section saying where the file goes and how to run it. Masking excluded applications was not done: the diagnostic exists to show what the check sees, and an excluded application is decided before the check | `diagnosis`: `the_oldest_files_go_and_others_stay`, `a_file_says_what_it_holds_and_stops_at_its_limit` (new module) |
| 6 | The application name and both class names pass through the log's rule for client text (`diagnose::shown`): at most 64 characters, each outside `A-Z a-z 0-9 . + - _` replaced by `?`, `...` when cut | `diagnose`: `a_name_a_program_chose_cannot_write_lines_of_its_own` (a class with a line break and a forged verdict stays one line) |
| 7 | Windows tests: `the_cached_window_handle_is_the_controls_own_and_none_for_an_item` compares the cached `NativeWindowHandle` with the test control's real handle, then focuses the item of a new list box in the test window (an element that is not a window) and expects list item, focus yes, a window's own no, handle 0; `the_line_of_the_focused_control_is_its_own_window_tree` tests `on_control_line` with the test's own process, a process that owns none, no control and no window in front. A real engine's custom element through the probe is still not tested: no engine on the build machines | Windows only (CI's Windows leg); compiled here |

Checks: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
the lint job's cross-target clippy steps (windows-msvc with and without `probe`,
aarch64-apple-darwin with and without `probe`, the testkit's); `cargo xtask banned-terms`
(0 hits); `cargo test --workspace --exclude iamb-desktop` (114 test binaries); `cargo test -p
iamb-desktop`; `pnpm typecheck` and `pnpm test` (550): all pass. Not checked: anything on
Windows, in particular the two new Windows tests, which run in CI's Windows leg for the first
time.
