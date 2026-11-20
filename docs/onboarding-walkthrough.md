# Onboarding walkthrough on fresh machines (task 3.7)

This guide is for the owner and the person who observes. It says what to install, what to time,
from which moment to which, what counts as a pass and what to write down. The design is in
[`ux/flows/1-onboarding.md`](ux/flows/1-onboarding.md) and [`ux/spec.md`](ux/spec.md) §4.6; this
page does not change it.

**The KPI** (plan, task 3.7): *install to first expansion under 2 minutes for 3 of 3 testers.*

Nothing here can be measured by a program. A program can only check the service's side of the
steps (the scripted timing in [`test-strategy.md`](test-strategy.md) §8.6, a few milliseconds);
whether a person finds the switch in System Settings, reads one sentence and presses one key in
two minutes is what this run is for.

## Who and how many

- **Testers.** Three people who have never used Iamb and have not seen the window before. They
  need not be technical. They do not know what a snippet is. Do not use the person who built it.
- **One run per OS.** The window has five steps on macOS and four on Windows, and macOS adds two
  trips to System Settings, so the two paths are different tests. The KPI is met per OS when 3 of
  3 testers of that OS pass. (If only one OS can be tested, say so in the result; do not carry
  the result over to the other.)
- **One observer** per tester, who times, says nothing about the product and writes the sheet.

## What to prepare

Every tester starts from a machine Iamb has never been on.

| | macOS | Windows |
| --- | --- | --- |
| Machine | A Mac on macOS 13 or later, as it comes from a fresh account or a restored snapshot | A PC on Windows 11, or a restored snapshot; the user is a normal user, not an administrator account running elevated |
| Clean start | A new local user, or a snapshot taken before Iamb was ever opened (so no Privacy entry, no login item, no data folder of Iamb exists) | The same: a snapshot, or a new local user |
| The build | The installer of the build under test, in the Downloads folder. Note the commit id and the file name | The same |
| Keyboard | US layout. (Other layouts are a second run, not mixed into this one) | US layout |
| Screen | One display. Notifications off. Plugged in. No other app open but the browser that holds this page, if any | The same |
| Recording | A screen recording of the whole run, so times can be read afterwards. It must not record the tester's own text (see "Typed text") | The same |
| Clock | A stopwatch the observer can read to the second, or the recording's clock | The same |

Between testers, restore the snapshot or use another new user. Uninstalling is not enough: a
previous run leaves the Privacy entries and the settings behind, and the second tester would skip
steps.

Check once, before the first tester, that the machine is clean: no Iamb in the Applications
folder or the Start menu, no "Iamb" entry in System Settings under Privacy and Security (macOS),
no data folder of Iamb under the user's home.

## What the tester is told

Read this aloud, and nothing else:

> This is a program that expands short abbreviations into longer text as you type. It is in the
> Downloads folder. Install it and get it to expand something for you. I will time you, and I
> cannot help. Say what you are thinking as you go if you like.

Then the observer keeps quiet. A question from the tester is answered with "What would you
do?". Nothing about the product is explained. If the tester asks what an abbreviation is, that
is a note for the sheet ("did not know the word"), not a question to answer.

## What to time, from which moment to which

Two clocks, started together, each with its own start and end. All times are read to the second.

| Mark | Moment | Clock |
| --- | --- | --- |
| **T0** | The tester double-clicks the installer (the moment the file is opened, not when it was downloaded) | Starts both clocks |
| **T1** | The installer is done and Iamb has started (the installer closes, or the Iamb window appears, whichever comes first) | Install ends |
| **T2** | The onboarding window ("Welcome to Iamb") is on screen | **Clock B** starts here |
| **T3** | macOS only: the second permission turns green ("Both are on") | |
| **T4** | The first snippet is saved (the window moves to "Try it here") | |
| **T5** | The window says **"That is all there is to it"**: Iamb replaced the abbreviation in the test field | **Both clocks stop** |

- **Clock A (the KPI)** is T0 to T5: install to first expansion.
- **Clock B** is T2 to T5: the onboarding itself. The flow's own budget for it is about 100
  seconds on macOS (10 + 60 + 20 + 10, `ux/flows/1-onboarding.md`, "Timing") and under 40 on
  Windows.
- The time a download or a security prompt of the operating system takes before T0 is not
  counted. A prompt after T0 (a "downloaded from the internet" confirmation, an administrator
  password) is counted: it is part of the install, and the sheet notes it.
- Step 5 is not timed: the goal is already met at T5. The observer still lets the tester finish
  it, because it carries part of the result (below).

## The task, step by step (what the observer expects to see)

The observer has this list and ticks it. It is not read to the tester.

1. **Install.** macOS: open the disk image, drag Iamb to Applications, open it. Windows: run the
   installer. Note every prompt that appears and how long the tester takes over it.
2. **Welcome.** The window opens by itself. The tester reads one screen and presses Enter or
   clicks "Get started".
3. **Permissions (macOS only).** Two rows, "Input Monitoring" and "Accessibility". The tester
   presses "Open settings", finds the switch next to **Iamb** (it must be listed as "Iamb" with
   Iamb's icon, `ux/spec.md` §11.2 entry 13), turns it on (a password or fingerprint prompt may
   appear), comes back, and does the same for the second. Note: did the row turn green by itself
   within a second or two? Did the window stay in view next to System Settings? Did "Iamb needs
   to restart its helper" appear, and did the button fix it?
