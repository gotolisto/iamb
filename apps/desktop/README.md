# Iamb desktop UI host

`iamb-desktop`, the UI host (architecture §3, §10; ADR-0017): a Tauri 2 shell with a React and
TypeScript front end. It starts hidden, stays resident, shows the tray icon (Windows) or
menu-bar icon (macOS), creates the library window on demand, and keeps the fill-in form and the
search palette created but hidden. It talks to the background service (`iamb-service`) over
`iamb-ipc` with the role `ui`; the front end never speaks IPC itself, it calls the Tauri
commands of the Rust side.

| Path | What |
| --- | --- |
| `src/` | The front end (Vite, React, TypeScript). `ipc.ts` is the typed API over the Tauri commands and events; `library/`, `form/`, `palette/` the three views (placeholders until tasks 3.3, 3.5, 3.6); `styles/tokens.css` the design tokens, generated from `docs/ux/tokens.json` by `pnpm tokens` (`scripts/tokens.mjs`, the same rules as the mock-ups' builder); `styles/base.css` uses only those. |
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
- The fill-in form (3.5) and the palette (3.6): their content, the non-activating panels on
  macOS, the foreground grant on Windows, and the focus hand-back.
- The library (3.3), the template editor (3.4), onboarding (3.7), settings (3.9), and the look
  of the design system (3.1).
