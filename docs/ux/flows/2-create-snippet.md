# Flow 2: Create a snippet

Mock-up: [`../mockups/create-snippet.html`](../mockups/create-snippet.html) · Specification:
[`../spec.md`](../spec.md) §4.2 and §4.3 · Built in tasks 3.3 (library, plain-text editor) and
3.4 (template editor)

## Goal

Turn a text the user keeps typing into a snippet without leaving the keyboard: new, abbreviation,
content, save. Anything beyond plain text (a date, a blank to fill in, an optional part) is
inserted as a chip and previewed by the same parser that will expand it.

## Entry points

| From | Result |
| --- | --- |
| ⌘N / Ctrl+N or the New button in the library | A draft in the selected folder |
| Menu → "New snippet…" | The library window opens with a draft in the root folder |
| Search palette, nothing matches → "New snippet labelled “…”" | The library window opens with a draft whose label is the query |
| Onboarding step 3 | A reduced form of the same thing (flow 1) |

## Steps

| # | Screen and copy | Keys | Service |
| --- | --- | --- | --- |
| 1 | **Library window.** Sidebar (All snippets, folders, tags, Library health, Why didn’t it expand?, Settings, status line "Iamb is on"), the list of the selected folder with the filter "Filter Replies" and the button [New ⌘N], and the editor showing the selected snippet. | ⌘N / Ctrl+N | `ListFolders`, `ListSnippets` once, refreshed on `LibraryChanged`; `Health` for the Broken badges |
| 2 | **New snippet.** Header "New snippet", kind "Plain text / Formatted", [Cancel], [Save ⌘S] (disabled). Fields: **Abbreviation** (focused; filled with the folder's prefix, help "Starts with `;r`, the prefix of the folder Replies."), **Label** *optional* (placeholder "Shown in lists and search"), **Content** (placeholder "The text Iamb types for you. Type { to insert a date, a fill-in field and more."), **Folder**, **Tags** *optional* (placeholder "Add a tag"). Footer: "Expands after a space or punctuation · Case: exactly as written · Options". | Tab to Label, Tab to Content. Esc or Cancel discards an untouched draft without asking | None: the draft lives in the window |
| 3 | **Content.** The user types text. Typing `{` opens the insert list: Date "today, or shifted" · Time "now, or shifted" · Fill-in field "ask when expanding" · Optional part "only if a field says so" · Cursor position "where typing continues" · Clipboard "what you last copied" · Another snippet "by abbreviation" · Key press "Enter, Tab, arrows…" · Wait "pause before the next step". The same items are on the toolbar (Date, Fill-in field, Cursor, Clipboard, More). | Type to filter, ↑ ↓, Enter inserts, Esc keeps a plain brace | `ParseTemplate(source)` after every change; the chips are drawn from its tokens |
| 4 | **A chip's card.** For Date: **Format** (a list of today's date written each way, and "Custom…"), **Shift** (a number and a unit: days, working days, weeks, months, years; help "Working days are Monday to Friday. Public holidays are not known."), "Today this gives: Monday 23 March", [Remove] [Done ⏎]. The chip then reads "Date +3 working days". | Enter on a chip opens its card; Tab through it; Enter or Esc closes, caret after the chip | `RenderPreview(source)` of the one macro for "Today this gives" |
| 5 | **Preview.** Under the content: "Preview" · "Asks for 4 values" · [Try the form]. Blanks without a value are grey name tags; an optional part whose toggle is off is left out; key presses are key badges; the cursor position is a caret. Header now shows the label and "Not saved". | ⌘⇧P / Ctrl+Shift+P moves to the preview and back | `RenderPreview(source, values)` 150 ms after the last change |
| 6 | **Options** (closed by default). **Expand**: "After a space or punctuation" · "When the word is finished. Enter and Tab do not count." / "Immediately" · "On the last character. Good for abbreviations that are never part of a word." **Capital letters**: "Exactly as written: only `;rlate`" / "Ignore: `;RLate` works too" / "Ignore, and follow mine: `;Rlate` starts with a capital, `;RLATE` is all capitals". ☑ "Only at the start of a word" · "Does not expand in the middle of other text." | ⌘⇧O / Ctrl+Shift+O; arrows inside a group | Part of the row: `trigger`, `case_mode`, `word_start` |
| 7 | **Save.** Toast: "Saved. Type **;rlate** and a space to use it." (immediate snippets: "Saved. Type **;rlate** to use it."). The row appears in the list, selected; the header loses "Not saved". | ⌘S / Ctrl+S. Focus stays in the editor | `PutSnippet` → `Saved`; then `LibraryChanged(revision)`. The snippet expands from that moment |

