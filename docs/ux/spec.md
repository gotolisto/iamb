# Iamb desktop UX specification

Status: proposed for review · Task 3.1 · Covers the desktop UI of Phase 3 and the sharing flow
of Phase 4 · Design system: [`design-system.md`](design-system.md) and
[`tokens.json`](tokens.json) · Flows: [`flows/`](flows/) · Mock-ups:
[`mockups/index.html`](mockups/index.html) · Review sheet: [`review.md`](review.md)

This document says what the Iamb desktop app shows, says and does. It is written against the
architecture ([`../architecture.md`](../architecture.md)), the decision records and the
template language ([`../template-language.md`](../template-language.md)), and it uses only the
messages of architecture §6, except where §11 below lists something the flows need that does
not exist yet. The six flows are specified step by step in `flows/`; this document holds what
they share.

Phase 3's exit gate is the yardstick: a new user installs Iamb, grants permissions, creates a
snippet and expands it, with no help.

## 1. Principles

Four rules decide every screen. When two pull in different directions, the earlier one wins.

1. **Private.** Iamb never shows, stores or sends what a person typed outside a snippet. The UI
   has no feature that would need it: no "recently typed", no suggestion of snippets from
   typing, no usage that leaves the computer. The diagnostics say why something did not expand
   without ever quoting a key. §9 has the rules the code must follow.
2. **Fast.** The two panels that interrupt typing appear instantly (fill-in form under 150 ms,
   search palette under 100 ms) and never animate. Nothing in the library waits on a spinner
   in normal use. §10 has the budgets.
3. **Keyboard-first.** Every task can be done without the pointer, the main action of every
   view is on Enter, and the shortcut is printed on the control. The pointer is always
   supported, never required.
4. **Quiet.** Iamb does its work without comment. It interrupts only when the user asked for
   something and did not get it, or when it cannot work at all. A typed abbreviation that does
   not expand produces no popup; the reason waits in one place for the user to look.

Three consequences that recur:

- **One primary action per view**, in the accent colour, triggered by Enter.
- **Say it in words.** Status is a sentence ("Iamb is paused"), an option is a sentence
  ("After a space or punctuation"), an error says what happened and what to do next.
- **Nothing half done.** A snippet is saved or it is a draft; a form inserts or changes
  nothing; a template with an error types nothing.

## 2. Words

The interface uses a small, fixed vocabulary. The words on the left never appear in the UI.

| The UI says | Not | Meaning |
| --- | --- | --- |
| snippet | template, macro, shortcut | A saved text with its abbreviation |
| abbreviation | trigger, keyword, shortcut | What the user types |
| expand | replace, fire, trigger | Iamb typing the snippet |
| after a space or punctuation / immediately | delimiter mode / immediate mode | The two ways a snippet expands |
| fill-in field; "asks for values" | form field, variable, placeholder | `{field}` |
| optional part | conditional, if-block | `{if}` … `{endif}` |
| insert (in the editor: Date, Time, Clipboard, Cursor position, Another snippet, Key press, Wait) | macro | The other macros |
| folder, tag | group, category, label | Organisation |
| group (a family or a team) | profile, organisation, workspace | A group profile |
| pause | disable, suspend | Global pause |
| "never expand in …" | exclusion, blocklist | An excluded app |
| needs review | pending approval, quarantined | A received snippet held under ADR-0022 |
| this computer | device, locally | Where data stays |
| shortcut (Settings: "Shortcuts") | hotkey, chord | A global key combination |

Voice: short sentences, present tense, no exclamation marks, no "please", no "oops", no "we".
Iamb is "Iamb", not "I". Buttons are verbs. Numbers are digits. Sentence case everywhere. Time
is said in the user's words ("30 minutes"), never in seconds or milliseconds. English is the
first language; every string lives in a message catalogue from the start (task 3.11), with
plural forms, so the other languages of the date tables (Spanish, German, French) can follow.

## 3. Information architecture

```text
Always present                       On demand                         Over other apps
─────────────────                    ─────────────────────────────     ───────────────────────
Tray / menu-bar icon ──menu──┬─────► Library window                    Fill-in form panel
                             │         ├─ Sidebar: All snippets,       Search palette
Global shortcut ─────────────┼──┐      │   Needs review, folders,      Notice (one line, when an
                             │  │      │   tags                        explicit request failed)
Typing an abbreviation ──┐   │  │      ├─ Snippet list
                         │   │  │      ├─ Editor (plain, chips, formatted)
                         │   │  │      ├─ Library health
                         │   │  │      ├─ Why didn’t it expand?
                         │   │  │      └─ Settings
                         │   │  │    Onboarding window (first start, or from the menu)
                         │   │  └──► Search palette
                         └───┼─────► Fill-in form panel (snippets with blanks)
                             └─────► New snippet (library window, new draft)
```

- **One main window.** The library window holds the library, the editor, library health, the
  diagnostics and the settings as views of its right-hand area; the sidebar switches between
  them. It is created on demand and destroyed when closed (architecture §10); closing it never
  stops Iamb.
- **Two panels** exist the whole time, hidden: the fill-in form and the search palette.
- **One onboarding window**, shown at first start and reachable later from the menu.
- **No other windows.** No floating toolbar, no notification-centre entries, no badge on the
  Dock or taskbar. On macOS Iamb has no Dock icon while only the panels are in use; the Dock
  icon appears with the library or onboarding window and goes with it.

How each surface is reached:

| Surface | From |
| --- | --- |
| Menu | Click the tray or menu-bar icon |
| Library window | Menu → Open library; the Dock or Start-menu icon; "Open my library" at the end of onboarding; ⌘E / Ctrl+E in the palette |
| New snippet | ⌘N / Ctrl+N in the library; menu → New snippet…; the palette when nothing matches |
| Search palette | The global shortcut (default Ctrl+Shift+Space on both systems); menu → Search snippets… |
| Fill-in form | Typing the abbreviation of a snippet with blanks; picking one in the palette; "Fill in … again" in the palette |
| Why didn’t it expand? | Menu; sidebar; the link in any notice or onboarding error |
| Settings | Menu; sidebar; ⌘, / Ctrl+, |
| Library health | Sidebar (with a count when something needs fixing); the "Broken" badge of a snippet |
| Onboarding | First start; menu → Set up Iamb (only while setup is unfinished); "Fix permissions" |

## 4. Windows and panels

Each surface is described by what it is for, how it is laid out, its states and its copy.
Screens that belong to a flow are drawn in the mock-ups and specified in `flows/`; the text
here is the reference for everything the flows do not walk through.

### 4.1 Tray and menu-bar menu

The only thing that is always visible. A native menu (not a web view), so it opens instantly
and behaves like every other menu of the system.

Icon states (three, never more):

| State | Icon | First line of the menu |
| --- | --- | --- |
| On | The mark | **Iamb is on** · "76 snippets" |
| Paused | The mark at reduced strength | **Iamb is paused** · "Typed abbreviations do not expand. Search still works." |
| Needs you | The mark with a dot | One of: **Iamb cannot expand yet** · "2 permissions are missing" / **Iamb is not running** · "Its background service stopped." / **An update is ready** · "Restart Iamb to finish." |