4. **First snippet.** The abbreviation `;em` is filled in. The tester types an email address.
   Use the sample address `ines@brindlewood.example` (see "Typed text").
5. **Try it.** The tester types `;em` and a space in the test field. Note whether they press
   Enter or Tab first (the hint "Enter does not expand a snippet" must appear and help), and
   whether the 1.5-second banner ("Iamb is not seeing the keyboard" and the others) shows up.
6. **Done.** The tester reads where Iamb lives, and leaves the window by "Open my library" or
   "Close". Note which, and whether they found the menu-bar or tray icon afterwards (ask them to
   point at it).

## What counts as a pass

A tester **passes** when all of these hold:

1. Clock A is **120 seconds or less** (T0 to T5).
2. The tester reached "That is all there is to it" **without help**: the observer did not answer
   a question about the product, point at the screen or touch the machine. (A hint given counts
   as a fail for that tester, whatever the time.)
3. The expansion happened in the window's test field **by the tester's own typing** (not a
   demonstration, not the observer's keys).
4. The three things the flow wants them to leave with, asked **after** the run, in the tester's
   own words, are answered correctly:
   1. "Does Iamb keep what you type?" (It stays on the computer and is not saved or sent.)
   2. "How do you make a snippet expand?" (Finish the abbreviation with a space or punctuation;
      Enter and Tab do not.)
   3. "Where is Iamb now, if you close the window?" (The menu bar on macOS, the notification
      area on Windows; they can point at it.)
   A tester who gives one of the three wrongly fails criterion 4; note which, since it is a
   problem with the copy, not with the tester.

**The KPI is met** for an OS when all three testers of that OS pass. A tester who fails does not
get a second try, and is not replaced: the failure is the result. Fix the cause, then run three
**new** testers (a retest with the same people measures memory, not the product).

## What to write down

One sheet per tester. Copy the table into the result file, one row each.

### Timing sheet

| Field | Value |
| --- | --- |
| Tester (initials only) and OS and version | |
| Machine and display scale | |
| Build (commit id and installer file name) | |
| Date | |
| T0 installer opened (clock time) | |
| T1 installed and started | |
| T2 onboarding window on screen | |
| T3 both permissions green (macOS) | |
| T4 first snippet saved | |
| T5 "That is all there is to it" | |
| **Clock A (T0 to T5), seconds** | |
| Clock B (T2 to T5), seconds | |
| Install (T0 to T1), seconds | |
| Help given (yes or no; what was asked) | |
| Pressed Enter or Tab after the abbreviation (yes or no) | |
| The 1.5-second banner appeared (which cause, did its button fix it) | |
| Used "Skip" anywhere (which step) | |
| Question 4.1, 4.2, 4.3 answered correctly (each yes or no) | |
| **Pass or fail** | |

### Notes, whatever the result

Write these as short observations with the time they happened (from the recording), not as
opinions:

- **Where they stopped.** Every pause over five seconds, and what the tester was looking at or
  said.
- **Words that did not land.** Any term the tester said aloud and did not understand
  ("abbreviation", "snippet", "Input Monitoring", "helper", "listener").
- **Wrong turns.** A button pressed that was not the one meant; a window opened by mistake; the
  System Settings pane the tester went to first (macOS).
- **The system's own steps.** Which prompts the OS added (downloaded-file confirmation,
  password, fingerprint, "Iamb would like to ...") and how long each took.
- **What worked without a word.** Rows that turned green by themselves, the focus landing on the
  right button, Enter doing the next thing. These are the things not to break.
- **Anything that failed to work** as the flow says: a row that did not update, the test field
  not expanding with the service running, the library not opening on the new snippet, "Start Iamb
  when I log in" not holding after a restart (check after the run by restarting the machine and
  seeing Iamb in the menu bar or tray).

### Typed text

The test field's content is never logged or stored by Iamb, and the recording must not become a
second place for it. Tell the tester to type the sample address `ines@brindlewood.example` as
the snippet's text, not their own. Do not write what a tester typed anywhere else on the sheet.
Delete the recording once the times are read, or keep it only as long as the result needs it, and
say so in the result.

## The result

After the last tester of an OS, fill in:

| | macOS | Windows |
| --- | --- | --- |
| Testers who passed, of 3 | | |
| Clock A of each (seconds) | | |
| Clock B of each (seconds) | | |
| Longest pause (seconds, which step) | | |
| KPI met (3 of 3) | | |
| Failures by cause (copy, system prompt, permissions, bug) | | |

Each failure becomes an issue that names the step, the cause and the time it cost. Copy the table
and the notes into the review of this task. Where the cause is a system step Iamb cannot change
(a password prompt, a download confirmation), say so, and say how many seconds it took: the
owner decides whether the budget or the step is wrong.

## What to run first, so the tester does not meet a known bug

On the machine, before the first tester, with the build under test:

1. The scripted check of the service's side of the steps (any OS, the mock platform):

       cargo test -p iamb-desktop --locked --test onboarding scripted_first_run_timing -- --nocapture

   It prints the time of each step with the steps automated; see `test-strategy.md` §8.6. It
   must pass; the numbers are only a sanity check.
2. Open the installed app once as the observer on a *different* user or snapshot, and walk the
   steps. This finds a broken installer before a tester does. Then restore the clean state.
