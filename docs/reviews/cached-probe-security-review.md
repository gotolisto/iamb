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

### 5.1 After the re-verification (section 6)

Made on top of `c4e0025`, not committed; run on Linux, the Windows code compiled and linted for
Windows only.

| Item | Fix | Test |
| --- | --- | --- |
| N1 (medium, function) | Both halves. **The loop:** the report a late answer's arrival makes now spends one of the window's looks again (`rules::Settle::spend`, called by the watcher before it reports for an arrival), like a pause does; the wake is still armed again after a refusal, but every report it causes comes out of the same budget of five, so after one focus change the window is asked at most six times and then stays `Unknown` until the next change. Counting was chosen over not re-arming because it bounds every path (pauses, arrivals, refusals of any kind) with one number, where not re-arming would leave the pause timer free to restart the cycle. **The limit:** `LATE_CALL_LIMIT` is 1 s, checked at compile time to be no lower than `FIRST_LOOK_TIMEOUT` (300 ms): a late answer only exists when its call outlasted its question, so a limit below the longest wait in time refused every late answer after a first look, while an answer in time is already believed with a call of up to 300 ms. One second keeps a late answer at most 1.25 s old when it is used (call plus wait), which `focus.rs`, `inspect.rs`, `secure.rs` and the §8 row now say; no measured call duration was available to set it from (the diagnostic now prints them). The overrides of `2c60590` (`with_late_call_limit(LONG)`) are removed: the tests run with the real limit, which is far from their calls | `rules`: `a_late_answer_spends_the_same_budget_as_a_pause`. `secure`, with the real limits and a fake whose every call sleeps (`Step::Slow`), the watcher played as `watch.rs` and `focus.rs` play it: `a_slow_application_is_judged_once_by_its_late_answer_with_the_real_limits` (calls of 600 ms: two questions, one call, `Normal`, and `Secure` for a password field; fails with the 250 ms limit), `a_too_slow_application_settles_unknown_on_the_budget` (calls of 1.6 s: `Unknown(Timeout)` after at most six questions, no more calls than questions), `a_fast_application_is_judged_at_the_first_look` (20 ms). The durations are at least 300 ms from every limit; the pauses are not waited for (a pause only lets the call finish, and the test waits for the arrival itself) |
| N2 | A late answer refused by the process rule, a timeout and no probe now call `custom_again(subject, false)`: the next custom answer is a first look again | `secure`: `an_unjudged_answer_between_two_looks_voids_the_first_custom_one` (fails without the reset) |
| N3 | `on_control_line` does not walk down when the focused control is the window in front itself: then only the window's own process and its ancestors count | Windows: `os_tests::the_line_reaches_down_into_a_control_with_children` (last two lines) |
| L3 doc | `uia.rs`: "the application reads the element and its properties together, at one moment somewhere inside that call (the answer is as old as the call took, which `inspect` bounds)" | Docs only |
| L7 | `HasKeyboardFocus` seen false: a test-only `Session::observe_window` (`ElementFromHandleBuildCache`, `IUIAutomation` slot 10, counted from the same generated tables and checked at compile time) reads the button while the text field has the focus. The downward walk runs its callback on a new combo box in the test window, whose edit box is its own child window | Windows: `os_tests::an_element_without_the_focus_says_so`, `os_tests::the_line_reaches_down_into_a_control_with_children` |