The icon does not change with the focused app (password field, excluded app): it would flicker
all day and say nothing the user can act on.

Menu items, in order:

| Item | Shortcut shown | Does |
| --- | --- | --- |
| *(status line, not selectable)* | | |
| Fix permissions… / Start Iamb again / Restart to update | | Only in "Needs you"; the item for the cause |
| Search snippets… | the global shortcut | Opens the palette for the app that was in front |
| New snippet… | | Library window with a new draft |
| Open library | | |
| Pause Iamb (checkable) | | `SetPaused`. No timed pause in version 1 |
| Never expand in *App* / Expand in *App* again | | Adds or removes the exclusion of the app that was in front when the menu opened. Hidden when that app is Iamb or cannot be named |
| Why didn’t it expand? | | |
| Settings… | ⌘, / Ctrl+, | |
| Quit Iamb (macOS) / Exit Iamb (Windows) | | Asks first: "Quit Iamb? Abbreviations stop expanding until you open it again." [Quit] [Cancel]. Quitting stops the service too |

If the tray icon is hidden in Settings, everything in the menu is still reachable from the
library window; Iamb keeps running (ADR-0017).

### 4.2 Library window

For finding, reading and organising snippets. Three panes: sidebar (220 px), snippet list
(320 px), editor (the rest). Minimum window size 880 × 560; below 1040 px wide the sidebar
collapses to icons. Pane widths, the selected folder and the selected snippet are remembered.

**Sidebar**, top to bottom:

- All snippets (count).
- Needs review (count), only when something waits (§4.10).
- Folders: a tree of at most four levels, personal folders first, then folders shared by
  others, each with its count. A shared folder carries the people icon. Drag to reorder or
  nest; drop snippets on a folder to move them.
- Tags: each with its colour dot and count. Selecting a tag filters the list across folders.
- At the bottom: Library health (with a count badge when there is something to fix), Why
  didn’t it expand?, Settings, and the status line ("Iamb is on" / "Iamb is paused" /
  "Iamb cannot expand yet"), which is a button that opens the menu's actions.

**Snippet list**: a filter field ("Filter Replies"), the New button, then rows (design system
§4.4), sorted by label; the sort can be changed to "Recently changed" or "Most used on this
computer". The list draws only the visible rows and must scroll at 60 frames per second with
10,000 snippets; filtering runs in the window over the list it already holds.

The header of a folder states who can see it and holds its actions: "Only you" · "Shared with
Brindlewood team" · "Shared by Ines · you can use" · "Shared by Ines · you can change".
Folder settings (rename, the abbreviation prefix, the defaults for new snippets, share) open
from the header's More button.

States of the library window:

| State | What is shown |
| --- | --- |
| Loading | The frame of the three panes at once; the lists fill when the service answers (normally under 100 ms). No spinner before 300 ms |
| Empty library | "No snippets yet" · "A snippet is text you save once and type with a short abbreviation from then on." [New snippet] [Import…] |
| Empty folder | "This folder is empty" [New snippet] |
| Filter without results | "Nothing in Replies matches “xyz”" [Clear filter] · and when other folders have matches: "3 matches in other folders" [Show them] |
| Nothing selected | The editor pane shows "Select a snippet, or press ⌘N to make one." |
| Several selected | The editor pane shows "5 snippets selected" with Move to…, Add tag…, Turn off, Delete |
| Service not running | A danger banner across the window: "**Iamb is not running.** Its background service stopped, so nothing can be saved or expanded." [Start it again]. The lists stay visible and read-only |
| Permissions missing (macOS) | A warning banner: "**Iamb cannot expand yet.** macOS has not given it Input Monitoring and Accessibility." [Fix permissions] |
| Paused | The status line says so; no banner |
| Read-only folder | Fields are read-only, Save is replaced by "Copy to my snippets" |

Destructive actions: deleting snippets shows a toast with Undo for 10 seconds instead of a
confirmation ("Deleted “Opening hours”." · "Deleted 5 snippets."). Deleting a folder asks:
"Delete “Returns” and its 11 snippets? Other snippets that include one of them will stop
working." [Delete] [Cancel].

Import and export (menu of the list's More button): Import… takes a JSON library or a CSV
file, with one choice, "When an abbreviation is already in use: skip the new one / stop and
import nothing", and reports "Imported 212 snippets into 6 folders. 3 were skipped because
their abbreviations are already in use." [Show skipped]. Export… writes the personal library
as JSON or CSV and says "Exported 76 snippets to iamb-library.json."

### 4.3 Editor

For writing one snippet. Flow 2 walks through it; this is the reference.

Fields, in tab order: Abbreviation · Label (optional) · Content · Folder · Tags · Options.
Header: the snippet's label, the On switch, More actions (Duplicate, Move to…, Copy content,
Delete), Cancel (drafts) and Save.

- **Kinds.** "Plain text" or "Formatted" (task 3.8). A formatted snippet is written as
  Markdown with a small toolbar (bold, italic, link, list, image) and the preview shows the
  formatted result. Switching from formatted to plain asks first when formatting would be lost.
  "Cursor position" is not offered for formatted snippets (the template language ignores it
  there).
- **Saving is explicit.** ⌘S / Ctrl+S or the Save button. Until then nothing reaches the
  library, so a half-written abbreviation can never expand. Unsaved edits show "Not saved" in
  the header and are guarded when the user leaves (flow 2). An edit to a saved snippet that is
  abandoned restores the saved text.
- **Chips.** Macros appear as chips (design system §4.6), inserted from the `{` list or the
  toolbar and edited in their card. The stored text is always the template syntax; "Show as
  text" in More actions shows and edits the raw source for people who prefer it, with the
  same error messages.
- **Preview.** Rendered by the service with the same parser as expansion. Blanks without a
  value show as grey name tags; "Try the form" opens the real fill-in form and feeds the
  preview. Key presses are shown as key badges, waits as "· 0.5 s ·", the cursor position as
  a caret. The preview says "Preview unavailable" with the reason when the template has an
  error.
