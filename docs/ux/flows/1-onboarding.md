# Flow 1: Onboarding

Mock-up: [`../mockups/onboarding.html`](../mockups/onboarding.html) · Specification:
[`../spec.md`](../spec.md) §4.6 · Built in task 3.7

## Goal

A person who has just installed Iamb grants what the system asks for, saves a first snippet and
sees it expand, with no help and in under two minutes. They leave knowing three things: a
snippet expands when the word is finished with a space or punctuation (not Enter or Tab,
ADR-0026); what they type stays on the computer; and where Iamb lives when the window is
closed.

## Entry points

| From | Opens at |
| --- | --- |
| First start after installation | Step 1 |
| Menu → "Set up Iamb" (offered until step 5 has been reached once) | The first unfinished step |
| "Fix permissions" (menu, library banner) | Step 2 alone, with "Done" in place of "Continue" |

One window, 560 × 600, titled "Welcome to Iamb". Closing it at any point leaves Iamb running.

## Steps

macOS has five steps; Windows has four, because it needs no permission (the progress line says
"Step n of 5" or "Step n of 4").

### Step 1: Welcome

| | |
| --- | --- |
| Heading | **Type a few letters. Get the whole thing.** |
| Text | Save the text you type again and again as a snippet. Type its abbreviation in any app and Iamb replaces it. |
| Example | `;em` → ines@brindlewood.example (illustration only) |
| Point 1 | **What you type stays on this computer** · Iamb holds your last few keys in memory to spot an abbreviation, then forgets them. It never saves or sends what you type. |
| Point 2 | **It stays out of password fields** · And out of any app you tell it to leave alone. |
| Actions | [Get started ⏎] (primary, focused) · link: "I have snippets to import" |
| Keys | Enter continues. Tab reaches the link. |
| Service | `GetStatus` (is the service there?), `GetPermissions` (when both are `granted` or `not_required`, step 2 is skipped), `GetSettings` (`ui.onboarding_done`) |

"I have snippets to import" opens the system's file dialog (JSON or CSV), runs
`Import(path, format)`, reports "Imported 212 snippets." and continues with step 2 or, on
Windows, step 4 with one of the imported snippets as the example. A library that already has
snippets skips step 3.

### Step 2: Permissions (macOS only)

| | |
| --- | --- |
| Heading | **Let Iamb see and type keys** |
| Text | macOS asks before any app may do this. Turn on Iamb in two lists. This window updates by itself. |
| Row 1 | **Input Monitoring** · Lets Iamb notice when you type an abbreviation. · state "Not granted yet" · [Open settings] |
| Row 2 | **Accessibility** · Lets Iamb type your snippet for you, and recognise password fields so it can stay out of them. · state "Not granted yet" · [Open settings] |
| Help | In System Settings, turn on the switch next to **Iamb**. macOS may ask for your password or fingerprint. |
| Actions | [Back] · "Skip for now" · [Continue] (disabled until both are granted) |
| When one is granted | Its row turns green: tick, "Granted", no button. A screen reader hears "Input Monitoring: granted." |
| When both are granted | Text becomes "Both are on. You can change them at any time in System Settings." Focus moves to [Continue ⏎]. |
| Keys | Enter opens the settings page of the first permission still missing; Tab moves between rows. |
| Service | `OpenPermissionSettings(input_monitoring)` or `(accessibility)`; `GetPermissions` every second while the step is visible; `GetStatus` for `listener` |

The permission state always comes from the service's worker, which is the program that holds
the grants (ADR-0021); the UI never asks the system about itself. The window moves beside
System Settings when that opens, so the two rows stay in view.

"Skip for now" continues to step 3. Step 4 is then replaced by "Iamb cannot expand yet" with
[Fix permissions], and the menu and library carry the same message until both are granted.

### Step 3: First snippet

| | |
| --- | --- |
| Heading | **Make your first snippet** |
| Text | Your email address is a good start. Change both sides to anything you like. |
| Field 1 | "When I type" · filled with `;em` |
| Field 2 | "Iamb types" · empty, focused; placeholder "your email address" |
| Tip | Tip: begin abbreviations with a character you rarely type before a word, such as `;`, so one never fires by accident. |
| Actions | [Back] · [Save and try it ⏎] |
| Keys | Focus starts in "Iamb types". Enter saves; Shift+Tab goes back to the abbreviation. |
| Service | `ListFolders` (the root folder "Snippets"), `PutSnippet` (plain, trigger `delimiter`, case `exact`, word start on) → `Saved` |

### Step 4: Try it