The fill-in field's card (step 4, for a field): **Name** ("Shown as the label in the form"),
**Kind** (One line · Several lines · Choice · Toggle), **Starts with** (the default), and for a
choice **Options** (one per line), for a toggle **Types when on** and **Types when off**. The
optional part's card: **Shown when** (a field of the snippet) **is on / is / is not** (a
value). Inserting "Optional part" places "If …" and "End if" around the selection, or around
the caret.

## Errors and recovery

| Situation | Message (under the field) | Saving |
| --- | --- | --- |
| Abbreviation empty | "Type an abbreviation." | Blocked |
| Abbreviation with a space | "An abbreviation cannot contain spaces." | Blocked |
| Abbreviation over 64 characters | "An abbreviation can be at most 64 characters." | Blocked |
| Abbreviation in use | "**;rty** is already the abbreviation of “Thanks for your order” in Replies. Each abbreviation can belong to one snippet." [Show that snippet] | Blocked; the service answers `conflict` if it is tried |
| Immediate, and it is the start of another | "This snippet expands immediately, so **;rl** will fire before you can finish typing **;rlate**. Use a different abbreviation, or let it wait for a space." | Allowed |
| Template error | The sentence for the code (specification §6.1), at the chip or position the service reports; for example `E_UNBALANCED_IF`: "This optional part has a start but no end. Put the caret where it should stop and insert “End if”." [Insert it at the end] | Allowed: "Saved, but it will not expand until the error is fixed." The row is marked Broken |
| Empty content | No message. An empty snippet is allowed (it deletes the abbreviation) | Allowed |
| Leaving with unsaved changes | Dialog "**Save your changes to “Late delivery”?** If you do not save, the changes you made to this snippet are lost." [Do not save] [Cancel] [Save ⏎] | — |
| The snippet was changed elsewhere while it was open (sync, another window) | Banner in the editor: "This snippet was changed on another device." [Show the new version] [Keep editing mine] | Saving keeps the user's text; the other version is not lost once sync records the conflict (Phase 4) |
| The service does not answer | Banner "Iamb is not running. … " [Start it again]; the draft stays in the window | After the restart, Save works |
| Read-only shared folder | Fields are read-only; [Copy to my snippets] in place of Save | The copy is a draft in the root folder |

## Timing

| Budget | Value |
| --- | --- |
| Library list | 60 frames per second with 10,000 snippets (task 3.3) |
| Chips after a change | Within one frame of the `ParseTemplate` answer; the text itself is never delayed by it |
| Preview | Within 150 ms of the last key |
| Save to "can expand" | Under 100 ms: the service swaps its snapshot when the row is written |

## Decisions in this flow

- **Explicit save, no autosave.** A draft can never expand, and a half-edited template never
  replaces a working one. The cost is one more key (⌘S) and one dialog when leaving.
- **Abbreviation before content** in the tab order, because the folder's prefix is already
  there and the abbreviation is the shorter thought.
- **Chips, with the source one step away** ("Show as text"): the syntax of the template
  language is never required and never hidden.
- **A broken snippet can be saved.** The template language says so, and work in progress
  should not be lost; the Broken badge and library health keep it visible.
