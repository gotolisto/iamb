# Running the compatibility matrix (task 2.7)

This guide is for the owner. It takes one session of a few hours per OS. The method and the pass
criteria are in [`compatibility-matrix.md`](compatibility-matrix.md); this page only says what to
type. The goal: at least 95 of 100 cells pass on each OS, and every failing cell has an exception
record (an override, a documented limitation or an issue).

You run the harness `iamb-compat` on the machine under test, next to the real service. Do the
Windows steps on a Windows 11 PC and the macOS steps on a Mac. Use a US keyboard layout, because
the harness types `^` after each expansion to check the caret, and on some layouts `^` is a dead
key.

## What you need on either OS

- The 20 apps of your OS (matrix section 1), logged in where needed. Gmail and Google Docs need an
  account. W2, W3, W20, M2, M3 and M20 need two local web pages: a page with a `<textarea>`, a page
  with a `contenteditable` div and a page with `<input type="password">`. Save this as `probe.html`
  and open it in the browsers:

  ```html
  <!doctype html><meta charset="utf-8">
  <textarea rows="12" cols="80"></textarea>
  <div contenteditable style="border:1px solid;min-height:6em"></div>
  <input type="password">
  ```

- The Rust toolchain from `rust-toolchain.toml` (rustup does this by itself on first build).
- The repository checked out at the commit you want to test. Note its commit id; the harness asks.
- A quiet machine: no other app typing, notifications off, power plugged in.

## Windows

1. Open a normal (not elevated) Command Prompt in the repository folder.
2. Build in release mode. Build the service on its own, as the service README asks, so other
   crates cannot add code to it:

   ```bat
   cargo build --release -p iamb-service
   cargo build --release -p iamb-cli -p iamb-testkit
   ```

3. Put the fixture snippets into a data folder of their own:

   ```bat
   target\release\iamb-compat.exe setup --data-dir C:\iamb-compat
   ```

   It imports `;cpt1` to `;cpt5` and prints how to start the service. Do this before step 4: the
   service holds the store's lock.
