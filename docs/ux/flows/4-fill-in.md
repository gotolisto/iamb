# Flow 4: Fill-in

Mock-up: [`../mockups/fill-in.html`](../mockups/fill-in.html) · Specification:
[`../spec.md`](../spec.md) §4.4 · Design system §4.13 and §4.14 · Built in task 3.5

## Goal

A snippet with blanks asks for them in a small form where the user is typing, takes the answers
from the keyboard alone, hands the focus back to the app and only then types the finished text.
Cancelling changes nothing at all (ADR-0013). The last answers are offered again for 30
minutes and then forgotten.

## Entry points

| From | Difference |
| --- | --- |
| Typing the abbreviation of a snippet that has fill-in fields | The abbreviation stays on screen until the form is submitted, then it is replaced |
| Picking such a snippet in the search palette (flow 5) | Nothing was typed, so nothing is deleted |
| "Fill in “…” again" in the palette, or its shortcut when one is set | The form opens with the last answers; nothing is deleted |
| "Try the form" in the editor's preview (flow 2) | Same panel over the library window; the result goes to the preview and is typed nowhere |

## Steps

| # | Screen and copy | Keys | Service |
| --- | --- | --- | --- |
| 1 | The user types `;rlate` and a space in a mail message. The abbreviation and the space stay where they are. | — | The engine matches, reads the clock and (if the snippet uses it) the clipboard once, opens the single form session and emits `ShowForm`. The service attaches the focus token of the window |
| 2 | **The form appears** in the upper third of the display, 440 px wide, with no animation. Header: the mark, "Late delivery", `;rlate`. Fields in the order they first appear in the snippet: **First name**, **Order number**, ☐ **Add voucher**, **Voucher code**. Footer: "Goes into Mail" · [Cancel esc] [Insert ⏎]. | Focus is in the first field, its content selected | `FormRequested(session, snippet, spec, focus, last_values)` → the UI host shows the pre-warmed panel and answers `FormAck(session)`. On Windows the service grants the UI host the foreground with the request; on macOS the panel does not activate Iamb |
| 3 | **The user answers.** One-line fields, several-line fields, choices (a list of up to six options, a select from seven), toggles. Nothing is required and nothing is validated. If the form opened with remembered answers the footer says "Your last answers. Start empty". | Tab / Shift+Tab between fields; ↑ ↓ or first letters for a choice; Space for a toggle; ⌘⌫ / Ctrl+Backspace starts empty | None. The answers exist only in the panel |
| 4 | **Insert.** The panel disappears; the app has the focus again; the abbreviation is replaced by the finished text. | Enter (⌘⏎ / Ctrl+Enter from a several-line field, and everywhere) | The UI host hides the panel, returns focus to the token's window, confirms that the window has the focus, and only then sends `FormSubmit(session, values)`. The engine waits for the focus verdict `normal`, then emits `DeleteBack` and the rendered actions |
| 4′ | **Cancel.** The panel disappears; the app has the focus again; the abbreviation is still there; nothing was typed or deleted. | Esc, or Cancel | The UI host returns focus, then `FormCancel(session)`. The engine closes the session and emits nothing |
| 5 | **Again, for the next one.** Within 30 minutes the search palette's first row is "Fill in “Late delivery” again · your last answers". Enter opens the form pre-filled. | The palette shortcut, then Enter | Focus back to the app, then `ReopenLastForm` → `FormRequested` with `last_values` |

Details that hold in every step:

- **The moment counts, not the submit.** Dates and the clipboard are those of the moment the
  abbreviation was typed, however long the form stays open.
- **All fields are always shown**, including those inside an optional part that ends up left
  out; the form does not change while it is open (template language §4.5).
- **The panel stays** when the user clicks elsewhere or switches apps. It returns to the front
  with its app. While it is open, other abbreviations with blanks do nothing and the palette
  shortcut does nothing.
- **Remembered answers** live only in the service's worker, in memory, and are cleared after
  30 minutes without use, on screen lock, on sign-out and on restart (architecture §4.5; the
  lock and sign-out part is finding 13 of the 2.8 review, to be closed in task 3.5). The UI
  never keeps them after the panel hides.
- **What is typed into the form is not typing into the app.** The service must not buffer or
  expand keys while the panel has the keyboard. On Windows the panel is Iamb's own window and
  this already holds; on macOS the panel does not activate Iamb, so the service needs to be
  told (finding 16 of the 2.8 review; specification §11.2, entry 14).

## Errors and recovery

| Situation | What the user sees | Service | Recovery |
| --- | --- | --- | --- |
| The focus cannot be returned (the window was closed, or will not take the focus within a second) | The panel returns with a banner: "**Iamb could not get back to Mail.** Nothing was typed. Click where the text should go and press Insert again, or cancel." Footer: "Your answers are still here." | `FormSubmit` is not sent | Insert again (the UI retries the hand-back), or Cancel → `FormCancel` |
| The engine refuses the submit: password field, excluded app, uninspectable window, composing | A notice, no focus taken: "**“Late delivery” was not inserted** · The cursor is in a password field. Your answers are kept for 30 minutes: press Ctrl+Shift+Space where the text should go." | `DiagnosticAdded` with `refused_secure_field`, `refused_excluded_app`, `refused_unknown_focus` or `refused_composing` and this snippet; the values stay available to `ReopenLastForm` | Step 5 in the right place |
| The user moved to another app before the text could be typed | Notice: "… You moved to another app before the text could be typed. Your answers are kept for 30 minutes: …" | `request_cancelled` | Step 5 |
| The previous snippet was still being typed | Notice: "… Iamb was still typing the previous snippet. …" | `injection_busy` | Step 5 |
| The user moved the caret in the app while the form was open | The wrong characters may be deleted (accepted in ADR-0013 and listed in the compatibility matrix) | — | The app's undo |
| The snippet has an error | No form at all: errors are found before a form is shown. The abbreviation stays | `Diagnostic` with the `E_…` code | Fix the snippet (the log's "Open snippet") |
| The UI host is not running | No form; the abbreviation stays | `form_no_ui_host`; the supervisor starts the UI host | Type the abbreviation again |
| The UI host does not acknowledge within 2 s | No form, or a late one that the UI must not show: on a `FormRequested` older than 2 s the UI answers `FormCancel` | `form_not_acknowledged` | Type the abbreviation again |
| A second abbreviation with blanks while a form is open | Nothing | `form_open` | Finish or cancel the open form |
| The snippet was deleted while its form was open | Insert shows a notice: "That snippet was deleted." | `unknown_snippet` | — |
| "Fill in … again" after the 30 minutes | The row is no longer offered; a shortcut press shows the notice "There is no earlier form to fill in again. Answers are kept for 30 minutes." | `no_last_form` | Type the abbreviation |

## Timing

| Budget | Value |
| --- | --- |
| Trigger to the form visible | Under 150 ms (plan, task 3.5): the panel is pre-warmed and hidden; it renders from the event alone |
| `FormRequested` to `FormAck` | Under 2 s or the service abandons the expansion (architecture §4.5); in practice with the first paint |
| Insert to text on screen | Focus hand-back and confirmation, then the 50 ms expansion budget |
| Answers remembered | 30 minutes without use |

## Decisions in this flow

- **No preview of the final text in the form.** It keeps the panel small and instant; the
  author previews in the editor. (Alternative: a collapsible preview line.)
- **Clicking outside does not cancel.** A long answer is too easy to lose.
- **No required fields**, because the template language has none.
- **"Fill in again" lives in the palette** and has no shortcut by default.
- **The footer names the target app**, so that a user who switched apps sees where the text
  will go.