| | |
| --- | --- |
| Heading | **Try it here** |
| Text | Type `;em` and then a **space**. |
| Test field | Empty, focused, three lines high |
| Help | Iamb expands when you finish the word with a space or with punctuation such as `.` `,` `?` `!` Enter and Tab do not expand a snippet. |
| Switch | "Expand the moment I finish typing `;em`, without a space" (off) |
| Actions | [Back] · link "It does not work" |
| Keys | Typing goes into the test field. Tab reaches the switch; Space flips it. There is no Continue yet. |
| Service | `SetTestField(window)` on entering the step: the one place in Iamb's own windows where typing expands (architecture §4.5). The switch saves the snippet with trigger `immediate` and word start off: `PutSnippet` |

When the field holds the snippet's text, the step changes to its success state:

| | |
| --- | --- |
| Heading | **That is all there is to it** |
| Text | It works the same way in your mail, your browser and everywhere else you type. |
| Status | ✓ Iamb replaced **;em** with your snippet. |
| Actions | [Try again] · [Continue ⏎] (focused) |
| Service | None. The service matched and typed inside its 50 ms budget; the UI sees its own field change. |

### Step 5: Done

| | |
| --- | --- |
| Heading | **Iamb is running** |
| Text (macOS) | It lives in the menu bar, at the top right of your screen. Close this window whenever you like: it keeps working. |
| Text (Windows) | It lives in the notification area, at the bottom right of your screen. Close this window whenever you like: it keeps working. |
| Point 1 | **Forgot an abbreviation? Press Ctrl+Shift+Space** (⌃⇧Space on macOS) · Search your snippets from any app and insert one with Enter. |
| Point 2 | **Need it quiet for a while?** · Pause Iamb from its menu-bar icon. (Windows: "its tray icon") |
| Checkbox | "Start Iamb when I log in" (on) |
| Actions | [Close] · [Open my library ⏎] |
| Keys | Enter opens the library with the new snippet selected; Esc closes. |
| Service | `SetTestField(none)`; `PutSetting(palette_hotkey, …)` when not set; `PutSetting(ui.onboarding_done, 1)`. Start at login is set by the UI host through the system's login-item API. |

## Errors and recovery

| Situation | What the user sees | Recovery |
| --- | --- | --- |
| The service is not running (step 1) | "Iamb's background service is not running." [Start it again] in place of [Get started] | The supervisor starts the worker; the step returns to normal when `GetStatus` answers |
| Abbreviation with a space (step 3) | "An abbreviation cannot contain spaces." | Fix the field |
| Empty content (step 3) | "Type the text Iamb should type for you." | Fix the field |
| Abbreviation in use (step 3; possible after an import) | "`;em` is already used by another snippet. Pick a different abbreviation." (`conflict`) | Fix the field |
| Enter or Tab after the abbreviation (step 4) | "**Enter does not expand a snippet.** Apps act on Enter at once (they send the message or move on) before Iamb could type. Finish with a space or punctuation, or let this snippet expand without one." [Expand without a space] | The button sets the trigger to immediate and clears the field |
| Abbreviation and space typed, nothing happens for 1.5 s, or "It does not work" (step 4) | One banner with the first cause found, in this order: "**Iamb is not seeing the keyboard.** macOS sometimes needs Iamb's helper to restart after a permission changes." [Restart the helper] · "**Iamb is paused.**" [Resume] · "**A permission was turned off.**" [Fix] · "**Your keyboard is composing text.** An input method is on. Switch to a plain layout to try." · "**Something else is in the way.**" [Open “Why didn’t it expand?”] | `GetStatus` (`listener`, `paused`), `GetPermissions`, `RecentDiagnostics`. Restart asks the supervisor to stop and start the worker (about two seconds), then `SetTestField(window)` again. "Skip this step" is always offered |
| The system refuses the shortcut (step 5) | "Choose a shortcut for search" with a recorder in place of point 1 | Any chord the system accepts |
| Both grants present but `listener` false for 5 s (step 2) | "Iamb needs to restart its helper to use the new permissions." [Restart] | As above |
| A permission is taken away later | The menu icon shows a dot; "Iamb cannot expand yet · 1 permission is missing" [Fix permissions…] | Step 2 alone |

## Timing

| Budget | Value |
| --- | --- |
| Install to first expansion | Under 2 minutes (plan, task 3.7) |
| Step 1 | 10 s: one read, one key |
| Step 2 (macOS) | 60 s: two trips to System Settings; the rows update within a second of each switch |
| Step 3 | 20 s: type one address, one key |
| Step 4 | 10 s: four characters; the expansion itself under 50 ms |
| Step 5 | Not counted: the goal is already met |

## Decisions in this flow

- The first snippet is created by the user, with a suggested abbreviation and their own text,
  because the exit gate is "creates a snippet and expands it". A ready-made sample snippet
  would be faster and teach less.
- ADR-0026 is taught twice: in the sentence under the test field, and at the moment the user
  presses Enter. The immediate trigger is offered both times.
- The onboarding promises exactly what architecture §8 guarantees and no more.