4. Start the service against that folder and leave the window open:

   ```bat
   target\release\iamb-service.exe --supervise --data-dir C:\iamb-compat
   ```

   A release build has no console window (it is the login item): the command returns at once and
   the service runs in the background, logging to `C:\iamb-compat\logs\`. Stop it with
   `taskkill /im iamb-service.exe /t /f`.

5. In a second Command Prompt, check that it answers:

   ```bat
   target\release\iamb.exe --data-dir C:\iamb-compat list
   ```

   You should see five snippets. Compare with `crates\iamb-testkit\fixtures\compat-expected.txt`.
6. Run the matrix. The harness asks a few facts for the header, then goes app by app (W1 to W20):

   ```bat
   target\release\iamb-compat.exe run --os windows
   ```

   For each app it says what to open and where to click. Open the app, click into an empty field,
   answer the prompt. In automatic mode the harness counts down, types `;cpt1`, waits, reads the
   field back through select-all and copy, and clears the field; it repeats 5 times per type.
   Do not touch the keyboard or mouse while it works. In manual mode you type the abbreviation
   yourself and answer pass or fail. Terminals (W11) and password fields (W19, W20) are always
   manual. Rich surfaces judge T5 by eye (see the notes below).
7. You can stop at any time (answer `q` or press Ctrl+C) and run the same command again: cells
   already in the results file are skipped. `--app W9` runs one app. `--again` runs recorded cells
   again; the new result replaces the old one in the report.
8. Print the report:

   ```bat
   target\release\iamb-compat.exe report --results compat-windows.jsonl > compat-windows.md
   ```

### Secure fields on Windows (W19, W20 and two extra checks)

The build machines have no browser and no password manager, so these cases are only checked here,
by hand. The harness asks the questions of the "Secure rows" note below; this says what to do
first. How a password field is detected, and what is not, is in the module docs of
`crates/iamb-platform-windows/src/focus.rs`.

1. **W20, browser.** Close every Chrome window first, so that Chrome starts without its
   accessibility tree, which is the hardest case. Open `probe.html` and click straight into the
   password field. Type `;cpt1` and a space. Pass: the field holds exactly the six characters and
   the diagnostics show a "secure field" entry (in the first second after Chrome starts, "unknown
   focus" is also a pass). Then the buffer check: type `;cp` in the password field, click into the
   `<textarea>`, type `t1`; nothing may expand. Do both once more reaching the password field
   with Tab from the `contenteditable` div instead of the mouse.
2. **W20, the other way round.** Type `;cpt1` in the `<textarea>` and in the `contenteditable`
   div: both expand. If the first try after starting the browser does not, note how long it took
   until one did (the browser describes the page only once it has been asked).
3. **W19, password manager.** Start the app fresh, so that the master password field is the first
   thing it ever has the focus on. Type `;cpt1` and a space: same pass as in step 1. Lock, unlock
   and repeat once.
4. **Extra, not cells:** steps 1 and 2 in Microsoft Edge and in Mozilla Firefox.
5. **Extra, known limit: a password prompt in a terminal.** In Windows Terminal run
   `runas /user:nobody cmd` and type `;cpt1` and a space at the password prompt. Windows does not
   tell that a console is asking for a password, so today this expands; record what happened.
   Then exclude the terminal and repeat; nothing may expand, and the diagnostics show "excluded
   app". An exclusion is a row of the table `app_exclusion`, written like the setting under
   "Per-app overrides" below, while the service is stopped. Its `app_id` is the name as the
   diagnostics show it, which on Windows is the executable's file name in lower case; the service
   rewrites another spelling (another case, a full path) into that form when it starts:

   ```sh
   sqlite> INSERT INTO app_exclusion (app_id, display_name, created_at)
      ...> VALUES ('windowsterminal.exe', 'Windows Terminal', 0);
   ```

## macOS

1. Open Terminal in the repository folder.
2. Build in release mode:

   ```sh
   cargo build --release -p iamb-service
   cargo build --release -p iamb-cli -p iamb-testkit
   ```

3. Grant the permissions (System Settings, Privacy & Security). Two programs need them:
   - The Iamb service binary, signed with the stable development identity (see the service
     README): **Accessibility** and **Input Monitoring**. Until packaging exists, add
     `target/release/iamb-service` by hand with the `+` button. After granting, quit and start
     the service again.
   - The terminal app you run `iamb-compat` from: **Accessibility**. The harness types through
     System Events, and macOS asks for this once. If a dialog asks whether Terminal may control
     System Events, allow it. If typing silently does nothing, this is the cause.

4. Put the fixture snippets into a data folder of their own:

   ```sh
   target/release/iamb-compat setup --data-dir ~/iamb-compat
   ```

5. Start the service against it and leave the window open:

   ```sh
   target/release/iamb-service --supervise --data-dir ~/iamb-compat
   ```

   If the service reports a missing permission, the worker log says which:
   `tail -f ~/iamb-compat/logs/worker.log`.
6. In a second window, check it answers: `target/release/iamb --data-dir ~/iamb-compat list`.
7. Run the matrix (M1 to M20):

   ```sh
   target/release/iamb-compat run --os macos
   ```

   The steps are as on Windows. Terminal (M11) and the password fields (M19, M20) are manual. For
   M11 the harness also asks you to switch on Secure Keyboard Entry once and check that nothing
   expands.
8. Print the report:

   ```sh
   target/release/iamb-compat report --results compat-macos.jsonl > compat-macos.md
   ```

### Secure fields on macOS (M11, M19, M20 and extra checks)

No build machine has a browser, a password manager or an unlocked desktop, so these cases are
only checked here, by hand. The harness asks the questions of the "Secure rows" note below; this
says what to do first. How a secure field is detected, and what is not, is in the module docs of
`crates/iamb-platform-macos/src/focus.rs`.

1. **M20, browser.** Open `probe.html` in Safari and click into the password field. Type `;cpt1`
   and a space. Pass: the field holds exactly the six characters. The diagnostics should show a
   "secure field" entry; if they show none, note it (while secure keyboard entry is on, macOS
   hands Iamb no key at all, so nothing was buffered, but the app reported no focus change).
   Then the buffer check: type `;cp` in the password field, click into the `<textarea>`, type
   `t1`; nothing may expand. Do both once more reaching the password field with Tab from the
   `contenteditable` div instead of the mouse.
2. **M20, the other way round.** Leave the password field with a click into the `<textarea>` and
   type `;cpt1` and a space: it expands, at the latest about a second after the click (that is
   how long the end of secure keyboard entry can take to be noticed in a browser that sends no
   focus notification). Note it if it takes longer or needs a switch to another app and back.
3. **M19, password manager.** Lock the app, so that the master password field has the focus.
   Type `;cpt1` and a space: same pass as in step 1. Unlock, type `;cpt1` in a search or note
   field of the app: record whether it expands (an app that keeps secure keyboard entry on while
   unlocked does not, and that is a pass for the secure row, noted as a limit).
4. **M11, Terminal.** In Terminal choose Terminal > Secure Keyboard Entry, type `;cpt1` and a
   space at the prompt: nothing expands. Switch to another app and back: the diagnostics show
   "secure field". Switch it off again and type the same: it expands within about a second,
   without leaving the window.
5. **Extra, not cells:** steps 1 and 2 in Google Chrome and in Mozilla Firefox, each started
   fresh, and a `sudo -k; sudo true` password prompt in Terminal with Secure Keyboard Entry off.
   macOS does not say that a terminal is asking for a password, so the last one expands today;
   record what happened. Excluding Terminal (bundle identifier `com.apple.Terminal` in the table
   `app_exclusion`, written while the service is stopped) must stop it, with "excluded app" in
   the diagnostics.
6. **If secure keyboard entry is stuck on.** When nothing expands anywhere and every entry says
   "secure field", some app left secure keyboard entry on. `ioreg -l -w 0 | grep SecureInput`
   names its process id; quit that app and note which one it was.

## End-to-end latency (the probe window)

This measures the 50 ms KPI (last key to expanded text, `docs/test-strategy.md` §6) and writes
its result file. Do it on each OS after the matrix, with the same service and data folder. It
takes about half an hour for the 1,000 trials and needs no answers; do not touch the keyboard or
the mouse while it runs.

1. Build the probe (a test app with one text control; it is never shipped):

   ```sh
   cargo build --release -p iamb-testkit --features probe --bin iamb-probe
   ```

2. Stop the service, add the 1,000 latency snippets (`;laaa` to `;lbml`) to the data folder,
   and start the service again as in the steps above:

   ```bat
   target\release\iamb-compat.exe setup --data-dir C:\iamb-compat --latency
   ```

   (macOS: `target/release/iamb-compat setup --data-dir ~/iamb-compat --latency`.)
3. Check the probe itself: `target\release\iamb-probe.exe --self-test` (macOS:
   `target/release/iamb-probe --self-test`) opens the probe window, types into it and prints
   `self-test: pass` with a time. On macOS the window must be allowed to come to the front.
4. Name the machine and run a short trial first, then the full run:

   ```bat
   set IAMB_BENCH_RUNNER=lab-win11-01
   target\release\iamb-compat.exe latency --trials 20
   target\release\iamb-compat.exe latency
   ```

   (macOS: `IAMB_BENCH_RUNNER=lab-macos-01 target/release/iamb-compat latency`.) The harness
   starts the probe window, which takes the front, and types each abbreviation as ordinary
   keys, the way it types in the matrix (on macOS the terminal needs Accessibility, as above).
   Each failure is printed with its trial id (`t0042-s0417`); `no-key` means the probe did not
   get the keys (another window took the front).
5. The result: the harness prints the p95 of the run and of each length class, `KPI
   e2e-plain-p95: met` or `not met`, and the paths of the files it wrote under
   `target/bench-results/` (`--out DIR` to put them elsewhere): `<run id>.jsonl`, the samples
   and `<run id>.trials.jsonl`. None of them holds snippet text. Copy the numbers into
   `docs/results/2.7-<os>.md` with the machine, OS build and commit, and keep the files.
6. Browser rows (optional, not the KPI): `iamb-compat latency --target browser --open --trials
   100` opens the probe's page (served on `127.0.0.1` only) in the default browser; open the
   printed address in another browser instead to measure that one. Click into the field once
   and keep the tab in front; the run starts when the page has connected.

## What the secure-field and input-method checks cannot see

These are known limits, not failures of a cell: the platform layers treat what they cannot
recognise as an ordinary field. The full table is "Detection limits" in `docs/architecture.md` §8;
record what you see for each one you meet, and check that excluding the app stops it.

- **Password prompts in terminals.** Not detected on Windows; on macOS only while the terminal
  has Secure Keyboard Entry on. An abbreviation typed at such a prompt expands (step 5 of each OS section above).
- **Applications that describe no field** (games, remote-desktop and virtual-machine windows,
  Java applications without the accessibility bridge): a password field there is an ordinary
  field on Windows; on macOS too, unless the app switches secure keyboard entry on.
- **A field that changes kind without a focus change** (a "show password" switch): the change is
  seen at the next focus change (Windows) or when secure keyboard entry changes (macOS).
- **Input methods.** Windows: one that runs on an ordinary layout (Vietnamese Telex, Indic
  transliteration) is not recognised, and a switch the listener does not notice is seen one key
  late. macOS: the first key after switching the input source is read with the previous layout,
  and the press-and-hold accent menu is not seen. Both fail closed otherwise (nothing expands
  while an input method is, or may be, composing).

The remedy in every case is the exclusion list or the pause. An exclusion's `app_id` names the
application as the diagnostics show it (above). The service stores and compares it in that form
(on Windows without regard to case or a directory, on macOS without regard to case) and refuses
an id that no application of the system can have.


- **T4 and the date.** The harness asks for today's local date once and accepts the next day too,
  for a run that crosses midnight. The caret is checked by the `^` it types after the expansion.
- **T5 and the image.** The image part waits for task 3.8: the library import cannot carry
  attachments, and until 3.8 a rich snippet expands as its Markdown source. So the 13 T5 cells on
  rich surfaces fail today. Either run only `--types 1,2,3,4` now and T5 after 3.8, or record
  the failures and give them one exception that points at 3.8. The report says "KPI met: no" while
  any cell is not run.
- **Secure rows (W19, W20, M19, M20).** The harness asks if anything expanded, whether the worker
  log or the diagnostics show a "secure field" entry, and, for T1, whether the half-typed
  abbreviation was forgotten (the buffer check). A pass means nothing expanded.
- **Clipboard.** Before each cell the harness puts a test value on the clipboard and checks it is
  back afterwards, then restores your own clipboard content. Do not copy anything during a run.
- **Privacy.** The results file holds verdicts and notes only. After a failed automatic cell the
  harness asks whether to include the field's text; it can contain other text you have on screen,
  so the answer is no unless you checked.
- **Undo.** After each T1 cell it asks what one Ctrl+Z or Cmd+Z did. This is noted, not gating.
- **Latency.** The 50 ms KPI is not measured by the matrix run (see section 7 of the matrix);
  the report shows `n/a`. The probe window measures it: see "End-to-end latency" above.

## When a cell fails

The harness prints the first problem (extra, missing or different characters, a leftover
abbreviation, a clipboard not restored). For each failing cell choose one of three ways and record
it with `iamb-compat exception`, which writes an exception record into the results file:

1. **A per-app override.** Try a different injection strategy for that app: type instead of paste
   or the other way round, a key delay, `shift+enter` for chat apps, a longer paste wait. The
   overrides are one JSON text in the setting `injection_overrides`, in the same format on both
   systems (see "Per-app overrides" below). Then run the cell again with
   `iamb-compat run --os <os> --app <ID> --again`. It counts as a pass only if the full re-run
   passes. An override that every user of the app needs also goes into the built-in table
   `BUILT_IN` of `crates/iamb-platform-windows/src/injector/strategy.rs` or
   `crates/iamb-platform-macos/src/injector/strategy.rs`, as a change with a test.

   ```sh
   iamb-compat exception --os windows --cell W9/T2 --resolution override \
     --symptom "newline commits the cell" --cause "Enter commits in edit mode"
   ```

2. **A documented limitation.** The app cannot be fixed from our side; add it to the user-facing
   known-issues list with its workaround, then record it with `--resolution limitation`.
3. **An issue.** Open an issue (Iamb or the app's upstream) and record it with
   `--resolution issue --issue <link>`. An issue record needs the link.

Run `iamb-compat report` again: "Failures that still need an override or an issue" must say
`none`.

## Per-app overrides

One JSON object. `defaults` applies to every app, and each entry under `apps` to one app,
changing only the fields it names. The app id is the executable name on Windows (`EXCEL.EXE` or
`excel`, any case) and the bundle identifier on macOS (`com.microsoft.Excel`).

```json
{
  "defaults": { "paste_threshold": 400 },
  "apps": {
    "slack.exe": { "line_break": "shift+enter" },
    "com.microsoft.Word": { "method": "keys", "key_delay_ms": 5 }
  }
}
```

| Field | Values | Windows | macOS |
| --- | --- | --- | --- |
| `method` | `auto` (keys, a paste above `paste_threshold`), `keys`, `paste` | yes | yes |
| `paste_threshold` | characters, 0 to 1000000 | yes | yes |
| `line_break` | `enter`, `shift+enter` | yes | yes, and also `option+enter`, `control+option+enter`, `paste` |
| `key_delay_ms` | pause between keys | 0 to 10000 | 0 to 1000 |
| `paste_wait_ms` | time the app gets to read a paste | 0 to 10000 | 20 to 5000 |
| `paste_keys` | `ctrl+v`, `shift+insert` | yes | no |
| `batch_delay_ms` | pause between input batches, 0 to 10000 | yes | no |
| `chunk_units` | UTF-16 units per key event, 1 to 20 | no | yes |

A field the system does not have, a misspelt field and a value out of range make the service
refuse the whole text; the previous overrides stay in force. Older texts written by development
builds (`default`, `typing`, `shift_enter`, `return`) are refused too.

There is no settings screen yet. Write the setting with any SQLite tool while the service is
stopped (Ctrl+C in its window; on Windows `taskkill /im iamb-service.exe /t /f`), then start it again (step 4 on Windows, step 5 on macOS):

```sh
sqlite3 ~/iamb-compat/library.db
sqlite> INSERT INTO setting (key, value) VALUES ('injection_overrides', '{"apps": {"slack.exe": {"line_break": "shift+enter"}}}')
   ...> ON CONFLICT (key) DO UPDATE SET value = excluded.value;
