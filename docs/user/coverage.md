# Coverage

Which user-facing feature is documented where. This list is for the team, not for readers.

It was built from the desktop app's source (`apps/desktop`), the
command line (`crates/iamb-cli`), the design documents in `docs/` and the UX specification in
`docs/ux/spec.md`.

Status:

- **Documented**: the feature is in the app and the page describes it.
- **Gap**: the feature is in the specification but not in the app, or it is in the app but not on a page.
- **Not in this version**: the feature belongs to sync or accounts, which are in progress.
- **TODO**: the page says something that still needs confirming. The TODO is also in the page itself.

Counts: 114 features listed. Documented 92, Gap 17, Not in this version 5. The gaps and TODOs are
listed at the end. Documented rows were checked against the code on the date of the last review:
claims the code did not support were corrected or removed, and the pages say where something is
not confirmed.

## Expansion

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| Expand after a space or punctuation | `TypingSection.tsx`, `architecture.md` 4.4 | [Snippets](snippets.md#options) | Documented |
| Expand immediately | `TypingSection.tsx` | [Snippets](snippets.md#options) | Documented |
| Only at the start of a word | `Editor.tsx`, `model.rs` | [Snippets](snippets.md#options) | Documented |
| Capital letters: exact, ignore, follow mine | `TypingSection.tsx` | [Snippets](snippets.md#options) | Documented |
| Delimiter keys (punctuation, closing brackets, Enter, Tab) | `TypingSection.tsx`, `model.ts` | [Settings](settings.md#typing) | Documented |
| Enter and Tab as delimiters, with a confirmation | `TypingSection.tsx` | [Settings](settings.md#typing), [Troubleshooting](troubleshooting.md#enter-and-tab-do-not-expand) | Documented |
| Pause expansion (menu, switch and the sidebar status line) | `tray.rs`, `GeneralSection.tsx`, `Sidebar.tsx` | [Settings](settings.md#general), [Exclusions](exclusions.md#pause-everywhere) | Documented |
| Turn a snippet on or off | `ListPane.tsx` | [Snippets](snippets.md#turn-a-snippet-on-or-off) | Documented |
| Abbreviation rules (no spaces, 64 characters, unique) | `architecture.md` 4.4, `Editor.tsx` | [Snippets](snippets.md#abbreviation-rules) | Documented |
| Shadowed and never-reached abbreviations | `architecture.md` 4.4, `health` | [Snippets](snippets.md#library-health) | Documented |
| Excluded apps | `AppsSection.tsx` | [Exclusions](exclusions.md) | Documented |
| Password fields never expand | `architecture.md` 4.1, 8 | [Exclusions](exclusions.md#password-fields), [Privacy](privacy.md) | Documented |
| Input-method composition suppresses expansion | `architecture.md` 4.1 | [Troubleshooting](troubleshooting.md#nothing-expands-while-you-type-a-special-character) | Documented |
| Per-app injection settings (send method, line breaks, delay) | `AppsSection.tsx` | [Settings](settings.md#apps), [Troubleshooting](troubleshooting.md#text-comes-out-wrong-in-an-app) | Documented |
| Typing during an expansion (out-of-order warning) | `architecture.md` 4.6 | [Troubleshooting](troubleshooting.md#keys-typed-fast-come-out-in-the-wrong-order) | Documented |
| Windows refuses input into higher-rights windows | `architecture.md` 5 | [Troubleshooting](troubleshooting.md#windows-will-not-let-iamb-type-into-a-window) | Documented |
| Clipboard restored after paste; clipboard history skipped | `architecture.md` 8 rule 8 | [Privacy](privacy.md#the-clipboard) | Documented |
| "Why didn't it expand?" log (200 entries, memory only) | `diagnostics/`, `architecture.md` 8 rule 4 | [Troubleshooting](troubleshooting.md), [Privacy](privacy.md#what-iamb-stores-and-where) | Documented |
| "Test an abbreviation" in the log | `Diagnostics.tsx` | [Troubleshooting](troubleshooting.md) | Documented |
| "Show technical entries" in the log | `Diagnostics.tsx` | [Troubleshooting](troubleshooting.md) | Documented |

## Templates

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| Plain text, brace rules, `{lbrace}` | `template-language.md` 2, 4.9 | [Templates](templates.md#plain-text), [Templates](templates.md#insert-a-literal-brace) | Documented |
| `{date}` with format and shift | `template-language.md` 4.1 | [Templates](templates.md#date-and-time) | Documented |
| `{time}` | `template-language.md` 4.1 | [Templates](templates.md#date-and-time) | Documented |
| Date and time format settings, names by locale | `DatesSection.tsx`, `model.ts` | [Settings](settings.md#dates) | Documented |
| `{clipboard}` with `trim` | `template-language.md` 4.2 | [Templates](templates.md#clipboard) | Documented |
| `{cursor}` | `template-language.md` 4.3 | [Templates](templates.md#cursor-position) | Documented |
| `{snippet}` | `template-language.md` 4.4 | [Templates](templates.md#another-snippet) | Documented |
| `{field}` (line, several lines, choice, toggle) | `template-language.md` 4.5 | [Templates](templates.md#fill-in-fields), [Forms](forms.md#field-types) | Documented |
| `{key}` with shift and times | `template-language.md` 4.6 | [Templates](templates.md#key-presses-and-waits) | Documented |
| `{wait}` | `template-language.md` 4.7 | [Templates](templates.md#key-presses-and-waits) | Documented |
| `{if}`, `{else}`, `{endif}`, implicit toggles | `template-language.md` 4.8 | [Templates](templates.md#optional-parts) | Documented |
| Template errors and the "Broken" state | `template-language.md` 5 | [Templates](templates.md#errors), [Snippets](snippets.md#library-health) | Documented |
| Insert menu opened with `{`, chips, chip cards | `TemplateField.tsx`, `macros.ts` | [Templates](templates.md), [Keyboard](keyboard.md#snippet-editor) | Documented |
| Preview in the editor, "Try the form" | `TemplatePreview.tsx` | [Templates](templates.md#preview) | Documented |
| "Show as text" to edit the raw source | UX spec 4.3 | none | Gap |

## Fill-in forms

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| Form opens over the app; Enter inserts; Esc cancels | `FormPanel.tsx` | [Forms](forms.md) | Documented |
| Keys: Tab, arrows, Space, Ctrl+Enter | `FormPanel.tsx`, `form/model.ts` | [Forms](forms.md#fill-it-in), [Keyboard](keyboard.md#fill-in-form) | Documented |
| Start empty (Ctrl+Backspace) with remembered answers | `FormPanel.tsx` | [Forms](forms.md#your-last-answers) | Documented |
| Remembered answers for 30 minutes, memory only | `architecture.md` 4.5 | [Forms](forms.md#your-last-answers) | Documented |
| Refused insert: notice, answers kept, focus not returned | `architecture.md` 4.5, `notice.rs` | [Forms](forms.md#when-iamb-cannot-type) | Documented |
| 100-field limit | `template-language.md` 4.5 | [Forms](forms.md#field-types) | Documented |

## Search window

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| Search shortcut (Ctrl+Shift+Space, changeable) | `palette.rs`, `ShortcutsSection.tsx` | [Search](search.md#open-search), [Settings](settings.md#shortcuts) | Documented |
| Search by abbreviation, label, text and tag | `architecture.md` 10 | [Search](search.md#find-a-snippet) | Documented |
| Recently used snippets and "Fill in again" when empty | `palette/model.ts` | [Search](search.md#find-a-snippet) | Documented |
| Choose: Enter, Ctrl+1 to Ctrl+9, Ctrl+E | `PaletteView.tsx` | [Search](search.md#choose-a-snippet) | Documented |
| Works while paused | UX spec 4.5, `architecture.md` 4.5 | [Search](search.md) | Documented |
| Search inside the library (Ctrl+K) | `Library.tsx` | [Search](search.md#open-search) | Documented |
| Review badge in search results | `palette/model.ts` | none | Not in this version (sync) |

## Library window

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| New snippet (Ctrl+N), save (Ctrl+S), drafts | `ListPane.tsx`, `Editor.tsx` | [Snippets](snippets.md#make-a-snippet) | Documented |
| Filter (Ctrl+F) and sort (label, recently changed, most used) | `ListPane.tsx` | [Snippets](snippets.md#find-and-sort) | Documented |
| Folders up to four levels; new folder inside; folder settings; delete | `Sidebar.tsx`, `Library.tsx` | [Snippets](snippets.md#folders) | Documented |
| Move a snippet by its Folder field | `Editor.tsx` | [Snippets](snippets.md#folders) | Documented |
| Folder reordering and nesting by drag | UX spec 4.2 | none | Gap |
| Tags with eight colours; rename, recolour, delete | `TagInput.tsx`, `api.ts` | [Snippets](snippets.md#tags) | Documented |
| Duplicate (Ctrl+D) | `ListPane.tsx` | [Snippets](snippets.md#duplicate-a-snippet) | Documented |
| Delete with Undo for 10 seconds | `Library.tsx`, `Toasts.tsx` | [Snippets](snippets.md#delete-a-snippet) | Documented |
| Multi-select with Shift and Ctrl+A; menu for several (Turn on or off, Delete; Duplicate for one) | `ListPane.tsx`, `Library.tsx` | [Snippets](snippets.md#select-several-snippets) | Documented |
| Library health: four sections | `health/` | [Snippets](snippets.md#library-health) | Documented |
| Formatted and plain snippet kinds | `Editor.tsx` | [Formatted snippets](formatted-snippets.md) | Documented |
| Formatting toolbar and shortcuts (bold, italic, link) | `TemplateField.tsx` | [Formatted snippets](formatted-snippets.md#formatting) | Documented |
| Images: PNG, JPEG, GIF, WebP, up to 5 MB | `TemplateField.tsx` | [Formatted snippets](formatted-snippets.md#images) | Documented |
| Links limited to http, https and mailto | `link.rs`, `architecture.md` 8 rule 10 | [Formatted snippets](formatted-snippets.md#links) | Documented |
| Warning when a formatted snippet becomes plain | `Editor.tsx` | [Formatted snippets](formatted-snippets.md) | Documented |
| Library window keyboard map (panes, F6, zoom, close) | `Library.tsx`, `windows.rs` | [Keyboard](keyboard.md#library-window) | Documented |
| Import and export from the library window | UX spec 4.2 | none | Gap |
| Shared folders, "Needs review", "Copy to my snippets" | `Sidebar.tsx`, UX spec 4.10 | [Sync](sync.md) | Not in this version (sync) |
| Share a folder (Ctrl+Shift+S) | UX spec 5.2 | none | Gap |

## Settings

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| General: pause switch, tray icon switch, appearance | `GeneralSection.tsx` | [Settings](settings.md#general) | Documented |
| Start Iamb when I log in (setup only; registers the background service, which does not start the icon yet) | `onboarding/steps.tsx`, `login_item.rs`, `supervisor.rs` | [Get started](getting-started.md#start-at-login), [Settings](settings.md#settings-not-in-this-version) | Documented |
| Start Iamb when I log in in Settings | UX spec 4.7 | none | Gap |
| Updates section and automatic updates | UX spec 4.7 | [Settings](settings.md#settings-not-in-this-version) | Gap |
| Typing settings | `TypingSection.tsx` | [Settings](settings.md#typing) | Documented |
| Date and time formats with custom format and errors | `DatesSection.tsx` | [Settings](settings.md#dates) | Documented |
| Month and day names: English, Español, Deutsch, Français | `DatesSection.tsx` | [Settings](settings.md#dates) | Documented |
| Shortcut for search | `ShortcutsSection.tsx` | [Settings](settings.md#shortcuts) | Documented |
| Shortcuts for fill-in again and pause | UX spec 5.1 | [Settings](settings.md#settings-not-in-this-version) | Gap |
| Excluded apps list | `AppsSection.tsx` | [Settings](settings.md#apps), [Exclusions](exclusions.md) | Documented |
| Usage counts and reset | `PrivacySection.tsx` | [Settings](settings.md#privacy) | Documented |
| Send crash reports switch (saved; nothing sends reports) and local crash reports | `PrivacySection.tsx`, `crash.rs`, `architecture.md` 8 rule 9 | [Settings](settings.md#privacy), [Privacy](privacy.md#crash-reports) | Documented as not sent, with a TODO |
| About: licence and source link | `AboutSection.tsx` | [Settings](settings.md#about) | Documented |
| Version in About | UX spec 4.7 | none | Gap |
| Account section | UX spec 4.7 | [Accounts](accounts.md) | Not in this version (accounts) |
| Open the log folder from About | UX spec 4.7 | none | Gap |

## Menu, onboarding and system

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| Menu: status line, Open library, Search, Fill in again, Pause, Set up, Fix permissions, Quit | `tray.rs` | [Get started](getting-started.md#the-menu) | Documented |
| Menu: Settings item | UX spec 4.1 | [Get started](getting-started.md#the-menu) (says it is missing) | Gap |
| Menu: New snippet… and Why didn't it expand? | UX spec 4.1 | [Get started](getting-started.md#the-menu) (says they are missing) | Gap |
| Menu: "Never expand in this app" | UX spec 4.1 | [Get started](getting-started.md#the-menu) (says it is missing) | Gap |
| Quit asks about unsaved edits | `Library.tsx`, `LibraryView.tsx` | [Get started](getting-started.md#the-menu) (menu) | Documented |
| Quit confirmation with the "stop expanding" sentence, and Quit stopping the service | UX spec 4.1, `commands.rs` `app_quit` (`app.exit(0)`, UI host only) | [Get started](getting-started.md#the-menu) (says it is not confirmed) | Gap |
| "Start it again" and "Restart" buttons (setup window; the library banner has none) | UX spec 4.2, `steps.tsx`, `onboarding.rs` `NoSupervisor` | [Troubleshooting](troubleshooting.md#iamb-is-not-running) | Gap: the buttons exist in setup but always fail |
| Library banner for missing permissions (macOS) | UX spec 4.2 | [Troubleshooting](troubleshooting.md#a-permission-is-missing-macos) (says it is missing) | Gap |
| Open the library by opening the Iamb app again, or from the Applications folder or Start menu | `main.rs` (a second start exits) | [Get started](getting-started.md#find-iamb-after-setup) (says it does not) | Gap |
| Onboarding: welcome, permissions (macOS), first snippet, try it, done | `onboarding/steps.tsx` | [Get started](getting-started.md) | Documented |
| Permission rows and their reasons | `onboarding/model.ts` | [Get started](getting-started.md#permissions) | Documented |
| Background service restarts; "Iamb is not running" | `architecture.md` 3.2 | [Troubleshooting](troubleshooting.md#iamb-is-not-running) | Documented |
| Library data location per system | `architecture.md` 3.3 | [Privacy](privacy.md#what-iamb-stores-and-where) | Documented |
| Accessibility: keyboard, shortcuts on controls, reduced motion, forced colours | `keyboard-run`, CSS | [Accessibility](accessibility.md) | Documented |
| Screen reader testing | `test-strategy.md` 8.10 | [Accessibility](accessibility.md#what-is-not-checked-yet) | Documented as not done |

## Command line

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| `iamb add` with label, folder, trigger, case, disabled, body file | `cli.rs`, `commands/add.rs` | [Command line](cli.md#add-a-snippet) | Documented |
| `iamb list` with folder filter and JSON | `cli.rs` | [Command line](cli.md#list-snippets) | Documented |
| `iamb remove` (soft delete) | `cli.rs` | [Command line](cli.md#remove-a-snippet) | Documented |
| `iamb import` with `--on-conflict` | `cli.rs`, `transfer.rs` | [Import and export](import-export.md#import) | Documented |
| `iamb export` (JSON and CSV) | `cli.rs`, `transfer.rs` | [Import and export](import-export.md#export) | Documented |
| `iamb expand` (dry run, fields, clock, clipboard, cursor, actions, exit status) | `cli.rs`, `commands/expand.rs` | [Command line](cli.md#check-what-a-snippet-would-type) | Documented |
| `--data-dir` and `IAMB_DATA_DIR` | `cli.rs`, `session.rs` | [Command line](cli.md#where-the-library-is) | Documented |
| CSV columns and escaping | `transfer.rs` | [Import and export](import-export.md#csv-files) | Documented |
| Installing the `iamb` command | none in the repository | none | Gap |

## Installing, privacy and help

| Feature | Source | Page | Status |
| --- | --- | --- | --- |
| Install on macOS and Windows | `docs/release.md`, `apps/desktop/release/` | [Get started](getting-started.md#install) | Documented, with TODOs (releases link, signing state) |
| Privacy: what is kept, where, what leaves the computer | `architecture.md` 3, 8 | [Privacy](privacy.md) | Documented |
| Licence and source | `LICENSE`, `AboutSection.tsx` | [Index](index.md), [Settings](settings.md#about) | Documented |
| Compatibility notes by app type | `compatibility-matrix.md`, `compatibility-run.md` | [Troubleshooting](troubleshooting.md#apps-that-need-care) | Documented, results TODO |
| Keyboard shortcut reference | `ux/spec.md` 5, app | [Keyboard](keyboard.md) | Documented |
| Sync between computers | `sync-design.md` | [Sync](sync.md) | Not in this version (sync) |
| Accounts and sign-in | `architecture.md` 6.4 | [Accounts](accounts.md) | Not in this version (accounts) |

## Gaps and TODOs

Gaps are features in the specification, or in the app, that the pages do not cover, or cover by
saying they are missing. Each one needs a decision before the page can say more.

1. **Release page URL and signing state.** Confirm before publishing. Every build is unsigned until
   the signing secrets exist (`docs/release.md`).
2. **Crash reports.** The switch is saved, but no code sends reports. The Settings page still says
   "A report says where Iamb failed". Confirm before publishing.
3. **Start at login in Settings.** Only in setup. It registers the background service; the
   supervisor does not start the menu icon or windows yet.
4. **Updates.** No update check or automatic install in the app.
5. **Shortcuts for fill-in again and pause.** Not in the app.
6. **Import and export in the library window.** Only in the command line. The paste-fidelity notes
   refer to an Import button that does not exist yet.
7. **Share a folder, and drag to reorder or nest.** Not in the app.
8. **Show as text (raw source) in the editor, and Move to… in More actions.** Not in the app.
9. **Menu: Settings, New snippet…, Why didn't it expand?, "Never expand in this app".** Not in the
   app.
10. **Quit Iamb.** It asks only about unsaved edits, and it ends the UI host only. The spec says it
    asks "Abbreviations stop expanding…" and stops the service too. Not confirmed what keeps running.
11. **Restart buttons.** "Start it again" and "Restart" exist in the setup window but cannot restart
    anything (`NoSupervisor`). The library's "Iamb is not running" banner has no button.
12. **Missing-permission banner in the library window (macOS).** Not in the app.
13. **Opening the library from Applications or the Start menu.** A second start of Iamb exits without
    opening anything. With the menu icon hidden, the library is reached through the search
    shortcut and Ctrl+E. Confirm with the design.
14. **Installing the `iamb` command.** The installers carry the desktop app and the service only.
15. **Compatibility results.** The matrix is a plan. No published results exist yet
    (`docs/results/` does not exist), so `troubleshooting.md` lists the differences without test
    outcomes.
16. **Version number in About, and the log folder button.** Not shown.
17. **Linux desktop.** The architecture says Linux is for development and the command line, and the
    Linux package is marked development. The pages follow the architecture.
18. **Accessibility results.** The manual screen-reader runs are not recorded.
19. **Review of snippets from other people, shared folders, accounts.** Belong to sync and accounts,
    which are in progress. The sync client crate exists but nothing starts it.

Not verified against code or a running app: the text of system prompts in the installers, what opening
the installed app starts, whether the menu's Quit leaves the service running, and the screen-reader
behaviour.
