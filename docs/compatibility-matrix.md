# Iamb compatibility matrix

Status: draft for approval · Task 0.5 · Run in task 2.7 and for every release candidate · Method in
[`test-strategy.md`](test-strategy.md)

Text injection breaks differently per UI toolkit, so the matrix picks one representative per
toolkit and surface rather than the 20 most popular apps. KPI: **≥ 95% of cells pass on each OS,
and every failing cell has an exception record.**

## 1. Apps

Versions are recorded in the results table at run time (current stable release of each app).

### 1.1 Windows 11

| # | App | Category | Surface to test | Known injection quirks |
| --- | --- | --- | --- | --- |
| W1 | Notepad | Native text control (Rich Edit based) | Document body | Autosave and tabs; reference row, should always pass |
| W2 | Google Chrome | Chromium browser, plain input | Local test page: `<textarea>` | Synthesised Unicode input works; autofill pop-ups can steal arrow keys |
| W3 | Mozilla Firefox | Gecko browser, contenteditable | Local test page: `contenteditable` div | Different Unicode key handling from Chromium; check emoji surrogate pairs |
| W4 | Microsoft Edge | Chromium browser, web mail | Gmail compose body | Web editor re-renders on input; fast key bursts can reorder; paste path preferred for long text |
| W5 | Google Chrome | Rich web editor | Google Docs document body | Own editing surface; ignores some synthetic events; caret moves need key presses, not caret APIs |
| W6 | Slack | Electron, chat | Message box | Enter sends the message: newlines must be Shift+Enter or paste |
| W7 | Visual Studio Code | Electron, code editor | Editor pane in a `.txt` file | Auto-closing brackets and quotes, auto-indent on newline, autocomplete pop-up on typed text |
| W8 | Microsoft Word | Office suite, word processor | Document body | AutoCorrect (straight to curly quotes, auto-capitalisation) alters typed text; rich paste keeps formatting |
| W9 | Microsoft Excel | Office suite, spreadsheet | Cell in edit mode (F2) | Enter commits the cell; newline in a cell is Alt+Enter; paste outside edit mode replaces the cell |
| W10 | Microsoft Outlook (new) | Office suite, mail client (WebView2) | Compose body | Web-based editor inside a native shell; clipboard restore timing sensitive |
| W11 | Windows Terminal | Terminal | PowerShell prompt running `cat > probe.txt` (read back from file) | Enter executes; Ctrl+V handled by the terminal, may warn on multi-line paste |
| W12 | Visual Studio 2022 | IDE (WPF) | C# editor in a comment block | IntelliSense commits on space and punctuation; auto-format on newline |
| W13 | IntelliJ IDEA Community | Java/Swing toolkit, IDE | Editor in a `.txt` file | Java key translation differs for dead keys and non-BMP characters |
| W14 | LibreOffice Writer | Own toolkit (VCL) | Document body | AutoCorrect similar to W8; own clipboard formats for rich paste |
| W15 | Telegram Desktop | Qt toolkit, chat | Message box | Qt input method handling; Enter sends |
| W16 | Microsoft OneNote | Note-taking | Page body | Rich paste creates a new container when the caret is not inside an existing note container |
| W17 | Remote Desktop Connection | Remote-desktop client | Notepad inside the remote session | Unicode packets vs. scancodes depend on client setting; clipboard is redirected, so restore is two-hop |
| W18 | Windows Settings | WinUI app | Search box | Single-line: newlines dropped; suggestion list reacts to every key |
| W19 | Bitwarden | Password manager (secure surface) | Master password field on the unlock screen | **Expected: nothing expands, nothing is buffered** |
| W20 | Google Chrome | Browser password field (secure surface) | Local test page: `<input type="password">` | **Expected: nothing expands, nothing is buffered**; detected through UI Automation `IsPassword` |

### 1.2 macOS