```

On Windows the file is `C:\iamb-compat\library.db`. The worker log
(`logs/worker.log` in the data folder) then says `injection settings applied`, or
`the injection_overrides setting was refused` with the reason. To remove all overrides, delete
the row: `DELETE FROM setting WHERE key = 'injection_overrides';`.

## Screen lock and sign-out (finding 13 of the 2.8 review)

The worker forgets a form's remembered answers when the session tells it the user left
(architecture §4.5, §5). The service tests cover the mock; these checks cover the systems. Each
check: with the service running and its log open (`logs/worker.log`), do the action and look
for the line `session event <name>`; then unlock or log in again.

| Action | Windows | macOS |
| --- | --- | --- |
| Lock the screen | Win+L: `session event locked` | Ctrl+Cmd+Q: `session event locked` (the distributed notification; the display sleeping says the same) |
| Switch user | Start menu, switch user: `session event locked` (the session locks first) | Fast user switching menu: `session event switched_out` |
| Sign out | Sign out: `session event signing_out` (the worker then ends with the session) | Log Out: `session event signing_out` |
| A credential prompt on the secure desktop | An elevation prompt: `session event locked` (a report too many; only a form's answers are lost) | — |

With a form snippet (a `{field}`), submit it once, lock and unlock, and type it again: the form
opens empty. Record a missing line as a failure of the session watcher, with the OS version.

## The fill-in form panel (task 3.5; finding 16 of the 2.8 review)

The KPI "focus returns correctly across the matrix" and the macOS half of finding 16 (keys typed
into a panel must not expand in the app behind it) can only be checked on the systems. The
end-to-end tests cover the flow on the mock (`apps/desktop/src-tauri/tests/form.rs`).

Setup, after the service is running against the compatibility data folder (steps 1 to 5 of your
OS above):

1. Add a form snippet (one line; on Windows use `^` for the line ends of the Command Prompt or
   type it as one line):

   ```sh
   iamb --data-dir <folder> add ";rlate" --label "Late delivery" "Hi {field name=\"First name\"}, order {field name=\"Order number\"} is late.{field name=\"Add voucher\" type=toggle on=\" Code: \"}{field name=Notes type=area}{field name=Size type=choice options=\"S|M|L\"}"
   ```

2. Build the UI host with its front end (`apps/desktop/README.md`: `pnpm build` in
   `apps/desktop`, then `cargo build --release -p iamb-desktop --features tauri/custom-protocol`)
   and start it against the same folder: `target/release/iamb-desktop --data-dir <folder>`. Keep
   its log open (`logs/desktop.log`).

For each of the 20 apps of your OS, in a text field of the app:

| # | Do | Pass when |
| --- | --- | --- |
| F1 | Type `;rlate` and a space | The panel appears at once in the upper third of the screen, 440 px wide, the cursor in **First name**. The log says `form N: first paint … ms after the request arrived`; note the number. macOS: the app keeps its name in the menu bar (Iamb does not become the active app). Windows: the app's title bar turns inactive |
| F2 | In **First name**, type `;cpt1 ` (a fixture abbreviation) | Nothing expands in the panel or in the app behind it; the worker log has no match for `;cpt1` |
| F3 | Fill in the fields with Tab, Space and the arrow keys only; press Enter | The panel disappears; the cursor is back in the app's field where it was; `;rlate ` is replaced by the finished text |
| F4 | Type `;rlate ` again; press Esc | The panel disappears; the cursor is back in the field; `;rlate ` is still there; nothing was typed or deleted |
| F5 | Type `;rlate `; click into the app's field behind the panel; type `;cpt1 ` there | The panel stays open; `;cpt1` does not expand (a form is open) and the worker log records `form_open`; the keys are the app's (they appear in its field) |
| F6 | Click back into the panel's field and type | The keys go to the panel; nothing expands |
| F7 | Switch to another app (⌘⇥ / Alt+Tab), then back | The panel steps aside while the other app is in front and comes back with the target app |
| F8 | Leave the panel open for a minute, then type `;cpt1 ` into it | Nothing expands (the lease was renewed); the worker log has no `panel_focus_expired` |
| F9 | Close the target window (or tab) with the form open; press Enter | The panel comes back with "Iamb could not get back to the app." and "Your answers are still here."; nothing is typed anywhere; Esc closes it |
| F10 | Tray or menu-bar menu: "Fill in the last form again…" with the cursor in the field | The form opens with the last answers and "Your last answers. Start empty"; ⌘⌫ / Ctrl+Backspace empties it; Enter types the text without deleting anything |

Record per app: F1 time, and pass or fail of F2 to F10, with the OS version and the app's
version. A failure of F3 or F4 (the focus did not come back, or came back to another field)
fails the KPI for that app unless it has an exception record, as for the matrix cells. On macOS,
until the non-activating panel exists (`apps/desktop/README.md`, "The macOS panel"), F1's menu-bar
check fails by design; record it as a known limitation and check the rest.

## The search palette (task 3.6; finding 16 of the 2.8 review)

The palette is the second panel that must hand the keyboard and the focus back correctly, and the
macOS half of finding 16 applies to it as to the form: keys typed into the query must not expand
in the app behind it. The end-to-end tests cover the flow on the mock
(`apps/desktop/src-tauri/tests/palette.rs`).

Setup: as for the form (the service running against the compatibility folder, the UI host built
with its front end and started against it). The UI host offers the default shortcut,
Ctrl+Shift+Space, to a service that has none; if the system refuses it, the log says so
(`the default palette shortcut was not registered`) and Settings → Shortcuts has the choice. Add a
snippet whose label has a word the abbreviation does not (`iamb add ";rret" --label "Return label"
"You can return any part within 30 days."`).

For each of the 20 apps of your OS, with the cursor in a text field of the app:

| # | Do | Pass when |
| --- | --- | --- |
| P1 | Press the shortcut | The palette appears at once in the upper third of the screen, 620 px wide, the cursor in the query box, nothing typed in it, the hint "Type to search by abbreviation, label, text or tag." The log says `palette N: first paint … ms after the request arrived`; note the number (under 100 ms). macOS: the app keeps its name in the menu bar. Windows: the app's title bar turns inactive |
| P2 | Type `ret` | The first row is "Return label" with the abbreviation `;rret`, the matched letters heavier; "N of M" shows at the right; the strip below shows the text it would type |
| P3 | Type `;cpt1 ` in the query | Nothing expands, in the palette or in the app behind it; the worker log has no match for `;cpt1` (the macOS half of finding 16) |
| P4 | Clear the query, type `ret`, press Enter | The palette disappears; the cursor is back in the app's field where it was; the text is typed there; nothing was deleted. Pause Iamb first and check that this still works |
| P5 | Open it again and press Esc | The palette disappears; the cursor is back where it was; nothing was typed or deleted. The next time it opens, the query is empty |
| P6 | Open it; press the shortcut again | The palette closes and the focus is the app's |
| P7 | Open it; click into the app's field behind; type `;cpt1 ` there | The palette stays; the keys are the app's and `;cpt1` expands there; click back into the query and type: nothing expands |
| P8 | Open it; switch to another app (⌘⇥ / Alt+Tab), then back | The palette steps aside and comes back with the target app |
| P9 | Open it, leave it for a minute, then type `;cpt1 ` into it | Nothing expands (the lease was renewed); the worker log has no `panel_focus_expired` |
| P10 | Pick a snippet with a `{field}` | The palette is replaced by the fill-in form in the same place; the form's first field has the keyboard; nothing the palette held remains (type `;cpt1 ` in the field: nothing expands) |
| P11 | In a password field of the app, open the palette and pick a snippet | A refusal in the diagnostics (`refused_secure_field`) and nothing typed. The notice of flow 5 does not exist yet (`apps/desktop/README.md`); look in "Why didn't it expand?" |
| P12 | Close the target window with the palette open; press Enter on a snippet | The palette stays with "Iamb could not get back to the app. Click where the text should go and press Enter again."; Esc closes it |
| P13 | Tray or menu-bar menu: "Search snippets…" | The palette opens for the app that was in front when the menu opened (Windows: the menu may have taken the focus; note which window the text then goes into) |
| P14 | In the library window, press Ctrl+K (⌘K) | The palette opens over the library window; Enter on a result selects that snippet in the library; nothing is typed |
| P15 | Submit a form, then open the palette with nothing typed | The first row is "Fill in “…” again"; Enter opens the form with the last answers |
| P16 | Type a word that matches nothing | "No snippet matches “…”." and one row "New snippet labelled “…”"; Enter opens the library with a draft whose label is the word |

Record per app: P1 time, and pass or fail of P2 to P16, with the OS version and the app's
version. A failure of P4 or P5 (the focus did not come back, or came back to another field) fails
the focus KPI for that app unless it has an exception record, as for the matrix cells. On macOS,
until the non-activating panel exists (`apps/desktop/README.md`, "The macOS panel"), P1's
menu-bar check fails by design; record it as a known limitation and check the rest.

## The installed helper and the UI host (macOS; finding 1 of the beta-gap review)

The helper app holds the user's Accessibility and Input Monitoring permissions (ADR-0021), and
the system judges a program the helper starts as its own child as the helper. So the helper
never starts the UI host, or anything else, as its child: it has the system open `Iamb.app`
(`/usr/bin/open -g -W -a`), and a release build refuses `--ui-host`
([`docs/reviews/beta-gaps-security-review.md`](reviews/beta-gaps-security-review.md), finding 1;
`crates/iamb-service/src/supervisor.rs`, "The UI host"). These checks need the installed, signed
app with Start at login on and both permissions granted to "Iamb Service".

| # | Do | Pass when |
| --- | --- | --- |
| H1 | Log out and in. Run `ps -o pid,ppid,command -ax \| grep -i iamb` | `iamb-desktop`'s parent (PPID) is 1 (launchd), not the pid of `iamb-service --supervise` |
| H2 | `sudo launchctl procinfo <pid of iamb-desktop> \| grep -i responsible` | The responsible process is `iamb-desktop` itself, not `Iamb Service` |
| H3 | Quit Iamb from the menu bar, wait a minute | It stays quit; `logs/supervisor.log` says `ui host … ended (Quit); not restarted`. Start it again, then `kill -SEGV <pid of iamb-desktop>`: it comes back within a few seconds. Then log out and back in, and quit it with `osascript -e 'quit app "Iamb"'`: the logout is not held up, and after each `logs/supervisor.log` says `ended (Quit); not restarted` (the quit marker is left when the system quits the app) |
| H4 | `open -n -W -a "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app" --args --supervise --ui-host /bin/sh --data-dir /tmp/iamb-h4`, then the same with `--platform mock` added before `--ui-host`, then `--supervise --platform mock` alone (no `--ui-host`, no `--data-dir`), and finally `open -n -W --env HOME=/tmp/iamb-h4home -a "…/Iamb Service.app" --args --supervise` | Each time the helper exits at once (no new `iamb-service` in `ps`, nothing in `/tmp/iamb-h4/logs` or under `/tmp/iamb-h4home`): a release build refuses `--ui-host` on either platform, `--platform mock`, a data root other than the default, and a default built from a `HOME` that is not the account's (the re-verification of finding 1) |
| H5 | The child check itself, with a development helper granted both permissions (a copy of `Iamb Service.app` whose `Contents/MacOS/iamb-service` is a debug build, `cargo build -p iamb-service`, signed ad hoc with `codesign -f -s -`): save as `/tmp/probe.sh` the script `#!/bin/sh` / `/usr/bin/swift -e 'import CoreGraphics; print(CGPreflightListenEventAccess(), CGPreflightPostEventAccess())' > /tmp/probe.txt`, make it executable, and start the development helper with `open -n -W -a <its path> --args --supervise --platform native --ui-host /tmp/probe.sh --data-dir /tmp/iamb-h5` | `/tmp/probe.txt` says `true true`: a child of the helper gets the helper's keyboard access, which is why the release helper starts no child. Then run `open -W -a Terminal /tmp/probe.sh` from a Terminal that has no grant of its own: `false false`. Record both answers; `true` in the second, or `false` in the first, changes the finding and must be reported |

## Delivering the result

1. Run `iamb-compat report --results compat-<os>.jsonl` for each OS.
2. Paste the output into `docs/results/compat-<os>-<date>.md`, for example
   `docs/results/compat-windows-2026-11-20.md`. The report already has the header line, the table
   and the exceptions table of the matrix template.
3. Keep the `.jsonl` file next to it, or attach it to the pull request.
4. The KPI is met on an OS when the report says `KPI met: yes`: at least 95 of 100 cells pass and
   every failing cell has an exception record. Check, on each OS, that the log folder has no typed
   text in it (the canary scan of `test-strategy.md` section 5.3 is run separately).
