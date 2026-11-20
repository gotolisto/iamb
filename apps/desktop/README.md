# Iamb desktop UI host

`iamb-desktop`, the UI host (architecture §3, §10; ADR-0017): a Tauri 2 shell with a React and
TypeScript front end. It starts hidden, stays resident, shows the tray icon (Windows) or
menu-bar icon (macOS), creates the library window on demand, and keeps the fill-in form and the
search palette created but hidden. It talks to the background service (`iamb-service`) over
`iamb-ipc` with the role `ui`; the front end never speaks IPC itself, it calls the Tauri
commands of the Rust side.

| Path | What |
| --- | --- |
| `src/` | The front end (Vite, React, TypeScript). `ipc.ts` is the typed API over the Tauri commands and events; `library/` the library window (task 3.3: sidebar, virtualised snippet list, editor; see below), `form/` the fill-in form panel (task 3.5; see below), `palette/` the search palette (task 3.6; see below); `styles/tokens.css` the design tokens, generated from `docs/ux/tokens.json` by `pnpm tokens` (`scripts/tokens.mjs`, the same rules as the mock-ups' builder); `styles/base.css` uses only those. |
| `src-tauri/` | The `iamb-desktop` crate: `src/bridge/` (the connection to the service, reconnecting with backoff), `src/contract.rs` (what the front end sees and how it maps onto `iamb-ipc`), `src/app/` (tray, windows, commands), `src/crash.rs` and `src/log.rs` (as in the service, architecture §8). |
| `src-tauri/tests/` | Bridge, message contract and crash-isolation tests against a worker on the mock platform. |
| `icon-source/icon.svg` | The source of `src-tauri/icons/` (a placeholder mark); regenerate with `pnpm tauri icon icon-source/icon.svg -o src-tauri/icons` and keep only the desktop files. |

## Prerequisites

- The Rust toolchain of `rust-toolchain.toml`, Node.js 22 and pnpm 10 (the exact version is in
  `package.json`, `packageManager`).
- Linux (development only): the web view and tray libraries, and Xvfb to run without a display:

      sudo apt-get install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev xvfb

- Windows: the WebView2 runtime (part of Windows 11). macOS: nothing beyond Xcode's command-line
  tools.

Install the front end's dependencies once (the lockfile is committed; versions are exact):

    cd apps/desktop
    pnpm install --frozen-lockfile

## Running in development

The UI host needs a service to talk to, but runs without one (the tray says "Service: not
running, retrying" and it reconnects when the service appears). Use one data directory for both:

    # 1. The service; on Linux it runs against the mock platform (no real keyboard).
    cargo run -p iamb-service -- --worker --platform mock --data-dir /tmp/iamb-dev

    # 2. The UI host with live reload: starts Vite on port 1420, builds and runs the crate.
    cd apps/desktop
    IAMB_DATA_DIR=/tmp/iamb-dev pnpm tauri dev

    #    Without a tray (a Linux desktop without a status-notifier host, or under Xvfb), open
    #    the library window at start:
    IAMB_DATA_DIR=/tmp/iamb-dev pnpm tauri dev -- -- --open-library

A plain `cargo build -p iamb-desktop` builds the development variant, which loads the front end
from the Vite server (`pnpm dev`) instead of embedding it. To build the variant that embeds it,
as a release does:

    pnpm build
    cargo build -p iamb-desktop --release --features tauri/custom-protocol

(`pnpm tauri build` does both and also makes the installers; packaging is a later task.)

The UI host writes `logs/desktop.log` in the data directory: connection changes, window
events and timings, never typed text, form values or message bodies (architecture §8). It takes
`run/desktop.lock`, so a second UI host for the same data directory exits at once.

## The library window

`src/library/` (task 3.3). `Library.tsx` holds the window's state and shortcuts; `useLibraryData.ts`
keeps folders, tags and every snippet's summary in memory (pages of 1,000, read again for what a
`LibraryChanged` names); `VirtualList.tsx` draws only the rows in view (48 px rows, 10,000
snippets); `Editor.tsx` is the snippet editor with explicit save. Its content field is the
template editor (task 3.4, `src/library/template/`: `TemplateField.tsx` with chips for fields and
macros, checked by the service's `template_parse` and previewed with `template_preview`). `api.ts`
mirrors `src-tauri/src/library.rs` (the `library_*` commands). Tests: `src/library/*.test.ts(x)`
against `testkit.tsx` (a fake service), `src-tauri/tests/library.rs` against a real worker.

Settings and "Why didn't it expand?" (task 3.9) are views of the same window:
`src/library/settings/` (one file per section) and `src/library/diagnostics/` (`reasons.ts` holds
the sentence for each diagnostic code; `codes.test.ts` fails when a code has none). They speak to
the service through `src-tauri/src/settings.rs` (the `settings_*` and `diagnostics_*` commands).
`src-tauri/tests/settings.rs` holds one persistence test per setting and a test that fails when a
setting the service reads has none.

    # scroll measurement with 10,000 snippets (needs Playwright; see docs/test-strategy.md §8.4)
    pnpm build && NODE_PATH=/path/to/node_modules node scripts/scroll-kpi.mjs

## The onboarding window

`src/onboarding/` and `src-tauri/src/onboarding.rs` (task 3.7). One window, label `onboarding`
(560 × 600), created on demand and destroyed when closed, opened by `src-tauri/src/app/onboarding_commands.rs`:
at start while `ui.onboarding_done` is not set, from the menu's "Set up Iamb…" (offered while setup is
unfinished) and from "Fix permissions…" (offered once setup is done while a permission is missing; a
thread asks the service every 30 s), which shows the permissions step alone (`?mode=fix`). The
steps are in `docs/ux/flows/1-onboarding.md`; `Onboarding.tsx` holds the state, `steps.tsx` one
component per step, `model.ts` the rules and the copy, `example.ts` the snippet the Try step expands.

- **Permissions** come from the worker (`GetPermissions`, ADR-0021), never from the UI host
  itself, and are read again every second while the step is shown (the service has no event for it
  yet, spec §11.2 entry 9). "Open settings" sends `OpenPermissionSettings`.
- **The test field** is the window itself: on entering the Try step the window asks the platform's
  focus inspector which window has the keyboard, refuses one that is not this process's, and sends
  `SetTestField` with it; leaving the step, reaching the end and closing the window withdraw it.
  What is typed there stays in the page's state and is never sent, logged or stored.
- **The end** (`onboarding_finish`) withdraws the field, stores the default search shortcut when
  none is set (a refusal is shown with a recorder) and `ui.onboarding_done`; the login item is
  registered through the platform's `LoginItem`.
- **Not built yet:** "Restart the helper" and "Start it again" need a control channel to the
  supervisor (ADR-0023); until it exists `onboarding_restart_helper` answers `unavailable` and the
  window says to quit Iamb and open it again. "I have snippets to import" needs the file dialog and
  the import command, which the UI host does not have yet.

Tests: `src/onboarding/*.test.ts(x)` (over a fake service), `src-tauri/tests/onboarding.rs` (a
real worker on the mock platform) and `tests/onboarding_contract.rs`; the timing run and the
walkthrough with testers are in `docs/test-strategy.md` §8.6 and `docs/onboarding-walkthrough.md`.

## The fill-in form panel

`src/form/` and `src-tauri/src/form.rs` (task 3.5). `form.rs` is the flow, independent of
Tauri: on `FormRequested` it answers `FormAck`, reads the snippet's header, lets the hidden panel
render, sends `PanelFocus { form, shown: true }` and waits for its answer, then shows the panel
and renews the lease every 10 s; Insert hides the panel, hands the focus back (`restore_focus`),
sends `PanelFocus { shown: false }` and then `FormSubmit`; Cancel does the same with
`FormCancel`. `src-tauri/src/app/form_panel.rs` is the window side and the `form_*` commands
(they answer the `form` window only); `src-tauri/src/app/windows.rs` has the panel helpers the
palette uses too (`show_panel`, `hide_panel`, `panel_frame`). `src/form/FormPanel.tsx` draws the
fields, `FormView.tsx` holds the answers for the panel's lifetime only, `model.ts` has the rules
and the copy. The tray's "Fill in the last form again…" sends `ReopenLastForm`. Tests:
`src/form/*.test.ts(x)`, `src-tauri/tests/form.rs` (the bridge and a real worker on the mock
platform).

    # the 150 ms measurement (docs/test-strategy.md §8.5)
    xvfb-run -a target/release/iamb-desktop --data-dir "$(mktemp -d)" --measure-form 20 --budget-ms 150

## The search palette

`src/palette/` and `src-tauri/src/palette.rs` (task 3.6). `palette.rs` is the flow, independent of
Tauri, built like the form's: on `PaletteRequested` it takes `PanelFocus { palette, shown: true }`
and waits for its answer before the panel takes the keyboard, shows the panel (620 by 560 px,
upper third of the display) and renews the lease every 10 s; a pick hides the panel, hands the focus
back (`restore_focus`), sends `PanelFocus { shown: false }` and then `ExpandSnippet` (a snippet
with blanks makes the service send `FormRequested` and the form panel takes over) or
`ReopenLastForm` ("Fill in “…” again"); Esc does the same without the last message; the shortcut
again closes it, and while a form is open it does nothing. `palette_data.rs` is the search and its
rows: `Search`, the rows drawn from the snippet list this side reads in pages and keeps (read again
after a `LibraryChanged`), the preview (`GetSnippet`, `RenderPreview`). `src-tauri/src/app/palette_panel.rs`
is the window side and the `palette_*` commands (they answer the `palette` window only);
`src/palette/PaletteView.tsx` draws it and holds the query in its state for the palette's lifetime
only. Tests: `src/palette/*.test.ts(x)`, `src-tauri/tests/palette.rs` (the bridge and a real worker
on the mock platform), the relevance fixtures `spec/search/relevance.toml`.

- **Who registers the shortcut.** The service: it registers the chord of the `palette_hotkey`
  setting (`PutSetting` answers `conflict` when the system refuses it), hears it, captures a focus
  token and sends `PaletteRequested`. The UI host never registers a shortcut. The service has no
  default of its own, so the UI host offers `Ctrl+Shift+Space` once to a service that has no
  setting (`palette::seed_default_shortcut`, a marker file in `run/` so that a shortcut the user
  clears is not brought back); the onboarding window's last step does the same when the setting is
  still unset.
- **From the tray**, "Search snippets…" opens the palette for the application in front at that
  moment; **in the library window**, Ctrl+K (⌘K) opens it over the window, and so does the
  shortcut while the library has the focus: a pick then selects the snippet in the library
  instead of typing it. "New snippet labelled “…”", "edit" (Ctrl+E, ⌘E), a review and a broken
  snippet hand the library window a request (`library_take_request`, `src/library/usePaletteRequests.ts`).
- **Ranking** is `iamb_core::search` (module docs): abbreviation, then label, content and tag, with
  typos forgiven last, in the service's `Search`.

    # the 100 ms measurement (docs/test-strategy.md §8.7)
    xvfb-run -a target/release/iamb-desktop --data-dir "$(mktemp -d)" --measure-palette 20 --budget-ms 100

### The macOS panel

On macOS the form and the palette are non-activating panels, so that the app being typed into
stays the active app (spec §4.4, §7; ADR-0017). Tauri 2 has no such window, and turning its
`NSWindow` into an `NSPanel` takes Objective-C calls that are `unsafe`, which this crate cannot
make (`unsafe_code = "forbid"` in the workspace lints, which it inherits). The conversion is
therefore in `iamb-platform-macos`, behind its `panel` feature, which only this crate enables (the
service links that crate without it, so its binary is unchanged; ADR-0008):

```rust
/// Turns `window` (a Tauri window, through `raw-window-handle`) into a non-activating panel.
/// Main thread only; idempotent.
pub fn make_non_activating_panel(
    window: &impl raw_window_handle::HasWindowHandle,
) -> Result<(), PlatformError>;
```

It takes the `NSView` of the `AppKit` handle and its `window`, changes the window's class to
`IambNonActivatingPanel`, a subclass of `NSPanel` registered once at run time, adds
`NSWindowStyleMask::NonactivatingPanel` to the style mask (after the class, so that `NSPanel`'s
`setStyleMask:` sees it), sets `becomesKeyOnlyIfNeeded` to `NO`, `floatingPanel` to `YES`,
`hidesOnDeactivate` to `NO` and adds `CanJoinAllSpaces | FullScreenAuxiliary` to the collection
behaviour. Why the class change is sound, and the checks it makes before it (`NSPanel` and the
subclass add no instance variables; the window's own class keeps its instance variables, has no
`dealloc` of its own, and is the same for every window converted) are in
`crates/iamb-platform-macos/src/panel.rs`.

One difference from the first plan: the panel's `canBecomeKeyWindow` does not answer `YES` but asks
the implementation of the window's original class (Tauri's), which answers Tauri's `focusable`
flag. A plain `YES` would let `prepaint_panel`, which shows a panel out of sight with
`set_focusable(false)`, take the keyboard from the app the user is typing into while the panel
paints, and a panel that steals keys is the failure this all exists to prevent. The panel never
becomes the main window.