Checks: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
cross-target clippy (windows-msvc with and without `probe`, aarch64-apple-darwin with and without
`probe`, the testkit's Windows step); `cargo xtask banned-terms` (0 hits); `cargo test -p
iamb-platform-windows` three times in a row (254 unit tests each time, about 7 s, the same
timings) and once more with every core kept busy by other processes (254 pass); `cargo test
--workspace --exclude iamb-desktop` (114 test binaries); `cargo test -p iamb-desktop`: all pass.
Not checked: anything on Windows; the four Windows tests added here and in section 5 run in CI's
Windows leg.

## 6. Re-verification

Date: 2026-11-28 · Commit re-verified: `cd4b07d` ("Close the strict review's findings on the
cached probe"), read as `git show cd4b07d`. While this was written the checkout moved on by two
commits that change tests only, and both were read: `c38b51d` (one expectation of a Windows
test) and `2c60590` (the tests of late answers set the call limit out of reach). Same reviewer,
same rule: judged from the code, not from the commit message or from section 5.

What was run, all on Linux from an exported copy with a build folder outside the tree: the 62
unit tests of `focus::` and the two of `iamb_service::diagnosis` pass; four scratch tests were
added outside the tree (three on the fakes of `secure`, one on `diagnosis`). CI was read for
the Windows side: on `cd4b07d` the Windows leg failed in one of the two new tests, on an
expectation about the framework name that `c38b51d` then corrected, and the macOS leg failed in
a test of late answers (6.3, "CI"); the run for `2c60590` had not finished when this was
written. Nothing was run on Windows by me, and there was still no browser engine.

### 6.1 Verdict

Every finding of section 2 was acted on, and the fences asked for in finding 1 are in. I could
not make a password field or another application's field `Normal` through the new second-look
rule: one call can never serve as both looks, a counted change starts the count again, and a
real password field at the second look is `Secure`.

One of the fixes brings a new problem that should be fixed before this ships. It fails closed,
so it is not a way to a wrong `Normal`:

- **Medium (function, not confidentiality), shown by a run:** the 250 ms limit on a late
  answer's call is below the 300 ms the first look waits, and a refused late answer asks
  afresh with the wake armed again. An application whose calls take longer than 250 ms is
  never judged by a late answer, and it is asked again the moment each answer arrives, for as
  long as its window is in front (6.3, N1). This undoes, for such applications, what `b632eb4`
  was written for.

Two low notes besides (N2, N3), and what is still open in findings 4 and 7.

### 6.2 The findings

| Finding | Status | Evidence |
| --- | --- | --- |
| 1 (medium) custom-element premise | **Closed in code; the manual engine cases are still open** | `classify.rs:243-260`: a custom element is a candidate only with the engine's own framework, the focus and no window handle; `secure.rs:158-162` believes a candidate only at a look again whose answer before, for the same subject, was a candidate too. Run: custom then custom at a look again: `Unknown`, `Normal`. A custom element of another framework or of the system's own, three times: `Unknown` every time. The docs no longer call the two facts evidence of a built tree. Not done, and said so in section 5: the manual cases per engine family. They are the only thing that can test the premise itself; the diagnostic is there to run them with |
| 2 page-drawn password box | **Closed** | The limits table has the row (`docs/architecture.md:981`), with the exclusion list as the remedy; `focus.rs` and `classify.rs` say the same |
| 3 age leaves out the call | **Closed as found; the fix has a new problem (N1)** | `inspect.rs:67`, `inspect.rs:431-433`: a late answer whose call took more than 250 ms is not taken. `focus.rs`, `inspect.rs` and the limits row now say what the age is (call plus wait, at most half a second). One sentence was missed: `uia.rs:35-36` still says "the element is chosen when the answer arrives" |
| 4 process rule | **Closed for the case described; unchanged when the focused control is the window in front** | `os.rs:330-378`: the control, its ancestors up to the window in front, its descendants. A sibling window's process (the preview pane beside the web view) is no longer accepted. A null control or null window gives `false` before any enumeration. See N3 for the case where the line is the whole window again |
| 5 diagnostic files | **Closed** | `diagnosis.rs:41-59` removes the oldest files so that five remain with the new one; `diagnosis.rs:113-125` stops at 2 MB and says so; the first line says the file names applications and times; the path goes to both output streams and `docs/compatibility-run.md:129` says how to run it. Run: the two module tests pass; 2,991 lines of 700 bytes wrote 2,096,939 bytes against a limit of 2,097,152. Masking excluded applications was declined with a reason I accept: the diagnostic exists to show what the check sees |
| 6 unescaped names | **Closed** | `diagnose.rs:39-55`: the application name and both class names are cut to 64 characters and everything outside letters, digits and `. + - _` becomes `?`. The module's test feeds the forged class of my scratch run and gets one line |
| 7 Windows tests | **Partly closed** | The cached window handle is now compared with the control's real handle and with 0 for a list item, which pins slot 68 both ways; on `cd4b07d`'s Windows run the first of these comparisons passed before the test failed further down on the framework name (expected `Win32`, got none of the known names; `c38b51d` changes the expectation). `HasKeyboardFocus` is still only ever seen as yes, which is all the focused element can show. `on_control_line` is tested with the test's own process, a process that owns no window, no control and no window in front; the downward enumeration never reaches its callback in that test (an edit control has no children), and "a sibling is not on the line" cannot be tested in one process. No engine element goes through the probe |

### 6.3 The points asked about, and what is new

**The second look for custom elements.**

- *Can one answer be both looks?* No. An answer in time is delivered to one question. A late
  answer is removed from its slot when it is looked at (`inspect.rs:423`) and is not put back
  once taken. Run: a late custom answer, then two more looks again: `Unknown` (the first
  candidate), then a second call was made and its answer, a password field, gave `Secure`.
- *Can the second look be a stale answer?* It can be a late answer of a second call, like any
  look again: at most 250 ms of call plus 250 ms of wait. Run: custom in time, a look again
  that timed out, its answer (call 200 ms) taken 200 ms after it came: `Normal`, where the
  next scripted answer was a password field. That is the documented window of every late
  answer, not a way round the rule; it is half a second and no longer unbounded.
- *Across a change the watcher missed?* The subject carries the count, and a missed change by
  definition does not move it, so the first look survives it. The second look is then a new
  question about whatever has the focus now. Run: custom, then a password field: `Secure`;
  custom, then the page root: `Unknown`, and the count starts again (the next custom is a first
  look). It comes out `Normal` only if the newly focused thing is itself a focused custom
  element of the engine twice, a pause apart: a page-drawn box (finding 2's limit) or a
  placeholder that lasts longer than the pause. The two looks are always at least one pause
  of the watcher apart (150 ms or more), because the second must be a report again.
- *Another count, another control, a look that is not a report again, no watcher:* none
  confirms (run, and the module's own test).

**N1. Medium (function): a call over 250 ms is never used and is asked for ever.**
`inspect.rs:67`, `inspect.rs:431-433`, `inspect.rs:470-475`, `watch.rs:139-150`,
`crates/iamb-platform/src/focus.rs:25,38`. Shown by a run on the fakes with the real limits,
with the watcher's part played as `watch.rs` and `focus.rs` write it; the watcher itself by
reading.

A late answer exists only when a call outlasted its question: 100 ms, or 300 ms for the first
look at a window. With the limit at 250 ms:

- after a first look, every late answer took more than 300 ms, so every one is refused;
- a refused answer asks afresh inside the same question, with 100 ms, which times out again,
  keeps the new call's answer for later and arms the wake again;
- the wake reports the window again at once and is not counted against the five looks again
  (`watch.rs:139-150` does not go through `Settle`), so the new answer is refused the same way.

Run, every call 400 ms, change asked with 300 ms: `Unknown(Timeout)`, then six wakes, each
"late TOO SLOW 400 ms", each asking again, 7 calls in 2.5 s, and nothing that ends it. The same
with calls of 280 ms after a move inside a window (100 ms): never judged, although the first
look at that window, with 300 ms, had believed the same application in time. Calls of 200 ms
work as before: one wake, late answer taken, `Normal`.

Scenario: a chat or mail application built on an engine answers in 350 ms while it is busy.
At `b632eb4` its field was judged by the late answer after one call. Now nothing expands there
at all, and for as long as its window is in front the service has a call into it running
back to back, each one followed by a focus report to the worker. The slow application is made
slower by the tool that waits for it.

It also buys nothing: an answer in time at the first look is believed with a call of up to
300 ms, so a late one of 260 ms is not the older answer.

Fix: (1) end the cycle whatever the limit: a wake's report counts against the looks again
(`Settle`), or a refused late answer asks afresh without arming another wake; (2) set
`LATE_CALL_LIMIT` no lower than the longest wait in time, and from the measured data rather
than by symmetry (the diagnostic prints `call N ms`; section 2 suggested one to two seconds),
then state the resulting bound in the docs; (3) a test on the fakes: with every call slower
than the limit, the number of questions after one change is bounded.

**N2. Low: an answer between the two looks that says the focus was elsewhere does not start
the count again.** `secure.rs:158-175`, `inspect.rs:355-360`. Shown by a run.

`custom_again` is called for answers that are judged. A late answer refused by the process rule,
a timeout and a refused late answer are not judged, so they leave the first look standing. Run:
custom, a look again that timed out, its late answer about an element of process 77 (refused,
`Unknown`), custom: `Normal`. The answer in the middle is the one piece of evidence the code
ever gets that the focus was in another process with no change counted, which is when the
first look should be void. The commit message says "the same answer comes again at the next
look"; the code says "a candidate again at some later look again".

Scenario: an engine window shows a custom placeholder; the focus goes to another process's
window and back with neither change counted; the placeholder is still there. Two looks agree
and the look between, which saw the other process, is ignored. It needs two missed changes
and a placeholder that outlasts both, so it is low.

Fix: in the branch that refuses a late answer by its process, call
`custom_again(subject, false)`; say "a later look again" in the docs.

**N3. Low, note: when the focused control is the window in front, its line is the whole
window.** `os.rs:308-311` (no focused child: the window itself), `os.rs:371-377`.
By reading.

A browser keeps the keyboard focus on its top-level window, and so does any window the system
names no focused child for. Then "the control's descendants" are all windows inside the window
in front, which is the rule finding 4 was about. Nothing known needs it there: the element's
process is the window's own and passes on that alone. Fix: skip the downward walk when the
control is the window in front, or say in the docs that the line is the whole window in that
case.

**The 250 ms call limit.** It does what finding 3 asked: the staleness of a late answer is now
bounded and documented. Its value is the problem (N1).

**`on_control_line`.** Read line by line: the upward walk ends at the window in front or
returns `false` when it runs off the top (a control outside the window has no line); it
cannot loop; neither enumeration can start from a null window, so the desktop is never walked.
A hostile child window made under the focused control still passes, as before and for the same
reason it did not matter before (finding 4).

**The diagnosis files.** Pruning looks only at regular files named `focus-diagnosis-<number>.txt`
and leaves everything else (run: a folder and a link with such names, and other files, stay).
Files stamped in the future, from a clock that was wrong once, are never the oldest, so they
stay and each real run then displaces the real run before it; cosmetic. A run that reaches
2 MB goes on printing and stops writing. Two runs at once can leave six files until the next
run. None of this is a finding.

**CI.** On `cd4b07d`, besides the Windows failure above, the macOS leg failed in
`inspect::tests::the_bound_counts_from_the_choice_not_from_the_question`: expected the late
answer, got a fresh question. That test ran with the real call limit, its fake call is held
for about 180 ms, and on a loaded runner the call passed 250 ms and was refused as too slow
(the reason `2c60590` gives; the 90 ms freshness bound of that test could fail the same way).
So the new limit met the project's own tests before it met an application. `2c60590` answers
by setting the limit to 10 s in every test that does not test it. That stops the failures;
it also means that no test of `secure` or `inspect` now runs a late answer against the real
limit (the one written for the limit sets its own, 60 ms), and none covers what happens after
a refusal (N1). The test asked for under N1 should use the real limits.

### 6.4 Not checked

- Everything on Windows, again: the two new Windows tests have not yet passed anywhere I could
  see (the run for `2c60590` was in progress).
- The premise of finding 1 on a real engine, for the measured family as much as for the other:
  whether a placeholder with the engine's own framework can stay custom and focused for longer
  than one pause.
- What the measured application's single call takes. N1 says what happens above 250 ms; whether
  that application is above or below it I do not know.
- The watcher's side of N1 was read, not run: the run plays the watcher by its code.