- **Options**, closed by default, summarised in the footer in words:
  - Expand: "After a space or punctuation" (help: "When the word is finished. Enter and Tab do
    not count.") or "Immediately" (help: "On the last character. Good for abbreviations that
    are never part of a word.").
  - Capital letters: "Exactly as written" · "Ignore" · "Ignore, and follow mine" (help:
    "`;sig` types it as written, `;Sig` starts with a capital, `;SIG` is all capitals. Text
    you fill in is never changed.").
  - "Only at the start of a word" (on by default for the first trigger, off for
    "Immediately").
- **Footer facts**: the options in words, and "Used 31 times on this computer · last used
  yesterday" (when the service can say so, §11).

Messages under the Abbreviation field:

| Situation | Message | Blocks saving |
| --- | --- | --- |
| Empty | "Type an abbreviation." | yes |
| Contains a space or a line break | "An abbreviation cannot contain spaces." | yes |
| Longer than 64 characters | "An abbreviation can be at most 64 characters." | yes |
| In use by a snippet of mine | "**;rty** is already the abbreviation of “Thanks for your order” in Replies. Each abbreviation can belong to one snippet." [Show that snippet] | yes |
| In use by a shared snippet | "**;rty** is also used by “Thanks” in the shared folder Shop replies. On this computer yours will expand and the shared one will not." | no |
| Immediate, and a longer one starts with it | "This snippet expands immediately, so **;rl** will fire before you can finish typing **;rlate**. Use a different abbreviation, or let it wait for a space." | no |
| Another immediate one is its beginning | "**;rl** expands immediately and will fire before **;rlate** can be typed. Change one of the two." | no |
| One or two letters or a plain word, expanding immediately | "Short abbreviations made only of letters can fire in the middle of ordinary typing. Starting with a character such as ; avoids that." | no |

Messages under the Content field come from the template error codes (§6.1). A snippet with a
template error can be saved; it is marked Broken, does not expand, and says so: "Saved, but it
will not expand until the error is fixed."

### 4.4 Fill-in form panel

For giving a snippet its blanks. Flow 4 walks through it.

- Appears in the upper third of the display the user is working on, horizontally centred,
  440 px wide, at once and without animation. Height follows the fields up to 70% of the
  screen, then the body scrolls.
- Header: the mark, the snippet's label, its abbreviation. Body: the fields in the order they
  first appear in the snippet (design system §4.13). Footer: on the left "Goes into *App*" or,
  when the form opened with remembered answers, "Your last answers. Start empty"; on the right
  Cancel and Insert.
- Focus starts in the first field with its content selected. No field is required. Enter
  inserts (⌘⏎ / Ctrl+Enter from a several-line field); Esc cancels. Clicking outside does not
  close the panel; switching to another app leaves it open and it comes back to the front when
  its app does.
- On Insert the panel hides, focus returns to the app, and only then is the text typed
  (ADR-0013). On Cancel the panel hides, focus returns, nothing else changes.
- States: open; inserting (hidden, waiting for the focus to be confirmed; if that takes longer
  than a second the panel returns with the "could not get back" message of flow 4); refused
  (a notice, flow 4). A second abbreviation typed while a form is open is ignored; the open
  panel does not react.
- On macOS the panel is a non-activating panel: the app being typed into stays the active app
  and keeps its menu bar. On Windows the panel takes the foreground and the app's title bar
  turns inactive until the panel closes.

### 4.5 Search palette

For finding a snippet without its abbreviation. Flow 5 walks through it.

- Same place and behaviour as the form panel; 620 px wide. A query row, up to eight result
  rows, a three-line preview of the selected snippet, a footer of key hints.
- With an empty query: "Fill in “…” again" when a form was submitted in the last 30 minutes,
  then the snippets used most recently on this computer (up to eight rows in all); when none was
  used or the service cannot report usage (§11), the hint "Type to search by abbreviation, label, text or tag."
- Rows show label, abbreviation, folder and first line, with matches marked. Badges: "Asks
  for values" icon, Off, Needs review, Broken. Enter does what the footer says for the
  selected row: insert, fill in and insert, review, or open.
- The query is never stored; the palette opens empty every time.
- Works while Iamb is paused. Opened while Iamb's own library window has the focus, it selects
  the snippet in the library instead of typing.
- States: open; no results ("No snippet matches “warranty”." with the row "New snippet
  labelled “warranty”"); service not answering ("Iamb is not running." [Start it again]);
  refused pick (a notice, flow 5). While a fill-in form is open the shortcut does nothing.

### 4.6 Onboarding window

For getting from install to a first expansion in under two minutes. Flow 1 walks through it.
A fixed-size window (560 × 600), one step at a time, a progress line, Back on every step but
the first, Enter for the main action. Steps: Welcome · Permissions (macOS only) · First
snippet · Try it · Done. Closing the window at any step leaves Iamb running; the menu then
offers "Set up Iamb" until the last step has been reached once.

### 4.7 Settings

A view of the library window with its own list of sections on the left. Every setting takes
effect at once and says so by its own state; there is no Save button and no restart.

| Section | Setting | Copy and behaviour |
| --- | --- | --- |
| General | Start Iamb when I log in | Switch. On by default (set at the end of onboarding) |
| | Show the Iamb icon in the menu bar / notification area | Switch. Help: "Iamb keeps running without it. Open it from the Dock / Start menu." |
| | Play a soft sound after a snippet expands | Switch, on by default (`ui.expansion_sound`; stored as `off` when off). Help: "A short, quiet chime each time an abbreviation is replaced. Turn it off for silence." |
| | Appearance | Follow the system · Light · Dark |
| | Updates | "Iamb 0.3.0 · up to date" [Check now] · switch "Install updates automatically". Help: "Updates are installed while you are not typing." |
| Typing | A snippet expands after | Checkboxes: "A space" (always on, shown disabled) · "Punctuation . , ; : ! ?" · "Closing brackets and quotes ) ] } \" '" · "Enter" · "Tab". Enter and Tab are off by default and carry a warning (below) |
| | New snippets expand | After a space or punctuation · Immediately |
| | Capital letters in new snippets | Exactly as written · Ignore · Ignore, and follow mine |
| Dates | Date format, Time format | A list of formats shown as today's date, plus "Custom…" with a format field and a live example. Error: "Iamb does not know “%Q”. See the list of date codes." |
| | Month and day names in | English · Español · Deutsch · Français |
| Shortcuts | Search snippets | Recorder (design system §4.2). Default Ctrl+Shift+Space |
| | Fill in the last form again · Pause or resume | Recorders, empty by default (§11) |
| Apps | Never expand in these apps | A list with app icon and name, [Add app…] (choose from the apps that are open, or browse) and Remove. Help: "Iamb does not store keys typed in these apps. Add password managers, remote desktops and terminals where you type passwords." |
| | If snippets come out wrong in an app | Per app: "Type it key by key" · "Paste it" · "Let Iamb decide"; "Line breaks as: Enter · Shift+Enter"; "Slow down by … ms". Help links to the compatibility notes. Stored as `injection_overrides` (ADR-0025) |
| Privacy | *(statements, not settings)* | "Keys you type are held in memory for a moment and never saved or sent." · "How often you use a snippet is counted on this computer only." · "The log of why something did not expand lives in memory and is gone when Iamb restarts." |
| | Send crash reports | Switch, off by default. Help: "A report says where Iamb failed. It never contains what you typed, your snippets or your clipboard." [See the last report] |
| Account | *(Phase 4)* | Signed-in address, groups, sync state, [Sign out] |
| About | | Version, licence (AGPL-3.0-or-later), source code link, [Open the log folder] |

Turning on Enter or Tab asks first (ADR-0026, item 2):

> **Expand on Enter?**
> Apps act on Enter before Iamb can type: a chat sends the message, a dialog closes, a form
> moves on. With this on, a snippet can be typed into whatever has the focus afterwards, and
> Iamb deletes characters there first. Turn it on only if you use Iamb in plain documents.
> [Turn on anyway] [Cancel]

The Tab text is the same with "Tab moves to the next field of a form". While either is on, the
setting shows a permanent warning line: "Enter also expands. Snippets may land in the wrong
place in chats, forms and dialogs."

### 4.8 Why didn’t it expand?

For answering that one question without reading a manual. Drawn in flow 3.

- Top: "Test an abbreviation", a field that says what would happen if its content were typed
  in an app, without typing anywhere: "**Expands.** “Thanks for your order” would replace
  ;rty and the space." · "**Not yet.** “Thanks for your order” waits for a space or
  punctuation after ;rty. Enter and Tab do not count." · "**No snippet.** Nothing has the
  abbreviation ;rtx." · "**Broken.** “Out of office” has an error: …" [Open snippet]. What is
  typed here is never stored.
- Below: the log, newest first, at most 200 entries, each with the time, a sentence, the app
  when known, and a button when something can be done. The header states: "Kept in memory
  only. Iamb never records what you typed."
- Foot: "Typed an abbreviation and pressed Enter or Tab? Those keys do not expand a snippet."
  [Why?] (opens the Typing settings).
- Empty state: "Nothing has been held back since Iamb started."

The sentences come from the reason codes (§6.2). An entry names a snippet only when a snippet
matched or was asked for; while Iamb is held back it names only the app and the reason
(architecture §8 rule 4).

### 4.9 Library health

For keeping a growing library working. A view with one section per kind of problem, each
listing the snippets with a one-line explanation and the fix. Sections with nothing in them are
not shown; when all are empty: "Nothing to fix. All 76 snippets can expand."

| Section | Entry reads | Fix offered |
| --- | --- | --- |
| Will not expand: error in the snippet | "“Out of office” · It includes ;sig2, which does not exist." | Open snippet |
| Will not expand: abbreviation taken | "“Workshop address” in Shop replies · Your own ;addr is used instead." | Open mine · Open shared |
| Never reached | "“Late delivery” (;rlate) · ;rl expands immediately and fires first." | Open ;rl · Open ;rlate |
| Not used on this computer for 90 days | "“Holiday closing” · last used 14 December" | Turn off · Delete. Header: "Counted on this computer only." |

`Health` answers every row (task 3.10): template errors and broken references (a `{snippet}` that
names nothing, a cycle, nesting over 8 levels) under the first, taken abbreviations, never reached,
and snippets not used for 90 days on this computer. A reference to a snippet that is switched off
is not broken. A snippet is listed once, in the first row that applies.

### 4.10 Review of received snippets

For deciding whether a snippet written by someone else may press keys, wait, read the
clipboard or type several lines on this computer (ADR-0022). Drawn in flow 6.

- "Needs review" in the sidebar lists every held snippet; rows carry the badge everywhere they
  appear, including the palette.
- The review dialog names the author and the time, lists in words what the snippet does
  besides typing text, shows what it types with those parts as chips, and, for a changed
  snippet, shows the difference from the text approved before.
- Buttons: "Not now" (focused first) and "Approve". No "Reject": leaving a snippet unapproved
  already keeps it from expanding, or keeps the previously approved text.
- Approval is per computer and is asked again after every change by someone else. Snippets
  the user wrote are never held. Single-line plain text from others is never held.

### 4.11 Notices and toasts

Design system §4.9. The complete list of notices over other apps, because each one is an
interruption and must earn it:

| When | Title | Sentence |
| --- | --- | --- |
| A palette pick or a form insert was refused | "“*Label*” was not inserted" | The reason from §6.2, and for a form: "Your answers are kept for 30 minutes: press *shortcut* where the text should go." |
| A form could not be shown because the UI was restarting | — | No notice (there is no UI to show it); the log has the entry |
| An update was installed | "Iamb was updated to 0.3.1" | Shown once, in the menu's status line, not as a notice |

Nothing else may use a notice. In particular: no notice for a typed abbreviation that did not
expand, for a finished sync, for a shared folder arriving while the library is closed (the
menu's status line says "1 folder was shared with you" until the library is opened).

## 5. Keyboard

### 5.1 Global

| Action | macOS | Windows |
| --- | --- | --- |
| Search snippets (palette) | ⌃⇧Space (changeable) | Ctrl+Shift+Space (changeable) |
| Fill in the last form again | unset; from the palette | unset; from the palette |
| Pause or resume | unset; from the menu | unset; from the menu |

### 5.2 Library window

| Action | macOS | Windows |
| --- | --- | --- |
| New snippet | ⌘N | Ctrl+N |
| New folder | ⌘⇧N | Ctrl+Shift+N |
| Save | ⌘S | Ctrl+S |
| Filter the list | ⌘F | Ctrl+F |
| Find in all snippets (opens the palette inside the window) | ⌘K | Ctrl+K |
| Focus sidebar / list / editor | ⌘1 / ⌘2 / ⌘3 | Ctrl+1 / Ctrl+2 / Ctrl+3 |
| Next / previous pane | F6 / ⇧F6 | F6 / Shift+F6 |
| Move in a list or tree | ↑ ↓, Home, End, type to jump | same |
| Open the selected snippet in the editor | ⏎ | Enter |
| Back from the editor to the list | esc | Esc |
| Rename folder | ⏎ | F2 |
| Delete (with Undo) | ⌘⌫ | Delete |
| Duplicate | ⌘D | Ctrl+D |
| Turn the selected snippet on or off | Space | Space |
| Share the selected folder | ⌘⇧S | Ctrl+Shift+S |
| Context menu | ⌃⏎ | Shift+F10 or the menu key |
| Settings | ⌘, | Ctrl+, |
| Zoom in / out / reset | ⌘+ / ⌘− / ⌘0 | Ctrl++ / Ctrl+− / Ctrl+0 |
| Close the window (Iamb keeps running) | ⌘W | Ctrl+W |

### 5.3 Editor

| Action | macOS | Windows |
| --- | --- | --- |
| Insert something | { , then type to filter | same |
| Open a chip's card | ⏎ or Space on the chip | Enter or Space |
| Move over a chip, select it, delete it | ← →, ⇧← ⇧→, ⌫ | ← →, Shift+← →, Backspace |
| Options | ⌘⇧O | Ctrl+Shift+O |
| Preview / back | ⌘⇧P | Ctrl+Shift+P |

Tab never inserts a tab character in the editor, so keyboard users are never trapped in it
(WCAG 2.1.2). A tab inside a snippet is the key press "Tab" from the insert list, or pasted text.

### 5.4 Fill-in form

| Action | macOS | Windows |
| --- | --- | --- |
| Next / previous field | ⇥ / ⇧⇥ | Tab / Shift+Tab |
| Insert | ⏎ (⌘⏎ from a several-line field, and everywhere) | Enter (Ctrl+Enter) |
| Cancel | esc | Esc |
| Choose an option | ↑ ↓, or type its first letters | same |
| Flip a toggle | Space | Space |
| Start empty | ⌘⌫ | Ctrl+Backspace |

### 5.5 Search palette

| Action | macOS | Windows |
| --- | --- | --- |
| Move | ↑ ↓ (wraps), Page Up, Page Down | same |
| Insert / fill in / review / open (as the footer says) | ⏎ | Enter |
| Insert row 1 to 9 | ⌘1 … ⌘9 | Ctrl+1 … Ctrl+9 |
| Open the selected snippet in the library | ⌘E | Ctrl+E |
| Close | esc, or the global shortcut again | Esc, or the global shortcut again |

### 5.6 Rules

- Shortcuts inside Iamb's windows are fixed in version 1; only the global ones can be changed.
- Every shortcut is shown on its control or in its menu item, and exposed to assistive
  technology with `aria-keyshortcuts`.
- No shortcut is a single printable character (WCAG 2.1.4), except typing to jump inside a
  list or tree that has the focus.
- Focus order follows reading order. Focus is never lost: when the focused thing disappears
  (a deleted row, a closed dialog), focus moves to the next row or back to where it came from.

## 6. Messages

### 6.1 Template errors

Shown under the Content field, at the chip or position the service reports, and in library
health. The code is shown small after the sentence for people who look things up.

| Code | Sentence |
| --- | --- |
| `E_UNCLOSED_MACRO` | "Something that starts with { is never closed. Add the closing } or delete it." |
| `E_BAD_SYNTAX` | "This part is not written correctly. Open it and set its values again." |
| `E_UNKNOWN_ATTR` | "This part has a setting it does not understand. Open it and set its values again." |
| `E_DUPLICATE_ATTR` | "This part has the same setting twice. Open it and set its values again." |
| `E_MISSING_ATTR` | "This part is missing something: a fill-in field needs a name, a key press needs a key." |
| `E_BAD_VALUE` | "This part has a value that is not allowed." (the card names the field: "Press it 1 to 50 times.") |
| `E_BAD_FORMAT` | "Iamb does not know this date or time code. See the list of codes." |
| `E_BAD_SHIFT` | "The shift is not written correctly. Use a sign, a number and a unit, for example +3 days." |
| `E_UNBALANCED_IF` | "This optional part has a start but no end." / "This “End if” has no start." / "This “Else” is outside an optional part." |
| `E_TOO_DEEP` | "Optional parts, or snippets inside snippets, are nested more than 8 deep." |
| `E_SNIPPET_NOT_FOUND` | "It includes the snippet **;sig2**, which does not exist." [Choose another] |
| `E_SNIPPET_CYCLE` | "It includes itself, through **;sig**. A snippet cannot contain itself." |
| `E_FIELD_CONFLICT` | "Two fill-in fields are both called “Bike” but are of different kinds. Give one another name, or make them the same." |
| `E_FIELD_UNDEFINED` | "This optional part looks at “Bike”, but there is no fill-in field with that name." |
| `E_FORM_TOO_LARGE` | "This snippet asks for more than 100 values." |
| `E_DATE_RANGE` | "The shifted date falls outside the years 1 to 9999." |
| `E_CURSOR_BEFORE_KEY` | "A key press comes after the cursor position. Move the cursor position to the end, or remove the key press: Iamb cannot know where the key leaves the cursor." |
| `E_WAIT_TOTAL` | "The waits add up to more than 30 seconds." |
| `E_OUTPUT_TOO_LARGE` | "The text is longer than 100,000 characters." |

### 6.2 Reasons in “Why didn’t it expand?” and in notices

| Code | Log entry | In a notice |
| --- | --- | --- |
| `E_…` | "“*Label*” has an error" · the sentence of §6.1 · "Nothing was typed." [Open snippet] | "This snippet has an error." [Open it] |
| `suspended_secure_field` | "Not expanding in a password field" · "Iamb stores no keys there." | — |
| `suspended_excluded_app` | "Not expanding in *App*" · "You excluded this app." [Manage apps] | — |
| `suspended_unknown_focus` | "Not expanding in *App*" · "Iamb cannot see into this window, so it stays out." (Windows adds "It may be running as administrator.") | — |
| `suspended_composing` | "Not expanding while your keyboard composes text" · "An input method is on." | — |
| `refused_secure_field` | "“*Label*” was not inserted: the cursor was in a password field" | "The cursor is in a password field. Iamb does not type there." |
| `refused_excluded_app` | "“*Label*” was not inserted: you excluded *App*" | "You excluded this app." |
| `refused_unknown_focus` | "“*Label*” was not inserted: Iamb cannot see into this window" | "Iamb cannot see into this window, so it stays out." |
| `refused_own_window` | "“*Label*” was not inserted into Iamb's own window" | — (the palette opens the snippet instead) |
| `refused_composing` | "“*Label*” was not inserted: your keyboard was composing text" | "Your keyboard is composing text. Finish the word and try again." |
| `request_cancelled` | "“*Label*” was not inserted: you moved to another app first" | "You moved to another app before the text could be typed." |
| `injection_busy` | "“*Label*” was not inserted: Iamb was still typing the previous snippet" | "Iamb was still typing the previous snippet. Try again." |
| `injection_cancelled` | "“*Label*” was stopped halfway" · "You switched windows or pressed a key that moves the cursor while Iamb was typing. What was already typed stayed where it was." | — |
| `typed_during_injection` | "Keys you typed while “*Label*” was being typed may be out of order" | — |
| `form_open` | "“*Label*” was not started: another fill-in form was still open" | — |
| `form_no_ui_host`, `form_not_acknowledged` | "“*Label*” could not show its form" · "Iamb's window process was restarting. Type the abbreviation again." | — |
| `no_last_form` | "There is no earlier form to fill in again" · "Answers are kept for 30 minutes." | Same sentence |
| `unknown_snippet` | "The snippet no longer exists" | "That snippet was deleted." |
| `not_a_delimiter` | "“*Label*” did not expand: Enter and Tab do not count as the end of a word" [Why?] (opens the Typing settings) | — |
| `keys_for_unreported_window`, `unexpected_injection_event`, `unknown_form_session`, `panel_focus_expired`, any unknown code | Not listed by default. "Show technical entries" reveals them as "Technical: *code*" for bug reports | — |

### 6.3 Errors from the service

| Code | Where | Sentence |
| --- | --- | --- |
| `conflict` | Save | The abbreviation message of §4.3 |
| `invalid` | Save, settings | The field's own message; otherwise "Iamb could not accept that value." with the service's sentence |
| `not_found` | Save, delete, open | "This snippet no longer exists. It may have been deleted on another device." [Close] |
| `unavailable` | Anything | The "Iamb is not running" banner, or for one action "Iamb is busy. Try again in a moment." |
| `not_permitted`, `unknown_request`, `not_implemented`, `unsupported_protocol`, `internal` | Anything | "Something went wrong inside Iamb. Restarting it usually helps." [Restart Iamb] [Copy details]. Details are the code and the request's name, never its content |

A different protocol version between the UI and the service (after a half-finished update)
shows: "Iamb was updated and needs to restart." [Restart Iamb].

## 7. Platform differences

| | macOS | Windows |
| --- | --- | --- |
| Always-visible icon | Menu-bar extra, a template image that follows the menu bar's colour | Notification-area icon; Windows may put it in the overflow, so the last onboarding step says where to find it |
| Menu | Standard menu-bar menu | Popup menu on left and right click |
| Leaving | "Quit Iamb" | "Exit Iamb" |
| Permissions | Input Monitoring and Accessibility, granted to Iamb's helper (ADR-0021); onboarding has a step for them, and they can be lost again | None; onboarding has no such step. Windows running as administrator cannot be typed into; the log says so |
| Name in System Settings | The helper must be listed as "Iamb" with Iamb's icon, or the user will not find it (§11) | — |
| Form and palette | Non-activating panels: the app stays active | Take the foreground with the service's grant; the app's title bar turns inactive |
| Notice position | Top right | Bottom right, above the taskbar |
| Window chrome | System title bar with traffic lights; unified with the toolbar | System title bar |
| Button order | Main action rightmost; Cancel to its left | Main action leftmost; Cancel to its right |
| Modifier | ⌘, written as symbols | Ctrl, written as words joined by + |
| Settings shortcut and place | ⌘, · also in the app menu as "Settings…" | Ctrl+, · in the sidebar and the tray menu |
| Dock / taskbar | No Dock icon unless a window is open | Taskbar button only while a window is open |
| Start at login | Login item of the helper app | Per-user Run key |
| App names in exclusions | Bundle name and icon | Product name and icon of the executable |
| High contrast | "Increase contrast" | Contrast themes (`forced-colors`) |
| Scroll bars, text selection, context menus, spell check | The system's | The system's |

## 8. Accessibility

Target: WCAG 2.1 level AA, on both systems, with VoiceOver on macOS and Narrator and NVDA on
Windows. Task 3.11 tests it; this section is what it tests against.

| Success criterion | How Iamb meets it |
| --- | --- |
| 1.1.1 Non-text content | Icons beside a label are hidden; icon-only buttons and status icons have names; the mark has the name "Iamb" |
| 1.3.1 Info and relationships | Real roles: `tree`, `listbox`, `dialog`, `radiogroup`, `switch`, `status`, `alert`; every field has a programmatic label and its message is tied to it |
| 1.3.2, 2.4.3 Order | Reading order and focus order are the visual order |
| 1.4.1 Use of colour | Status, chips, matches, changes and tags all carry an icon, words or weight besides colour (design system §1) |
| 1.4.3, 1.4.11 Contrast | The 88 checks of design system §2.2 |
| 1.4.4, 1.4.10 Resize and reflow | 200% zoom in a 1280 × 800 window with no pane scrolling sideways; the panels scroll vertically only |
| 1.4.12 Text spacing | Rows and controls grow with their text; nothing has a fixed height that clips |
| 1.4.13 Content on hover or focus | Tooltips are dismissible with Esc, stay while hovered, and never hold information that is not also elsewhere |
| 2.1.1, 2.1.2 Keyboard | §5; no trap, including the editor (Tab always moves on) |
| 2.1.4 Character shortcuts | None |
| 2.2.1 Timing | Toasts and notices carry nothing that is not also in the log or the list; they pause while focused or hovered. Remembered answers expire after 30 minutes for privacy; the form says "Your last answers" so their presence is never a surprise |
| 2.3.1 Flashes | Nothing flashes |
| 2.4.2, 2.4.6 Titles and labels | Each window has a title that names its view: "Iamb – Replies", "Iamb – Settings" |
| 2.4.7 Focus visible | A 2 px ring on everything focusable (design system §4) |
| 2.5.3 Label in name | The accessible name starts with the visible label |
| 3.2.1, 3.2.2 Predictable | Focus alone never changes anything; settings change only what they say |
| 3.3.1, 3.3.3 Errors | Every error names the field, says what is wrong and what to do; errors are announced when they appear |
| 4.1.2, 4.1.3 Name, role, value; status messages | Live regions for "Saved", "Granted", result counts in the palette ("4 results"), and the test field's result |

Names and roles of the main surfaces:

| Surface | Role and name | Announced on opening |
| --- | --- | --- |
| Library window | `application` regions: "Library" (navigation), "Snippets in Replies" (listbox), "Snippet" (form) | The window title |
| Fill-in form | `dialog`, named by the snippet's label | "Late delivery, fill-in form, 4 fields. First name, edit text" |
| Search palette | `dialog` "Search snippets" containing a `combobox` that controls a `listbox` | "Search snippets, edit text"; then on each change "4 results, Return label, abbreviation semicolon r r e t" |
| Onboarding | `dialog`-like window, each step a heading | "Step 2 of 5. Let Iamb see and type keys" |
| Review dialog | `dialog`, named "Review Paste tracking link" | Author, then the list of what the snippet does |
| Notice | `alert` | Title and sentence |
| Tray / menu-bar icon | The system's status item, named "Iamb, on" / "Iamb, paused" / "Iamb, needs attention" | — |

Other requirements:

- **Screen readers and the panels.** The form and the palette must take the screen reader's
  focus when they appear and return it to the app when they close, on both systems, including
  as non-activating panels on macOS. This is a risk to prove early in tasks 3.5 and 3.6.
- **Reduced motion and high contrast**: design system §2.6 and §5.
- **Pointer targets**: at least 24 × 24 px.
- **No reliance on sound.** The only sound is the soft chime after an expansion (Settings → General, on by default, `ui.expansion_sound`); nothing depends on hearing it, and the visible result is the text itself.
- **Language.** The `lang` of the interface is set; snippet content is user text and is not
  marked with a language.

## 9. Privacy in the UI

The architecture's rules (§8) bind the UI host as much as the service. For the UI they mean:

1. **Never logged, never stored, never sent:** the content of the onboarding test field, the
   "Test an abbreviation" field, the palette's query, fill-in values, preview test values and
   clipboard content. They live in the component's state, travel only inside the one IPC frame
   that needs them, and are dropped when the surface closes. They are never written to the
   UI's logs, its settings, the web view's storage, crash reports or analytics.
2. **No analytics.** The UI records nothing about what the user does. Crash reports are local
   and uploaded only after opt-in (Settings → Privacy), and never contain the above.
3. **Logs** of the UI host record message names, sizes and durations, like the service's.
   They never record a message body.
4. **Remembered answers** of a fill-in form are held by the service's worker only. The UI
   receives them in `FormRequested`, shows them, and forgets them when the panel hides.
5. **The palette's query** is cleared when the palette closes. The single exception is the
   user choosing "New snippet labelled “…”", which hands the text to the editor as a draft's
   label.
6. **Usage counts** ("Used 31 times on this computer") are always labelled as local, and
   Settings → Privacy can reset them.
7. **Diagnostics** show codes turned into sentences, a snippet's label where the service
   names a snippet, and an app's name. Never typed text. "Copy details" on an error copies
   codes and message names only.
8. **Web content.** The UI loads no remote content: no web fonts, no images from the network,
   no embedded pages. Links open in the system browser. Formatted snippets are previewed from
   the service's sanitised rendering (raw HTML disabled, links limited to `http`, `https` and
   `mailto`).
9. **What onboarding promises** is exactly what the architecture guarantees: keys are held in
   memory for a moment and never saved or sent; password fields and excluded apps are left
   alone. It does not promise more (architecture §8 lists the detection limits, and the
   Apps settings name the remedy: exclude the app).

## 10. Budgets

| What | Budget | How the UI keeps it |
| --- | --- | --- |
| Plain-text expansion | 50 ms at the 95th percentile | The UI is not involved |
| Fill-in form visible after the trigger | 150 ms | Pre-warmed hidden panel; the fields are rendered from the event with no further request; no animation |
| `FormAck` after `FormRequested` | 2 s, or the expansion is abandoned | Sent when the panel is shown, before first paint finishes |
| Palette visible after the shortcut | 100 ms | Pre-warmed hidden panel; shows with an empty query at once |
| Palette results after a key | 50 ms with 10,000 snippets | Search runs in the service over the snapshot; rows are drawn from the list in memory |
| Library list | 60 frames per second with 10,000 snippets | Only visible rows exist; fixed row height; filtering off the main thread above 2,000 rows |
| Editor preview | 150 ms after the last key | One `RenderPreview`, debounced; an older answer is dropped |
| Install to first expansion | 2 minutes | Five steps, defaults filled in, one decision each |
| Shared folder on the other devices | 10 s | Phase 4; the UI only reacts to `LibraryChanged` |
| UI host idle | Set in task 3.2 | The library window is destroyed when closed; the two panels are small |

## 11. Messages used, and what is needed from the service

### 11.1 Messages each surface uses

All from architecture §6.2 (service endpoint) and §6.4 (account endpoint).

| Surface | Requests | Events |
| --- | --- | --- |
| Menu | `GetStatus`, `SetPaused`, `PutExclusion`, `ListExclusions`, `GetPermissions` | `StatusChanged` |
| Library | `ListFolders`, `ListSnippets`, `ListSnippetSummaries`, `GetSnippet`, `PutSnippet`, `DeleteSnippet`, `PutFolder`, `DeleteFolder`, `PutTag`, `ListTags`, `DeleteTag`, `Import`, `Export`, `Health`, `ListPendingReviews` | `LibraryChanged` |
| Editor | `ParseTemplate`, `RenderPreview`, `CheckAbbreviation`, `PutSnippet`, `ListTags`, `PutTag` | `LibraryChanged` |
| Expansion feedback | (none) | `ExpansionFinished` (no snippet, no text; the form window plays the chime) |
| Fill-in form | `FormAck`, `PanelFocus`, `FormSubmit`, `FormCancel`, `ReopenLastForm` | `FormRequested`, `DiagnosticAdded` |
| Palette | `PanelFocus`, `Search`, `ExpandSnippet`, `RenderPreview`, `ReopenLastForm` | `PaletteRequested`, `DiagnosticAdded` |
| Onboarding | `GetStatus`, `GetPermissions`, `OpenPermissionSettings`, `SetTestField`, `ListFolders`, `PutSnippet`, `GetSettings`, `PutSetting`, `RecentDiagnostics` | `StatusChanged` |
| Settings | `GetSettings`, `PutSetting`, `ListExclusions`, `PutExclusion` | `SettingsChanged` |
| Why didn’t it expand? | `RecentDiagnostics`, `DryRun` | `DiagnosticAdded` |
| Library health | `Health` | `LibraryChanged` |
| Review | `ListPendingReviews`, `ApproveSnippet`, `GetSnippet` | `LibraryChanged` |
| Sharing (Phase 4) | `AccountStatus`, `BeginSignIn`, `CancelSignIn`, `Profiles`, `Members`, `Invite`, `Invitations`, `AcceptInvitation`, `SetFolderShare`, `RemoveFolderShare` | `AccountChanged`, `SyncProgress` |

Settings keys the UI reads and writes with `GetSettings` / `PutSetting`: `delimiters`,
`date_format`, `time_format`, `locale`, `palette_hotkey`, `reopen_form_hotkey`, `pause_hotkey`,
`injection_overrides`, and its own
under the prefix `ui.` (`ui.onboarding_done`, `ui.theme`, `ui.tray_icon`, `ui.expansion_sound`, `ui.new_trigger`,
`ui.new_case_mode`, pane widths). The service stores unknown keys as they are, so no change is
needed for those. Start at login is set through the system's login-item API by the UI host,
and the supervisor's stop and start (ADR-0023) are used for "Start it again" and "Restart the
helper"; neither goes over the service endpoint.

### 11.2 Needed from the service

Each entry says what the design needs, why, and what the UI does until it exists. Entries
marked **blocks** stop a flow from working as specified; the others degrade gracefully. Entries
marked **Available** are answered by the service since protocol 3 (architecture §6.2); the "Until
then" column is kept for a UI that meets an older service.

**For Phase 3**

| # | Needed | Why | Until then |
| --- | --- | --- | --- |
| 1 | **Available.** `ParseTemplate` and `RenderPreview` answered (they returned `not_implemented` before protocol 3; task 3.4). Tokens carry their attributes; errors their code, `start`, `end` and the included snippet that holds them; the preview its parts (text with the ranges each field produced, `sample` where the field's name stands in for a missing value, the clipboard ranges; key presses; waits), the cursor and the fields. Positions are UTF-16 offsets. Tokens should carry each macro's attributes; `RenderPreview` should return the action list (text, key presses, waits), the cursor position and the list of fill-in fields, not only text | Chips, chip cards, the preview with keys and blanks, "Try the form" | **Blocks** the template editor. The UI can read attributes from the source between a token's `start` and `end`, and show a text-only preview |
| 2 | **Available.** Tags: `ListTags` (with counts), `PutTag` (create, rename, recolour; the colour must be one of the eight names, the name unique without regard to case), `DeleteTag` (detaches it everywhere); `PutSnippet` carries the snippet's tags, `GetSnippet` and `ListSnippets` return them, `SnippetQuery.tag` filters by one, and `Search` matches tag names | Tags in the sidebar, the editor and search | **Blocks** tags in 3.3; everything else works without |
| 3 | **Available.** A light way to list snippets: `ListSnippetSummaries` (pages of at most 1,000, no body beyond a first line, with tags and the reason a snippet is broken), and `LibraryChanged` naming the snippets, folders and tags that changed (nothing named means reload: an import) | 10,000 snippets with bodies may not fit one 8 MB frame and take long to redraw | `ListSnippets` per folder; full reload on `LibraryChanged` |
| 4 | **Available** (task 3.10). Usage per snippet (`uses`, `last_used`) in the list answer, `RecentSnippets` for the palette, and `ResetUsage` | "Recently used" in the palette, the editor's footer, "Not used for 90 days" in library health, the reset in Settings | Those parts are hidden; the palette shows its hint (an older service answers `unknown_request`) |
| 5 | Shortcuts: re-register `palette_hotkey` when the setting changes; answer `PutSetting` with an error when the system refuses the chord (today only logged); two more settings, for "fill in the last form again" and "pause or resume". **Available:** all of them (`conflict` when the system refuses the chord, which is then not stored and the old one stays; `invalid` for one that does not parse); the two more are `reopen_form_hotkey` and `pause_hotkey` | The recorder's "Another app already uses this shortcut"; the two unset shortcuts of Settings | The palette shortcut may need a restart of the service to change; the other two stay hidden |
| 6 | **Available.** A timestamp on every diagnostic (`at` is required since protocol 3) | The log's time column | The UI stamps entries it receives live and shows no time for older ones |
| 7 | **Available.** A diagnostic when a matchable abbreviation is followed by Enter or Tab while they are not delimiters (it names the snippet, as other match diagnostics do): `not_a_delimiter` | So the log can answer the most likely beginner question by itself | The fixed line at the foot of the log and the "Test an abbreviation" field |
| 8 | The outcome of an explicit request (`ExpandSnippet`, `FormSubmit`, `ReopenLastForm`) as an event: typed, or refused with the code | A reliable notice. Today success is silent and failure arrives as `DiagnosticAdded`, which the UI must match to its request by snippet and time | The UI matches a `DiagnosticAdded` for the same snippet within 3 s of its request |
| 9 | An event when a permission or the listener state changes | Onboarding and the "Needs you" icon without polling | `GetPermissions` every second while the permissions step is visible, every 30 s while something is missing |
| 10 | **Available.** Restoring a deleted snippet: `PutSnippet` with the deleted snippet's id restores it, with its usage and tags | "Undo" after delete | Undo re-creates the snippet with a new id (usage is lost) |
| 11 | **Available.** A check for an abbreviation (`conflict`, hides, hidden by) without saving: `CheckAbbreviation` | The editor's warnings from the one authority, with the service's case folding | The UI checks against its list with the same folding rule; `PutSnippet` still decides |
| 12 | **Available.** `Search` hits with the matched field and ranges (UTF-16) | Highlighting that agrees with the ranking | The UI marks plain substring matches itself |
| 13 | Packaging: the macOS helper listed as "Iamb" with the app icon in System Settings; a way for the UI host to learn the display the focused window is on | The permissions step; panels on the right screen | Onboarding names the entry as it appears; panels use the display under the pointer |
| 14 | **Available:** `PanelFocus(panel, shown)` (architecture §4.5; ADR-0013 addendum). The UI host sends `shown: true` and waits for the answer before the panel takes the keyboard, sends it again every 10 seconds while the panel is open (a 30-second lease), and sends `shown: false` when the panel gives the keyboard back, before `FormSubmit` or `ExpandSnippet`, and when it resigns the keyboard (a click into the app behind it). A way to say "one of Iamb's panels has the keyboard" (a UI-role message, or suspension from `FormRequested` and `PaletteRequested` until they are answered). Already tracked: finding 16 of the 2.8 review, architecture §8 "Open findings" | On macOS the form and the palette do not activate Iamb, so what is typed into them would be buffered, and could expand, as if typed into the app behind | **Blocks** shipping the form and the palette on macOS (tasks 3.5 and 3.6). No workaround in the UI |
| 15 | **Available.** Screen-lock and sign-out events that clear the remembered answers (finding 13, same place): the service watches the session itself; the UI has nothing to send | "Your last answers" must not survive a locked screen | Answers are still cleared after 30 minutes and at restart; tracked for task 3.5 |

**For Phase 4 (flow 6)**

| # | Needed | Why |
| --- | --- | --- |
| 16 | `ListFolders`, `ListSnippets` and `GetSnippet` over the whole effective library, each folder with its profile, owner, share and the user's permission (they cover the personal profile only) | **Blocks** showing shared folders, "Shared by …", read-only editing |
| 17 | Account endpoint: create a group (`CreateProfile(kind, name)`), rename and delete it, decline an invitation, list and withdraw invitations that were sent, change a member's role, leave a group | **Blocks** "New group…", Decline, Withdraw |
| 18 | For a pending review: the body approved before, the pending body, the author's display name and the time of the change (`ListPendingReviews` gives ids only) | **Blocks** the review dialog's author line and change view |
| 19 | A diagnostic when a snippet held for review was typed | "Needs your review" in the log, with a Review button |
| 20 | The list of changes that were not synced (`ListKeptChanges`, `CopyKeptChange`, `DismissKeptChange`): a change the server refused, because the folder became read-only, was unshared or was deleted long ago, is kept on this computer with its content ([`sync-design.md`](../sync-design.md) D5) | A refused change must not vanish; the user copies it into a snippet of their own or dismisses it |
| 21 | A conflict with both versions, their authors, devices and times, and its two actions (`Conflicts`, `GetConflict`, `ResolveConflict` on the account endpoint; sync design C3, C20) | "Changed on another device" · [Show the other version] in the editor, and the list of conflicts |
| 22 | A mark, with the reason, on a row the server cannot store yet (too large, over the plan's limit); the row keeps working on this computer (sync design D5) | So that "not synced" is never a surprise on the second computer |
| 23 | Devices of the account and "sign out everywhere" (`Devices`, `RenameDevice`, `RevokeDevice`, `SignOutEverywhere`), and deleting the account | Settings → Account; the remedy for a lost computer (sync design §11.3) |
| 24 | The members of a group, with their addresses for its owner and admins (`Members`); the offer of a group's ownership and its answer (`TransferOwnership`, `AcceptOwnership`, `DeclineOwnership`); the name other people see (`SetDisplayName`), asked for at sign-in (sync design §3.1, C16) | The member list; "Ines wants to hand “Brindlewood team” over to you." [Accept] [Decline]; nobody is shown as part of an email address |
| 25 | An approval that names the content the dialog showed (`ApproveSnippet(id, content_hash)`), answered `changed` when the snippet holds other content by then; `needs_review` when a snippet that waits for review is copied, duplicated or moved to another profile; for a pending review, what each `{snippet}` in it resolves to on this computer (sync design D10) | A change that arrives between the dialog and the click is not approved unseen; a copy cannot skip the review; the review can say what an included snippet types |

Entries 20 to 25 come from the sync design (task 4.1, revised after its review), which also
lists, per conflict scenario, what a device shows its user as a code (`conflict`, `kept`,
`blocked`, `shared`, `revoked`, `duplicate`, `broken`, `review`, `adjusted`, `superseded`,
`plan_limit`, `upgrade`, `invitation_invalid`, `refused`, `signed_out`, `restored`;
[`sync-design.md`](../sync-design.md) §12). `signed_out` is "This computer was signed out of
your account. Your own snippets are still here." and `restored` is "Iamb's server was restored
from a backup. Please look at the members of your groups." Task 4.5 gives each its final words
under the rules of §2 and §4.11: all of them appear in the library window, none as a notice
over another application. A conflict (entry 21) can offer more than one other version, when
more than two people changed a snippet; each has its own [Restore].

Nothing else in the six flows needs a message that does not exist.

## 12. Decisions that need the owner

The choices below shape the product more than the pixels do. Each is taken one way in this
specification and listed in [`review.md`](review.md) with the alternative.

1. Saving a snippet is explicit (⌘S / Ctrl+S); there is no autosave.
2. The default global shortcut is Ctrl+Shift+Space on both systems.
3. The tray icon has three states and does not react to the focused app.
4. Iamb shows a notice only for explicit requests that failed, never for a typed abbreviation.
5. The first snippet of onboarding is the user's email address, with the abbreviation `;em`,
   and onboarding recommends starting abbreviations with `;`.
6. The fill-in form has no live preview of the final text.
7. Clicking outside the fill-in form does not cancel it.
8. The palette never remembers its query and lists "recently used" when empty.
9. The review dialog focuses "Not now", and there is no "Reject".
10. Sharing is always with a group, and the UI calls a company profile a "team".
11. Settings, diagnostics and library health are views of the library window, not windows of
    their own.
12. A tag's colour is one of eight named colours, not a free colour.
