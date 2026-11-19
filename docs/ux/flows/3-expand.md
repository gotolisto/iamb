# Flow 3: Expand

Mock-up: [`../mockups/expand.html`](../mockups/expand.html) · Specification:
[`../spec.md`](../spec.md) §4.1 and §4.8 · Built in Phases 1 and 2 (the expansion), tasks 3.2
(menu) and 3.9 (diagnostics)

## Goal

The user types an abbreviation in any app and the snippet's text is there, as if they had
typed it. Iamb shows nothing and takes no focus. When it does not expand, the user can find
out why in one place, in one sentence, without Iamb ever having recorded what they typed.

## Entry points

Typing, in any app. The menu and "Why didn’t it expand?" are reached from the tray or menu-bar
icon, or from the library's sidebar.

## Steps

| # | What happens | Keys | Service |
| --- | --- | --- | --- |
| 1 | The user types `;addr` in a mail message. Nothing visible happens. | The abbreviation | The listener hands each key to the engine, which holds at most 66 characters in locked memory (architecture §4.1) |
| 2 | The user types a space. The abbreviation and the space are replaced by the snippet's text, followed by the space. The caret is after it. | Space, or one of `. , ; : ! ? ) ] } " '` | Engine: match (delimiter trigger) → `DeleteBack`, `TypeText`, `Expanded`. Injector: types it. No message to or from the UI. Usage is counted on this computer when the injection finishes |

Variants, all without any UI:

| Option of the snippet | Typed | Result |
| --- | --- | --- |
| Immediately | `;;d` | "18 March 2026" on the last character; no space needed, none added |
| Ignore capitals, and follow mine | `;Ty` and a space | "Thank you for your order." with its first letter capitalised; `;TY` gives all capitals. Text from fill-in fields and the clipboard is never changed |
| A cursor position in the text | `;ref` and a space | "(your reference: \|)" with the caret inside; the space is not typed again |
| Only at the start of a word | `p;ty` and a space | Nothing: the abbreviation is in the middle of a word |
| Formatted snippet (task 3.8) | `;sigf` and a space | Pasted with formatting where the app accepts it, plain text elsewhere; the clipboard is restored afterwards |

**Enter and Tab do not expand** (ADR-0026). An abbreviation followed by Enter is sent, or
submitted, as typed. The user learns this in onboarding; the editor's Options repeat it; the
foot of "Why didn’t it expand?" repeats it; Settings → Typing can add Enter and Tab, behind a
warning (specification §4.7).

**Where Iamb stays out**, silently: password fields, apps the user excluded, windows it cannot
inspect (on Windows, those running as administrator), while an input method is composing, in
Iamb's own windows (except the onboarding test field), and while paused. In these cases the
engine stores no key at all.

### The menu

Opened from the tray or menu-bar icon. First line: the status in words.

| Item | Copy |
| --- | --- |
| Status | **Iamb is on** · "76 snippets" / **Iamb is paused** · "Typed abbreviations do not expand. Search still works." / **Iamb cannot expand yet** · "2 permissions are missing" |
| | Search snippets… (with the shortcut) · New snippet… · Open library |
| | Pause Iamb (ticked while paused) · Never expand in Mail (or "Expand in Mail again") |
| | Why didn’t it expand? · Settings… |
| | Quit Iamb (macOS) / Exit Iamb (Windows) |

Service: `GetStatus` when the menu opens, `StatusChanged` for the icon, `SetPaused(paused)`,
`PutExclusion(app_id, display_name, excluded)`. The app named in the menu is the one that was
in front when the menu opened, read by the UI host's own focus inspector.

The icon has three states: on, paused (dimmed), needs you (a dot). It does not change when the
focus moves into a password field or an excluded app.

### Why didn’t it expand?

A view of the library window.

| Part | Copy |
| --- | --- |
| Header | **Why didn’t it expand?** · "Kept in memory only. Iamb never records what you typed." |
| Test field | Label "Test an abbreviation". Results: "**Expands.** “Thanks for your order” would replace ;rty and the space." · "**Not yet.** “Thanks for your order” waits for a space or punctuation after **;rty**. Enter and Tab do not count." · "**No snippet.** Nothing has the abbreviation ;rtx." · "**Broken.** “Out of office” has an error: it includes ;sig2, which does not exist." [Open snippet] |
| Log | Newest first, at most 200. Each entry: time · a sentence · the app when known · a button when something can be done. The sentences are in specification §6.2 |
| Foot | "Typed an abbreviation and pressed Enter or Tab? Those keys do not expand a snippet." [Why?] |
| Empty | "Nothing has been held back since Iamb started." |

Service: `RecentDiagnostics` on opening, `DiagnosticAdded` while open, `DryRun(typed)` for the
test field (its `actions` and `hints` decide which of the four results is shown). The test
field's text is sent only in that request and is never stored.

## Errors and recovery

Nothing in this flow shows an error by itself. What can go wrong is found in the log:

| Situation | Log entry | Recovery |
| --- | --- | --- |
| The snippet's template has an error | "“Out of office” has an error · It includes the snippet ;sig2, which does not exist. Nothing was typed." [Open snippet] | Fix the snippet. The abbreviation stayed as typed |
| The user switched windows, or pressed a key that moves the cursor, while Iamb was typing | "“Workshop address” was stopped halfway · … What was already typed stayed where it was." | Delete the fragment and type the abbreviation again |
| The user typed during the injection | "Keys you typed while “Workshop address” was being typed may be out of order" | Check the text |
| Password field, excluded app, uninspectable window, composing | "Not expanding in …" with the reason; no snippet is named | None needed; "Manage apps" for exclusions |
| A snippet with blanks while the UI was restarting | "“Late delivery” could not show its form · Iamb's window process was restarting. Type the abbreviation again." | Type it again |
| The service is not running | No entry (nothing is listening). The icon shows "Needs you": "Iamb is not running" [Start Iamb again] | The supervisor restarts it; this state should last seconds |

## Timing

| Budget | Value |
| --- | --- |
| Last key to text on screen, plain text up to 500 characters | Under 50 ms for 95% of expansions (architecture §4.7) |
| Menu opens | Immediately: it is a native menu; `GetStatus` is asked when it opens and the status line updates in place |
| Log entry visible after the event | Under 100 ms while the view is open |

## Decisions in this flow

- **No feedback on success.** No sound, no highlight, no counter in the menu.
- **No feedback on failure either, for typed abbreviations.** A person types thousands of
  words that are not abbreviations; anything that reacts to typing would be noise, and would
  suggest Iamb is watching more than it is. The log answers when asked.
- **The icon does not follow the focus.**
- **No undo of Iamb's own.** The app's undo applies; a dedicated "undo last expansion" would
  need Iamb to remember what it replaced.