| # | App | Category | Surface to test | Known injection quirks |
| --- | --- | --- | --- | --- |
| M1 | TextEdit | Native text control (`NSTextView`) | Rich text document body | Reference row; smart quotes and dashes if enabled in system settings |
| M2 | Safari | WebKit browser, plain input | Local test page: `<textarea>` | Key events with Unicode strings accepted; autofill can intercept |
| M3 | Google Chrome | Chromium browser, contenteditable | Local test page: `contenteditable` div | Check emoji and combining marks |
| M4 | Mozilla Firefox | Gecko browser, web mail | Gmail compose body | As W4 |
| M5 | Safari | Rich web editor | Google Docs document body | As W5 |
| M6 | Slack | Electron, chat | Message box | As W6 |
| M7 | Visual Studio Code | Electron, code editor | Editor pane in a `.txt` file | As W7 |
| M8 | Microsoft Word | Office suite, word processor | Document body | As W8 |
| M9 | Microsoft Excel | Office suite, spreadsheet | Cell in edit mode | Newline in a cell is Control+Option+Return; Return commits |
| M10 | Mail | Office suite, mail client (native, WebKit body) | Compose body | Rich paste accepted; plain mode drops formatting |
| M11 | Terminal | Terminal | zsh prompt running `cat > probe.txt` (read back from file) | With Secure Keyboard Entry on, nothing may expand (extra check, not a cell) |
| M12 | Xcode | IDE | Swift editor in a comment block | Autocomplete commits on Return; auto-indent |
| M13 | IntelliJ IDEA Community | Java/Swing toolkit, IDE | Editor in a `.txt` file | As W13 |
| M14 | LibreOffice Writer | Own toolkit (VCL) | Document body | As W14 |
| M15 | Telegram Desktop | Qt toolkit, chat | Message box | As W15 |
| M16 | Notes | Note-taking | Note body | Rich paste supported; checklist formatting may be applied to typed `-` |
| M17 | Messages | Catalyst app | Message field | Return sends; Catalyst text input differs from AppKit |
| M18 | Windows App (Remote Desktop) | Remote-desktop client | Notepad inside the remote session | As W17 |
| M19 | Bitwarden | Password manager (secure surface) | Master password field on the unlock screen | **Expected: nothing expands, nothing is buffered**; secure event input enabled by the field |
| M20 | Safari | Browser password field (secure surface) | Local test page: `<input type="password">` | **Expected: nothing expands, nothing is buffered**; WebKit enables secure event input |

## 2. Snippet types

All five use immediate trigger mode with exact case (delimiter mode is covered by engine tests).
The snippets are the library file `crates/iamb-testkit/fixtures/compat.json` (the store's JSON
import format); the expected results are `fixtures/compat-expected.toml`, and
`fixtures/compat-expected.txt` shows them for a person to compare by eye. Tests keep the three
in step. The harness is `iamb-compat`, run as described in [`compatibility-run.md`](compatibility-run.md).

| Type | Abbreviation | Body | Expected result |
| --- | --- | --- | --- |
| T1 short plain | `;cpt1` | `Kind regards, the Iamb team` (27 chars) | Same text |
| T2 long plain | `;cpt2` | Exactly 500 chars of ASCII prose in 8 lines separated by `\n` | Same text; single-line surfaces (W18) per the fixture's single-line expectation (newlines dropped) |
| T3 Unicode | `;cpt3` | `Café déjà vu — naïve façade, Ærøskøbing, 日本語, 👋🏽 👩‍💻 ✅` | Same text after NFC normalisation; emoji with skin tone and ZWJ sequences intact |
| T4 template | `;cpt4` | `Report for {date fmt="%Y-%m-%d"}: {cursor} - done` | Today's local date (either date if the run crosses midnight); caret at the `{cursor}` position |
| T5 rich | `;cpt5` | Markdown: one **bold** word, one link (`https://example.org/iamb`), one 16×16 PNG image (the image is pending task 3.8: the JSON import carries no attachments, so until then the fixture holds bold and link only) | Rich surfaces: bold run, link with exact URL, one image of 16×16. Plain surfaces: the fixture's plain-text fallback |

Rich surfaces for T5: W4, W5, W8, W10, W14, W16, M1, M4, M5, M8, M10, M14, M16. Every other
non-secure row is a plain surface and expects the fallback.

## 3. Pass criteria per cell

Before each repetition the harness clears the surface, puts a sentinel on the clipboard (text
`clip-sentinel-<n>`; on rich surfaces also an HTML flavour) and focuses the surface.

| Criterion | Applies to | How it is checked | Gating |
| --- | --- | --- | --- |
| Exact text read back | T1–T5 | UI Automation (Windows) / Accessibility `AXValue` (macOS); terminals: `probe.txt`; web pages and Docs: select all + copy after the clipboard check | Yes |
| Abbreviation fully removed | T1–T5 | Read-back contains no character of the abbreviation outside the expected text | Yes |
| No duplicated or dropped characters | T1–T5 | Covered by exact equality; failures classified (dup, drop, reorder) in the log | Yes |
| Caret position | T1–T3, T5 at end; T4 at macro | Harness types `^` after the expansion; it must land at the expected index | Yes |
| Rich structure | T5 on rich surfaces | Select all + copy, inspect HTML flavour: `<b>`/`<strong>`, `href`, one `<img>` | Yes |
| Clipboard restored | All | 500 ms after the last action, clipboard text (and HTML flavour) equals the sentinel | Yes |
| Latency | T1 | Harness injects the last key (T0) and timestamps the accessibility text-change notification that shows the full expansion (T1); 2 ms polling where no notification exists. p95 of 20 repetitions < 50 ms | Yes |
| Undo | All | One Ctrl+Z / Cmd+Z; record whether it removes the whole expansion, part of it, or restores the abbreviation | No, noted |

