# Security review of "late answers" in the Windows secure-field check

Date: 2026-11-27 · Commit reviewed: `30452a5` ("Say why a field could not be judged, and wait for
slow applications") · Reviewer: independent, did not write the code and judged it from the code,
not from the commit message

Scope: `crates/iamb-platform-windows/src/focus.rs` and `focus/{inspect,secure,classify,rules,watch,uia}.rs`,
`crates/iamb-platform-windows/tests/browser_engine.rs`, `crates/iamb-platform/src/focus.rs`,
`crates/iamb-service/src/worker/{threads,engine,mod}.rs`, `crates/iamb-ipc/src/messages.rs`
(`WireDiagnostic::detail`), and how the UI shows the detail
(`apps/desktop/src/library/diagnostics/reasons.ts`).

The review changed no file of the tree and added this one. Everything was run on Linux. The
decision itself (`inspect`, `secure`, `classify`, `rules`) is pure code that compiles there: its
48 unit tests pass, and three scratch tests against the same fakes were added outside the tree.
The watcher, the UI Automation probe and the Edge test need Windows and were read, not run.
Every finding says "shown by a run" or "by reading".

## 1. Verdict

The mechanism is carefully fenced, and the fences hold for what they name: another window, a
focus change the watcher counted, no running watcher, a window no longer in front, an answer
older than 10 s, a system password control, and the page of a browser engine that is not built
yet. None of those could be made to come out `Normal`.

One thing the fences do not cover should be fixed before this ships, because it makes a
password field `Normal` where the code before this commit said `Unknown`:

- **Medium, shown by a run:** a late answer is the state of the field when the application got
  round to answering, and it is taken at the next look, up to four seconds later. A field that
  became a password field in between, with no focus change counted, is judged `Normal`, the
  answer is settled, and nothing asks again until the next focus event. A fresh call at that
  moment says `Secure` (finding 1).

The rest is low: the subject of a late answer leaves out which control has the focus
(finding 2), changes the service reports itself are not counted (finding 3), the answer says
nothing about which window it describes (finding 4), and notes on the log line, the wire field
and the Edge test (findings 5 to 7).

## 2. Findings

### Medium

#### 1. A late answer is older than the look that uses it, and "not a password field" is then final

Where: `crates/iamb-platform-windows/src/focus/inspect.rs:182-196` (the late answer is taken
when the next question comes), `:50` and `:185` (its age is counted from the question, for 10 s),
`focus/rules.rs` `LOOK_AGAIN_MS` (the next question comes after 150 ms to 4 s),
`focus/watch.rs:120-127` (a report again does not count as a change).

Shown by a run of `secure::judge` with the module's own fakes, for a browser engine's window
and for any other:

1. The first look times out; the application then answers "an edit field, not a password".
2. The field becomes a password field: the page changes its type (a "show password" switch
   turned back, a sign-in page that turns its one field from name to password), or the focus
   moves and the application raises no focus event for it. The watcher's count is unchanged.
3. The second look, for the same window and count, returns **`Normal`, settled** (`look_again:
   false`). The application was asked once.
4. A third question would return `Secure`. There is none: a settled answer is not asked again,
   and the watcher reports only on events.

So the user types a password into a field Iamb holds to be ordinary: the keys go through the
buffer, and an abbreviation in them expands.

What is new in this, and what is not. "A field that becomes a password field without a focus
change is seen at the next focus change" is a listed limit (architecture §8, "Detection
limits"), and for an application that answers in time it applies from the first answer on. Two
things are new:

- The answer is stale before it is used. A fresh answer describes the field at the moment of
  the look. A late one describes it when the application answered, and then waits for the next
  look: up to 4 s by the pauses, 10 s by the lifetime. The module docs say the late answer is
  "what a fresh call would have said had it been fast enough"; it is what the *earlier*
  question would have said.
- Windows that never got an answer now get this one. Before the commit a slow application
  timed out at every look and stayed `Unknown`, which fails closed. Those are the windows this
  path is for (applications on a browser engine, packaged apps), so the staleness lands exactly
  where sign-in pages live.

Fix, in order of value:

1. Use the answer when it arrives, not at the next pause. Have the given-up thread note when
   the answer came, and have its arrival make the watcher report the window again at once
   (the same path as the timer, without a new count). Then count the answer's age from its
   arrival and take it only when it is fresh (a quarter of a second is ample); an answer that
   has waited longer is dropped and the question asked afresh.
2. Say in the module docs and in §8's table what the answer is: the field as it was when the
   application answered.
3. A test that could fail for this: the scratch test of step 1 to 4 above, asserting that the
   second look is not `Normal` once the answer has waited longer than the bound.

### Low

#### 2. The subject of a late answer leaves out which control has the focus

Where: `crates/iamb-platform-windows/src/focus/secure.rs:105-110`, `focus/inspect.rs` `Subject`.

Shown by a run. The subject is the top-level window and the count. The kind of the focused
control (`Desk::control`: the engine's window, a system edit control, anything else) is read
fresh at every look, and the late answer is classified under the kind read *now*:

- The engine's window has the focus; the first look times out; the engine then answers with
  the page as a whole (the unbuilt page). Under the engine's window that answer is not
  believed, also when it comes late (shown: `Unknown(NotInspectable)`, look again). Good.
- Now the focus moves to another control of the same top-level window that is not the
  engine's (the application's own address bar, a native dialog field of a class Iamb does not
  know), and the look comes before that change is counted. The late "page as a whole, not a
  password" is classified under `Other`, where it is believed: **`Normal`** (shown).

The system raises a focus event for every control with a window of its own, so the count
catches up within milliseconds and the next report asks afresh; the wrong answer lives that
long. It is the one way found to have the unbuilt page believed. A system password control is
not affected: it is `Secure` before any answer is looked at (shown).

Fix: put the focused control's window handle and its kind into `Subject`. Both are read from
the system without waiting for the application, so it costs nothing, and a late answer then
serves only the control it was asked for.

#### 3. Changes the service reports itself are not counted

Where: `crates/iamb-platform-windows/src/focus.rs:307-314` (`focus_now`),
`crates/iamb-service/src/worker/engine.rs:956-964` (the service's own look after keys went to
a window the watcher did not report), `crates/iamb-service/src/worker/threads.rs:45-48`
(`Rejudge`).

By reading. Only the watcher adds to the count. When the service finds by itself that the
focus is in another window (its evidence that the watcher missed a change) it sends that change
to the focus thread, which asks `secure_state` with the count as it was. "No late answer
without a running watcher" tests whether the watcher's thread lives, not whether its hooks
still deliver. With hooks that stopped delivering, a window left and entered again within ten
seconds is asked with the same window and the same count as before, and a late answer about
its earlier field is taken. It needs a watcher that fails without ending, so it is unlikely,
but it is the case the service's own look exists for.

Fix: add one to the count in `focus_now` (every reading the service turns into a report), or
give `secure_state` a marker from the caller that a report is a change and not a look again,
and take a late answer only for a look again.

#### 4. The answer does not say which window it is about

Where: `crates/iamb-platform-windows/src/focus/uia.rs:224-241`, `focus/classify.rs`
(`Observation`).

By reading. The probe asks for the system's focused element, whatever window it is in, and the
observation holds two facts about the element and nothing of where it is. For an answer in time
the two "still in front" checks around the call tie it to the window. For a late answer the
call ran at some moment between the two questions, and what ties it to the window is the count
alone: correct while every change of foreground is counted, which the watcher's "always
reported" rule and its timer provide, and nothing else if one is ever lost. Fix, as a second
line: read the element's process id with the other two properties and take the answer only
when it is the target window's process (or, for an engine, a process of the same root window).

#### 5. The log line and the wire field

Where: `crates/iamb-service/src/worker/threads.rs:72-79`, `crates/iamb-ipc/src/messages.rs`
(`WireDiagnostic::detail`), `apps/desktop/src/library/diagnostics/reasons.ts:223`.

By reading. Neither leaks typed content or the application today: the detail is one of five
fixed words from `UnknownReason::code`, set in one place (`engine.rs`, only for the two
"unknown focus" codes and only while that verdict is in force), and the log line holds the word
and nothing else. Three notes:

- **The field is free text on the wire and the UI shows what it does not know as it came.**
  That the detail is never content holds by the one call site, not by its type, unlike
  `Observation`, which cannot hold content by construction. Make it an enumeration on the wire,
  or have the UI show only the words it knows.
- **One log line per unknown verdict.** A window that cannot be judged gives up to six lines
  per focus event (the first look and five more), and an elevated window one per focus event
  for as long as the user works in it. That is a timed trace of when the user moved between
  fields of such windows, on disk, and it pushes other lines out of five files of 1 MB. Log
  the reason when it changes for the window in front, not at every look.
- The diagnostic itself names the application, as before and as §8 rule 4 allows; the detail
  adds nothing to that.

#### 6. The Edge test passes when it did not run, and does not test a change

Where: `crates/iamb-platform-windows/tests/browser_engine.rs:126`, `:138`, `:147-162`.

By reading. It is ignored in CI, which is right for a test that needs a desktop. Run by hand:

- without Edge, or when the window does not come to the front, it prints "SKIPPED" and
  passes. A run asked for with `--ignored` that could not look should fail.
- it asks two pages that never change: a text area must be `Normal`, a password field
  `Secure`. The case this commit makes possible, finding 1, is not in it: a page whose field
  is a text field at first and a password field a moment later (by script, with no focus
  change), where the answer after the change must not be `Normal`. Add that page; it is the
  test that would show whether the engine raises an event for the change and how late the
  answer is.
- it asks every 150 ms itself and does not use the watcher's reports again, so the pauses of
  `LOOK_AGAIN_MS`, up to 4 s, which decide how stale a late answer is, are not what it
  measures.

#### 7. Smaller

- **The 7.5 s of looks** change nothing for safety: the state is `Unknown` and the engine
  suspended throughout, and each report clears the buffer.
- **The unit tests** cover each fence once (same subject, another window, a later count, no
  watcher, not in front, too old, no subject). Missing: the two cases of findings 1 and 2.

## 3. What held

Shown by a run of the pure code unless marked.

- **A focus change the watcher counted.** A question with a later count never takes the late
  answer and asks afresh (the module's test, and by reading: the count is raised before the
  change is reported, `watch.rs`, so the question about a change cannot be served an answer
  from before it).
- **A change reported late.** Until its report arrives the earlier verdict stands, as for any
  answer; the report then asks afresh. The late path adds nothing to that window.
- **Another window.** A late answer is dropped by a question about another window, and by a
  window that is not in front before or after the look.
- **A window whose handle is used again.** It comes to the front with a foreground event,
  which is always counted (reading).
- **No watcher.** Without a running watcher the question names no subject, and a late answer
  that was waiting is dropped.
- **The unbuilt page.** Under the engine's own window it is not believed, late or not
  (finding 2 is the other control).
- **A password answer that comes late is `Secure`**, and a system password control is `Secure`
  whatever answer waits.
- **Races between the two threads.** The late answer is taken out of its slot under the lock
  and waited for outside it; a question that finds none asks afresh; a thread that was still
  starting leaves no late answer, and one that skipped its question ends the wait with "ask
  afresh" (reading). `secure_state` has one caller in the service, the focus thread.
- **Age.** After 10 s from the question the answer is not taken (finding 1 is about the ten
  seconds themselves).
- **The detail and the log.** Fixed words only; no typed text, no application, no window.

## 4. What could not be checked

- **Anything on Windows:** whether a browser engine raises a focus event for a change made
  while its tree is being built or for a field whose type changes; how late its first answers
  are and what they describe; the watcher's delivery under load; the Edge test.
- The UI's rendering of the detail was read, not run.

Tests run: the 48 unit tests of `focus::` in `iamb-platform-windows` (pass), and three scratch
tests on the same fakes for findings 1 and 2 and the system password control, built outside the
shared target directory.

## 5. Fixes

Made after the review, on top of `4fa8b00`, not committed. Everything was run on Linux. The pure
code of the decision is tested there; the watcher, the probe and the Edge test were compiled for
Windows (`clippy --target x86_64-pc-windows-msvc`) but not run.

| Finding | Fix | Test (fails without the fix) |
| --- | --- | --- |
| 1 (medium) | The inspection thread puts its answer in a mailbox with the time it **arrived**. When the question it was asked for had stopped waiting, the arrival calls the question's wake, which posts `WM_LATE_ANSWER` to the watcher thread. The watcher then reports the window again **at once** (marked as a report again, no new count), if it is still the window in front and the one last reported. A late answer is taken only within `LATE_ANSWER_FRESH` (250 ms) of its arrival; an older one is dropped and the application asked afresh. The 10 s lifetime counted from the question is gone. The module docs of `focus.rs` and `inspect.rs` and the table of architecture §8 now say what a late answer is: the field as it was when the application answered, used within a quarter of a second, not "what a fresh call would have said". (`inspect.rs` `Mailbox`, `observe`; `watch.rs` `late_answer`, `Running::late_answer_wake`; `os.rs` `WM_LATE_ANSWER`) | `inspect`: `a_late_answer_wakes_the_watcher_and_serves_the_report_it_makes`, `a_late_answer_that_waited_too_long_is_not_taken`, `the_bound_counts_from_the_arrival_not_from_the_question`. `secure`: `a_late_answer_that_waited_is_not_taken_and_a_new_password_field_is_secure` (your steps 1 to 4: the second look is `Secure`, the application asked twice), `a_slow_application_is_judged_by_its_late_answer_at_once` |
| 2 | `Subject` holds the focused control's window handle and its kind besides the top-level window and the count. `Desk::control` returns the control's handle, its process and its kind (`secure::Control`), all read from the system without waiting for the application. | `secure`: `a_late_answer_is_not_taken_for_another_focused_control` (your scenario: the late unbuilt page, the focus then on a control of kind `Other`, on another control of the engine's kind, or on another handle: `Secure` from a fresh call each time). `inspect`: `a_late_answer_never_serves_another_window_control_or_count_or_a_change` |
| 3 | Both options. A late answer is taken only by a question for a **report the watcher made again**: before such a report (after a pause, or for an arrival) the watcher marks it with the window and the count (`rules::Counts`), and `secure_state` takes the mark; a question that finds no mark for its own window and the current count (a change, the service's own reading, a re-judge after an exclusion change) never takes one. In addition `focus_now`, the service's own reading after keys went to a window the watcher did not report, counts as a change and clears the mark. Late answers are kept and used only while the watcher's thread runs. | `rules`: `a_report_again_is_marked_for_its_window_and_count_and_taken_once`, `a_change_clears_the_mark_of_a_report_again`. `secure`: `a_late_answer_is_not_taken_by_a_change_after_a_count_or_without_a_watcher` |
| 4 | The probe reads the element's `ProcessId` (`IUIAutomationElement` slot 20, checked at compile time like the other slots) with the two other properties; `Observation::Element` carries it. A late answer is believed only when that process is the window's or that of the focused control's window, the process the system names for the engine's window (`GetWindowThreadProcessId`); anything else, and process 0, is `Unknown(NotInspectable)`. Process ancestry ("a known child") is **not** read: it cannot be told reliably (a parent may have ended and its id been given to another process, and the parent id the system keeps is not checked against that), so the fail-closed choice is the two processes the system names for the two windows. The check applies to late answers only; an answer in time stays tied to the window by the two "in front" checks around the call. | `secure`: `a_late_answer_about_an_element_of_another_process_is_not_believed`, and the web view case in `a_slow_application_is_judged_by_its_late_answer_at_once` (an element of the engine's process is taken) |
| 5 | The wire field is `Option<DiagnosticDetail>`, an enumeration of the five words with `#[serde(other)] Other`, so a word of a newer service decodes (the entry still arrives) as `Other` and nothing else can be carried. The UI host passes on only the five words; the page shows only the words it knows, and nothing for any other value. The worker logs the reason only when it changes for the window in front (a new window or a new reason), not at every look or re-judge. | `iamb-ipc`: `the_detail_of_a_diagnostic_is_one_of_its_words`; golden `response_diagnostics.json`. `codes.test.ts`: "names the technical reason of an unknown focus in words" (unknown values, `__proto__`, markup: no line). `iamb-service` `end_to_end`: `an_unknown_focus_says_why_the_field_could_not_be_judged` (one log line per window and reason; a re-judge adds none; no application name) |
| 6 | The Edge test fails when it cannot look (no Edge, no window in front) unless `IAMB_ALLOW_SKIP` is set. It judges the watcher's own reports (a sink that asks `secure_state` for each report, as the service's focus thread does), so the reports again and the late-answer reports are what it measures. A third page turns its text field into a password field 8 s after it loads, with no focus change; from 2 s after the turn no answer, asked directly or from a report, may be `Normal`, and one must be `Secure`. | Manual, Windows: `cargo test -p iamb-platform-windows --test browser_engine -- --ignored --test-threads 1` |
| 7 | The unit tests of findings 1 and 2 are above. They were checked against the fixed code with each fix taken out again: without the freshness bound, `a_late_answer_that_waited_too_long_is_not_taken` and `a_late_answer_that_waited_is_not_taken_and_a_new_password_field_is_secure` fail; without the wake, the wake tests and every `secure` test that waits for an arrival fail; with a `Subject` compared by window and count only, `a_late_answer_is_not_taken_for_another_focused_control` and `a_late_answer_never_serves_another_window_control_or_count_or_a_change` fail. | — |

What remains for a Windows machine: that a browser engine's late answers arrive and are used
within the bound (the Edge test prints the reasons and times); whether the engine raises a focus
event when a field changes its type; that `ProcessId` of a web view's element is the engine's
browser process, as assumed for finding 4 (if it is not, late answers there are
`NotInspectable` and the window falls back to fresh answers: fail closed).

### 5.1 After the re-verification (section 6)

Made on top of `31e4472`, not committed; run on Linux, the Windows code compiled and linted for
Windows only.

| Item | Fix | Test |
| --- | --- | --- |
| 6.3 a, the bound started at the last of four calls | The fallback the re-verification allows: the bound is counted from when the focused element was **obtained**. The probe notes the time right after `GetFocusedElement` returns, before the three property calls, and hands it on with the observation (`inspect::Probed::chosen`); the mailbox keeps that time, and `observe` takes a late answer only while `chosen.elapsed() <= LATE_ANSWER_FRESH`. One cached request (`CreateCacheRequest`, `AddProperty`, `GetFocusedElementBuildCache` and the cached getters) was not done: it needs three more hand-declared method tables (`IUIAutomationCacheRequest`, and slots 12 and 20 of `IUIAutomation` and the cached getters of `IUIAutomationElement`), whose slots can be counted but not run here, and a wrong slot is a call into the wrong method; the fallback closes the finding the same way, because the properties are read from that same element object. Docs: `focus.rs` and `inspect.rs` module docs, architecture §8's table. | `inspect`: `the_bound_counts_from_when_the_element_was_chosen` (a fake step that names its element at once and gives its properties four bounds later: the answer, taken at once after its arrival, is refused and the application asked afresh). It fails with the bound counted from the arrival (checked by putting `Instant::now()` back in the mailbox). The earlier test is renamed `the_bound_counts_from_the_choice_not_from_the_question`. |
| L6, a turn while an answer is pending | A fourth Edge page: on F9, which the test sends, the page blocks its main thread for 1.5 s and then turns its field into a password field, while the test asks every 50 ms and the watcher's reports are judged as well. Every answer asked, and every report verdict, from 1.5 s plus 250 ms after the key on must not be `Normal`; one must be `Secure`. Still `#[ignore]`, still fails when it cannot look. It prints how many questions timed out: the engine answers UI Automation from its browser process's copy of the page's tree, so a busy page may not make any question time out, and then the pending-answer case was not reached on that run (the count says so). | Manual, Windows: `cargo test -p iamb-platform-windows --test browser_engine -- --ignored --test-threads 1` |

Checks: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
`cargo clippy --target x86_64-pc-windows-msvc` for `iamb-platform-windows` and `iamb-ipc`, with
and without `probe`, and the testkit's Windows step; `cargo xtask banned-terms` (0 hits);
`cargo test -p iamb-platform-windows` (239 unit tests on the fakes): all pass.

## 6. Re-verification

Date: 2026-11-27 · Commit reviewed: `c600b00` ("Close the strict review's findings on late
answers") · By the reviewer of sections 1 to 4, not the author of the fix · Same limits: run on
Linux only. The pure code of the decision passes its 51 unit tests there, the protocol's
golden tests pass, and a second scratch test on the same fakes was added outside the tree. The
watcher, the probe, the thread message and the Edge test need Windows and were read, not run.

### 6.1 Verdict

The medium finding is closed for what it described: a late answer no longer waits for the next
pause, and one that has waited is not taken. The low findings are closed, one of them partly.
The fixes brought no new medium or high problem. One low remainder of finding 1 is new to
this section (6.3, a), and the Edge test's new page does not exercise the case it was added
for (L6).

### 6.2 The findings

| # | Now | Evidence |
| --- | --- | --- |
| M1, a late answer older than the look that uses it | **Closed** for the wait after the answer arrives; a low remainder inside the call (6.3, a) | Run, with the real bound: the field turns into a password field after the answer arrived; a report again 400 ms after the arrival asks afresh and says `Secure` (the application is asked a second time); at 0 and 100 ms the late `Normal` is taken, which is the quarter second the docs now state. The arrival calls the question's wake in every case. The age is counted from the arrival (`inspect.rs:126`, `:326`); the ten-second lifetime is gone and is not needed |
| L2, the subject left out the focused control | **Closed** | Run: with another control handle, or the same handle now of kind `Other`, the late answer is dropped and a fresh call decides. The unbuilt page that comes late under the engine's own window is still not believed |
| L3, changes the service reports itself | **Closed** | Run: a question that is not a report again (no mark) never takes a late answer, also at once after its arrival. Reading: the watcher marks a report again for its window and the count before it makes it, a change clears the mark, and `focus_now` counts as a change (`focus.rs:337-348`, `rules.rs` `Counts`). A mark taken by another question than the one it was made for does no harm: the subject still carries the count |
| L4, the answer did not say which window it is about | **Closed** | Run: a late answer from another process, or from process 0, is `Unknown(NotInspectable)` with a look again; from the top-level window's process or the focused control's it is taken. Reading: the process id is read with the other properties (`uia.rs:246-258`), and the control's process is read from the system at the look. It applies to late answers only, which is where it was missing |
| L5, the wire field and the log line | **Closed** | The field is an enumeration of the five words and `Other`; a word this version does not know decodes as `Other`, the UI host passes none on for it and the page shows nothing (reading; golden tests pass). The worker logs the reason when the window or the reason changes |
| L6, the Edge test | **Partly** | It fails when it cannot look (unless told to skip) and judges the watcher's own reports. The new page turns its field 8 s after it loads, which is after the answer has settled, so no late answer is waiting at the turn: it shows that a fresh question says `Secure` afterwards, not that a late answer is refused. Turn the field within the first second, or keep the page busy at the turn, so that the turn falls between a late answer's arrival and its use |
| L7, tests | **Closed** | The cases of findings 1 and 2 are unit tests now, and section 5 says each fails with its fix taken out |

### 6.3 The points asked about, and what is new

**The `WM_LATE_ANSWER` wake** (`watch.rs:139-150`, `:327-338`). The answer's arrival posts the
message to the watcher thread, which reports the window again only if it is still in front and
is the window it last reported, and marks that report. Sound by reading: the wake runs outside
the mailbox's lock; a wake set after the answer is already there runs at once; a watcher that
has left its loop gets no message; a message that cannot be posted leaves the window to the
next pause, where the answer is too old and a fresh call is made.

- A thread message can be posted by any process of the user. A forged one makes the watcher
  report the window in front once more: a look, and the engine's buffer cleared. It cannot
  make a late answer be taken that would not be (the subject, the mark's count and the bound
  do not depend on it), and the two older thread messages can be forged the same way. A
  process that floods it stops Iamb expanding, which it can do in other ways. No fix needed;
  noted so that the message never gains a meaning that matters.
- The time between "the thread is alive" and the post is a few instructions; a thread id
  given to another thread in that time would get a message it ignores.

**The 250 ms bound.** Counted from when the whole answer arrived, checked when the answer is
taken, and applied to an answer that was already there; an answer that arrives while the
question waits for it is new by definition. Shown above.

- a. **Low, new: the bound starts when the last of four calls returns, and the element is
  chosen by the first** (`uia.rs:246-258`). The probe asks for the focused element, then for
  three of its properties, each a call into the application. For an application that is busy
  once and then free, the four follow each other at once. For one where every call is slow,
  which is the case the commit describes, the element may have been chosen seconds before the
  answer "arrives". A move of the focus inside the page in that time, with no event, gives
  the properties of the element before. The same holds for an answer in time, but there the
  whole call is under 100 ms. Fix: ask once, with a cache request that brings the three
  properties with the element (`GetFocusedElementBuildCache`), or note the time the element
  was obtained and count the bound from there.

**The process match.** As in the table. Two notes: an element of another window of the same
process passes it, so it is a second line behind the count, as intended; and whether a web
view's elements name the engine's process that owns the focused control is still unverified
(section 5 says so). If they do not, every late answer there is `NotInspectable` and such
windows are back to timing out: safe, but the commit's purpose is not met, and only the Edge
test on a real machine shows which.

**The `Other` catch-all.** Nothing but the six values can be carried, and `Other` is shown as
nothing at both steps. This version never sends it. A detail that is not a string fails the
frame, like any other type error.

**Also checked.** `focus_now` has one caller in the service (its own look after keys went to
an unreported window), so counting it as a change costs late answers only when the service
already doubts the watcher. A report made for an arrival cancels the pause's timer; a look
that still cannot tell starts the next pause, so the retries go on. A late answer has no
overall lifetime any more: one that arrives after a long hang is used only if the window, the
control and the count are still the same and the report comes within the bound, which is the
right rule.

### 6.4 Not checked

Everything on Windows: that late answers of a real engine arrive and are used within the
bound, the thread message, the process id a web view's element reports, whether the engine
raises a focus event when a field changes its type, and the Edge test itself.