`windows::prewarm` calls it for `palette` and `form` right after they are created (in the setup
hook, on the main thread, before either is shown), and `windows::show_panel` then calls `show`
without `set_focus` for a converted panel: `show` orders the panel front and makes it key, which a
non-activating panel does without activating Iamb, where `set_focus` would activate the app. If a
conversion fails, the log says so (`the form window stays an ordinary window, which activates
Iamb: …`) and that panel is shown with `set_focus` as before; the flow, `PanelFocus` and the
hand-back are the same either way.

`crates/iamb-platform-macos/tests/panel.rs` (`cargo test -p iamb-platform-macos --features panel
--test panel`, macOS only) converts a borderless window of an `NSWindow` subclass that keeps a
`focusable` flag as Tauri's does, checks the class, the mask and the other settings, that the flag
still decides `canBecomeKeyWindow`, that a second call changes nothing and a window of another
class is refused; then, with a window server session, that the shown panel becomes the key window
while the app does not become active, and does not become key while its flag says not focusable.
What only a Mac with the real UI host can confirm (the target app stays active, keys typed into the
panel go to the panel, the hand-back) is in `docs/compatibility-run.md`, "The macOS panel" (M-P1
to M-P5).

## Checks

    pnpm typecheck
    pnpm test
    cargo clippy -p iamb-desktop --all-targets --locked -- -D warnings
    cargo test -p iamb-desktop --locked

After a change to `docs/ux/tokens.json`, run `pnpm tokens` (a test fails until then).
`IAMB_BLESS=1 cargo test -p iamb-desktop --test front_end_contract` rewrites
`src/contract.fixtures.json` after a deliberate change to what the Rust side sends; update
`src/ipc.ts` to match and review the diff.

## Measuring the cold open

`iamb-desktop --measure-open RUNS [--budget-ms MS]` opens the library window `RUNS` times as the
tray action does, prints when each first painted, and exits (status 1 over the budget). Use a
binary that embeds the front end (above):

    xvfb-run -a target/release/iamb-desktop --data-dir "$(mktemp -d)" --measure-open 20 --budget-ms 400

How it measures and what it gave is in `docs/test-strategy.md` §8.

## What later tasks add

- The supervisor starts and restarts the UI host at login (architecture §3.2); until then start
  it yourself as above.
- Rich text (3.8), library health (3.10) and the accessibility pass (3.11).