Repetitions: 5 per cell (20 for T1, for the latency statistic). A cell passes when every repetition
meets every gating criterion.

**Secure rows (W19, W20, M19, M20).** For each type, type the abbreviation into the password field.
The cell passes when the field holds exactly the typed characters (nothing deleted, nothing
inserted), the clipboard is untouched, the diagnostics ring shows a "secure field" entry, and the
buffer check passes: type the first half of `;cpt1` in the field, click into a normal field, type
the second half, and nothing expands.

## 4. Computing the result

- 20 apps × 5 types = **100 cells per OS**; pass rate = passing cells ÷ 100. Secure rows count as
  pass when nothing expands, as defined above.
- KPI met on an OS when pass rate ≥ 95% **and** every failing cell has an exception record.
- A cell fixed by a per-app strategy override counts as pass only after a full re-run of that cell.

## 5. Exceptions

Every failing cell gets an exception record in the results file, and one of:

| Resolution | Where |
| --- | --- |
| Per-app override (injection strategy, key delays, paste vs. type, newline key) | Strategy table in `iamb-platform-<os>`; cell re-run must pass |
| Documented limitation | User-facing known-issues list, with the workaround |
| Upstream or Iamb issue | Tracker issue linked from the record; re-test date set |

Record fields: ID (`E-<os>-<n>`), OS build, app and version, cell, symptom, cause, resolution,
issue link, date found, date re-tested.

## 6. Results template

Copy per run into `docs/results/compat-<os>-<date>.md` (`iamb-compat report` prints it).

```text
Run: <date> · OS: <name and build> · Machine: <model, CPU, RAM> · Commit: <sha> · Tester: <role>

| #  | App (version)      | T1 | T2 | T3 | T4 | T5 | T1 p95 ms | Undo (T1)     | Exception |
| -- | ------------------ | -- | -- | -- | -- | -- | --------- | ------------- | --------- |
| W1 | Notepad (x.y)      | P  | P  | P  | P  | P  | 12        | whole         |           |
| W9 | Excel (x.y)        | P  | F  | P  | P  | P  | 31        | partial       | E-win-1   |
| …  |                    |    |    |    |    |    |           |               |           |

Cells passed: <n>/100 (<n>%) · KPI met: yes/no

Exceptions:
| ID      | Cell   | Symptom | Cause | Resolution | Issue | Found | Re-tested |
| ------- | ------ | ------- | ----- | ---------- | ----- | ----- | --------- |
```

## 7. What the `iamb-compat` harness does differently

The harness runs on one real machine with a person at the keyboard, so it follows the sections
above with these differences. Each one is also a line in its usage guide.

| Matrix says | Harness does |
| --- | --- |
| Read back through UI Automation or `AXValue`; terminals through `probe.txt` | Select all and copy, then compare the clipboard (after the clipboard check). Terminals (W11, M11) and password fields are judged by the person. `probe.txt` is not read |
| Types the abbreviation as ordinary keys | The platform injector marks every event it sends as Iamb's own output, which the listener ignores by design, so it cannot type an abbreviation. The harness types through the system (`SendKeys` on Windows, System Events on macOS) |
| Caret: types `^` after the expansion | Same; the expected text is compared with the `^` at the expected index. Use a keyboard layout where `^` is not a dead key |
| 5 repetitions per cell, 20 for T1 | 5 for every cell, by default (`--reps`); manual cells ask the person to do that many |
| Latency: p95 of 20 repetitions under 50 ms | Not measured. Two processes and a clipboard round trip cannot see 50 ms. The probe window of `test-strategy.md` section 6 owns this KPI. The T1 p95 column prints `n/a` |
| T5 on rich surfaces: inspect the HTML flavour | Judged by the person (bold, link, image). Until task 3.8, rich snippets expand as their Markdown source, so these 13 cells fail; run T5 after 3.8 (`--types 1,2,3,4` before) |
| T3 equal after NFC normalisation | Exact comparison. A cell that differs only in normalisation is a failure to review by hand |
| Sentinel with an HTML flavour on rich surfaces | The sentinel is text only |
| Exceptions in the results file | `iamb-compat exception` writes them; `report` lists failures without one. Not run cells count as not passing, and a run with cells not run is reported as "KPI met: no" |

