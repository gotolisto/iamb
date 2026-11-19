# Flow 5: Search

Mock-up: [`../mockups/search.html`](../mockups/search.html) · Specification:
[`../spec.md`](../spec.md) §4.5 · Built in task 3.6

## Goal

Find a snippet without remembering its abbreviation and type it where the cursor is: one
shortcut, a few letters, Enter. The palette searches abbreviations, labels, content and tags,
and it shows each snippet's abbreviation so the user needs the palette less next time.

## Entry points

| From | Types into |
| --- | --- |
| The global shortcut (default Ctrl+Shift+Space; ⌃⇧Space on macOS) | The window that had the focus |
| Menu → "Search snippets…" | The window that was in front when the menu opened |
| ⌘K / Ctrl+K inside the library window | Nothing: Enter selects the snippet in the library |

## Steps

| # | Screen and copy | Keys | Service |
| --- | --- | --- | --- |
| 1 | **The palette appears** over the app, upper third of the display, 620 px wide, no animation. Query row: a search icon and the placeholder "Search snippets". Below: "Recently used" with up to eight snippets (until the service reports usage: the hint "Type to search by abbreviation, label, text or tag."). When a form was submitted in the last 30 minutes, the first row is "Fill in “Late delivery” again". Footer: "↑↓ move · ⏎ insert · ⌘E edit · esc close". | The shortcut. Pressing it again closes the palette | The shortcut reaches the service's listener; the service captures a focus token and sends `PaletteRequested(focus)`; the UI host shows the pre-warmed panel |
| 2 | **The user types** "ret". Results replace the list at once: "4 of 76" at the right of the query. Each row: label, abbreviation, then folder and first line; matched characters are heavier and tinted. The first row is selected. Under the list, a three-line preview of the selected snippet as it would be typed now. | Typing searches. ↑ ↓ move and wrap; Page Up / Page Down jump; ⌘1 … ⌘9 insert that row | `Search(text, limit: 50)` on every change; an answer to an older query is dropped. Rows are drawn from the snippet list the UI host holds. Preview: `RenderPreview(source)` of the selected snippet |
| 3 | **Enter.** The palette disappears; the app has the focus again; the snippet's text is typed at the caret. Nothing is deleted. | Enter | The UI host hides the panel, returns focus to the token's window, confirms it, then `ExpandSnippet(id, focus)`. The engine waits for the verdict `normal` and types |
| 3′ | **A snippet with blanks.** The row carries the "asks for values" icon and the footer says "⏎ fill in and insert". Enter replaces the palette with the fill-in form in the same place (flow 4). | Enter | `ExpandSnippet(id, focus)` → `FormRequested` |
| 3″ | **Esc.** The palette disappears; the app has the focus again; nothing else happens. | Esc | The UI host returns focus. No message |

What Enter does depends on the selected row, and the footer always says which:

| Row | Badge | Enter | Footer |
| --- | --- | --- | --- |
| Ordinary snippet | — | Types it | "⏎ insert" |
| Has fill-in fields | field icon | Opens the form | "⏎ fill in and insert" |
| Switched off | Off | Types it (off only stops the abbreviation) | "⏎ insert" |
| Held for review (flow 6) | Needs your review | Opens the review in the library window | "⏎ review" |
| Template error | Broken | Opens the snippet in the library window | "⏎ open" |
| "Fill in “…” again" | your last answers | Opens the form pre-filled | "⏎ open the form" |
| "New snippet labelled “…”" (only when nothing matches) | opens the library | Opens a draft in the library window | "⏎ create" |

Rules:

- The query is never stored or logged. The palette opens empty every time.
- Search works while Iamb is paused: pause stops typed abbreviations, not a deliberate pick.
- The palette never resizes while typing: once there is a query it keeps the height of eight
  rows and the preview.
- Results are ranked by the service (abbreviation matches first, then label, content, tag).
  Ties keep a stable order, so the same query always gives the same list.
- While a fill-in form is open, the shortcut does nothing.
- What is typed into the query is not typing into the app: the service must not buffer or
  expand it. On macOS that needs the same signal as the fill-in form (specification §11.2,
  entry 14).

## Errors and recovery

| Situation | What the user sees | Service | Recovery |
| --- | --- | --- | --- |
| Nothing matches | "No snippet matches “warranty”." and one row: "New snippet labelled “warranty”" | `Search` → no hits | Enter creates a draft (flow 2); Esc returns to the app |
| The pick is refused: password field | Notice: "**“Return label” was not inserted** · The cursor is in a password field. Iamb does not type there." | `DiagnosticAdded` `refused_secure_field` for the snippet | Put the cursor elsewhere, search again |
| … excluded app | "You excluded this app." | `refused_excluded_app` | Settings → Apps |
| … a window Iamb cannot inspect | "Iamb cannot see into this window, so it stays out." | `refused_unknown_focus` | — |
| … the user moved to another app | "You moved to another app before the text could be typed." | `request_cancelled` | Search again |
| … an input method is composing | "Your keyboard is composing text. Finish the word and try again." | `refused_composing` | Finish composing |
| … the previous snippet is still being typed | "Iamb was still typing the previous snippet. Try again." | `injection_busy` | Try again |
| … the snippet has an error | "This snippet has an error." [Open it] | `E_…` | Fix it |
| The focus cannot be returned | The palette stays, with one line under the query: "Iamb could not get back to Mail. Click where the text should go and press Enter again." | `ExpandSnippet` is not sent | Enter again, or Esc |
| The service does not answer a search within 300 ms | The previous results stay, dimmed; after 2 s: "Iamb is not running." [Start it again] | — | The supervisor restarts the worker |
| The system refused the shortcut at registration | Settings → Shortcuts shows "Another app already uses this shortcut." The menu item still opens the palette | — | Choose another chord |

## Timing

| Budget | Value |
| --- | --- |
| Shortcut to the palette visible | Under 100 ms (plan, task 3.6) |
| Key to results | Under 50 ms with 10,000 snippets |
| Enter to text on screen | Focus hand-back and confirmation, then the 50 ms expansion budget |

## Decisions in this flow

- **Default shortcut Ctrl+Shift+Space on both systems.** Free in the systems' defaults;
  avoids the input-source shortcuts of macOS and AltGr combinations on Windows.
- **The query is never remembered.** Privacy first; the list of recently used snippets does
  the job of a remembered query.
- **One action per row**, on Enter. No "copy instead", no secondary actions beyond "edit".
- **The abbreviation is shown on every row**, on purpose: the palette teaches the
  abbreviations.
