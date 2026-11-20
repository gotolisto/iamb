# Continuous integration

CI is defined in `.github/workflows/ci.yml` and runs on every pull request, on pushes to `main`
and on manual dispatch. The toolchain version comes from `rust-toolchain.toml`; warnings are
errors (`RUSTFLAGS=-D warnings`). The pipeline should finish in under ten minutes; the
`desktop` legs, which build Tauri, take about 3 to 5 minutes with a warm cache and up to about
12 minutes cold (Windows is the slowest).

The UI host (`iamb-desktop`, `apps/desktop`) is a workspace member but has a job of its own:
`lint` and `test` pass `--exclude iamb-desktop` to their workspace-wide commands, because the
crate needs the system web view's development files on Linux and a front-end build, and its
Tauri dependencies would slow every leg.

## Jobs

- `lint` (Linux): formatting, clippy with `-D warnings`, clippy of the two OS crates for their
  own targets (`iamb-platform-windows` for `x86_64-pc-windows-msvc`, `iamb-platform-macos` for
  `aarch64-apple-darwin`; see "Checking Windows and macOS code on Linux" below), the banned-term
  check, and the template spec examples checked against the reference implementation.
- `test` (Linux, Windows, macOS, not fail-fast): `cargo build` and `cargo test` for the whole
  workspace with `--locked`. Between the two, each leg builds the service on its own in
  release (`cargo build -p iamb-service --release`, check 4 of ADR-0008: no feature that
  another workspace member, such as `iamb-testkit` with `probe`, turns on in a shared
  dependency is unified into it) and runs clippy on that build alone. The next step reads the
  binary's imports with the toolchain's `llvm-nm` / `llvm-objdump` on every OS, prints the
  libraries it links, and fails on a name-resolution or HTTP function or library
  (`getaddrinfo`, `GetAddrInfo*`, `DnsQuery`, WinHTTP, WinINet, CFNetwork, Network.framework).
  `ws2_32.dll` alone is not an error on Windows: the standard library may link it without the
  service calling it. The Windows and macOS legs then run clippy with `-D warnings` for
  the whole workspace, so code that exists only on one OS is linted, with its tests, on that
  OS; the step runs even when a test failed. The Linux leg skips it because `lint` already
  does it.
  The `probe` feature (the probe window of the latency measurement, off by default) has steps
  of its own: `lint` runs clippy on `iamb-testkit` with it, on the two OS crates for their
  targets with it, and on `iamb-testkit --no-default-features --features probe --lib --bins`
  for both targets; each `test` leg runs `cargo test -p iamb-testkit --features probe --test
  probe_browser`, and the Windows and macOS legs also the OS crate's `--features probe --test
  probe` (Windows with `IAMB_REQUIRE_DESKTOP`).
  The `panel` feature of `iamb-platform-macos` (the macOS non-activating panel, enabled only
  by `iamb-desktop`) likewise: `lint` runs clippy on that crate with it for the macOS target,
  and the macOS `test` leg runs `cargo test -p iamb-platform-macos --features panel --test
  panel`. The service is built without it, so it does not change the service binary.
  On macOS the focus test (`cargo test -p iamb-platform-macos --test focus`) fails, rather
  than skips, when its key-stamp check cannot run for a reason other than a missing
  Accessibility or Input Monitoring grant or secure keyboard entry left on; on a machine that
  has the grants (the lab, a runner set up with them), `IAMB_REQUIRE_KEY_STAMP=1` makes those
  skips failures too (the close-out review's C5). The hosted runner has no grants and does not
  set it.
- `desktop` (Linux, Windows, macOS, not fail-fast): the UI host. The front end in
  `apps/desktop` (`pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test` with vitest,
  `pnpm build`), then `cargo build`, `cargo clippy -- -D warnings` and `cargo test` for
  `-p iamb-desktop --all-targets --features tauri/custom-protocol` (the feature embeds the built
  front end, as a release build does; using it for every step builds the crate once). The Linux
  leg first installs `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`,
  `libxdo-dev` and `xvfb`. Last, the cold-open measurement of the library window
  (`iamb-desktop --measure-open 10`, `docs/test-strategy.md` §8.1): on Linux under Xvfb it fails
  the job when an open takes more than 800 ms (twice the lab target of 400 ms, the CI rule of test strategy §2.2; one open on a shared runner was seen at 413 ms while the others took 41-66 ms); on Windows and macOS it only prints the numbers
  (the step may fail without failing the job). Then the fill-in form measurement (`iamb-desktop --measure-form 10`,
  `docs/test-strategy.md` §8.5), from the request reaching the UI host to the panel's first
  paint: gated at 300 ms on Linux under Xvfb (twice the 150 ms target), report only elsewhere. The tests include the crash-isolation tests,
  which run a worker on the mock platform in the test process (and, on Linux, kill a UI host
  child process).
- `coverage` (Linux): `cargo llvm-cov` on `iamb-core`; fails below 85% line coverage.
- `ci-ok`: succeeds only if `lint`, `coverage`, every `test` leg and every `desktop` leg
  succeeded. This is the single check to require in branch protection.

`.github/workflows/bench.yml` runs the four benchmarks in quick mode and uploads the results. On
pushes to `main` and manual runs it runs them once, report only. On a pull request it runs them
twice and then `cargo xtask bench-compare`, which fails the `bench` job when the same bench
regresses against `crates/iamb-bench/baselines/ci-ubuntu.jsonl` in both runs, and passes ("not
armed yet") while that file does not exist (see `docs/test-strategy.md` §3, "Regression rule").
After them, on every event, it runs the search bench in quick mode with `IAMB_SEARCH_BUDGET_MS=60`
and `iamb-service`'s `search_timing` test in a release build, each failing the job when the
palette's search takes more than 60 ms at p95 (twice the 30 ms target, `docs/test-strategy.md`
§8.7); the search results are kept apart from the compared runs.
`bench` is not part of `ci-ok`; require it in branch protection as well for a regression to block
a merge.

`.github/workflows/nightly.yml` runs every night and on demand, and never blocks a merge:

- `fuzz-template`: 10 minutes of `cargo fuzz` on the template engine with a pinned nightly
  toolchain, a 1-second limit per input and 512 MB of memory; the corpus is kept between nights.
- `mutants-matcher`: the matcher mutation sample; fails when fewer than 80% of viable mutants
  are caught.
- `release-limits`: `cargo test --release` for `iamb-core` and `iamb-store`, which turns on the
  tight timing limits (load KPI under 100 ms, worst-case template budgets) that debug builds
  relax.

## Running the checks locally

On Linux the workspace-wide commands below include `iamb-desktop`, which needs the packages the
`desktop` job installs (see `apps/desktop/README.md`); add `--exclude iamb-desktop` to skip it.

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo build --workspace --all-targets --locked
    cargo build -p iamb-service --release --locked
    cargo test --workspace --locked
    cargo xtask banned-terms
    python3 spec/template/reference.py
    cargo llvm-cov -p iamb-core --locked --summary-only --fail-under-lines 85

The `desktop` job's steps:

    (cd apps/desktop && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test && pnpm build)
    cargo clippy -p iamb-desktop --all-targets --locked --features tauri/custom-protocol -- -D warnings
    cargo test -p iamb-desktop --locked --features tauri/custom-protocol
    xvfb-run -a target/debug/iamb-desktop --data-dir "$(mktemp -d)" --measure-open 10 --budget-ms 800

`cargo xtask` is an alias (see `.cargo/config.toml`) for the `xtask` workspace crate.

## Checking Windows and macOS code on Linux

`iamb-platform-windows` and `iamb-platform-macos` are empty on every OS but their own
(their modules and their OS dependencies are behind `cfg(windows)` and
`cfg(target_os = "macos")`), so the ordinary commands above do not look at their code on a
Linux machine. Rust can still type-check and lint them for their own targets without a Windows
or macOS SDK, because checking stops before linking. Install the two targets once:

    rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin

Then:

    cargo check -p iamb-platform-windows --target x86_64-pc-windows-msvc
    cargo clippy -p iamb-platform-windows --target x86_64-pc-windows-msvc --all-targets --locked -- -D warnings
    cargo check -p iamb-platform-macos --target aarch64-apple-darwin
    cargo clippy -p iamb-platform-macos --target aarch64-apple-darwin --all-targets --locked -- -D warnings

`--all-targets` includes the crates' tests, which are type-checked and linted but not run. The
`lint` job runs the two clippy commands on every pull request.

What this does not cover: nothing is linked or run, so a wrong function signature in a
hand-written `extern` declaration, a missing framework or a failing test only shows on the real
OS, in the Windows and macOS legs of `test`. `cargo build --target ...` and `cargo test
--target ...` do not work from Linux. A dependency whose build script needs the OS SDK would
break these commands; the OS crates' dependencies are chosen so that none does
(`crates/iamb-platform/tests/lints.rs` also checks that they stay behind their `cfg`, that the
two crates keep the workspace lints apart from allowing documented `unsafe`, and that no
networking namespace of the Windows bindings is enabled).

## No network code in the service

Architecture §2 and ADR-0008 name four checks; CI runs all of them.

1. No networking crate in the service's resolved dependency tree:
   `crates/iamb-service/tests/no_network.rs` walks `Cargo.lock` from `iamb-service` and
   compares every package with a deny list.
2. No networking namespace of the Windows bindings: `crates/iamb-platform/tests/lints.rs`.
3. No `std::net` (but `Shutdown`, which Unix sockets take) and no socket or name-resolution
   call of `libc` in the six crates `iamb-service`, `iamb-ipc`, `iamb-store`, `iamb-platform`,
   `iamb-platform-windows` and `iamb-platform-macos`: clippy's `disallowed-types` and
   `disallowed-methods`, configured in a `clippy.toml` in each of them, so every clippy run
   with `-D warnings` above enforces it. The lint resolves paths, so a nested import
   (`use std::{io, net::TcpStream};`) is caught as well as a qualified one. The six files are
   one configuration copied (clippy uses the nearest `clippy.toml` and does not merge them);
   `no_network.rs` fails when one is missing, when they differ, or when an entry is missing.
   `iamb-testkit` is not one of the six: the probe's browser page needs a local TCP socket.
4. The release binary built with `-p iamb-service` alone: the step "Build the service alone
   (release)" of `test`, on every OS.

The lint of check 3 can be switched off where it is used, and it does not see a function
declared by hand (the follow-up review's N6). `no_network.rs` therefore also searches every
source file of the six crates and fails on an `allow` or `expect` attribute (also inside
`cfg_attr`) that names `clippy::disallowed_types`, `clippy::disallowed_methods`, `clippy::style`
or `clippy::all`, on an `extern` block that declares a socket, connection or name-resolution
function (or names one with `link_name`), and on a `#[link]` attribute or `link!` macro that
names a networking library (`ws2_32`, `dnsapi`, `winhttp`, `wininet`, `CFNetwork`, `Network`,
…). The import check of the built binary runs on all three systems (above).

## Banned terms

The repository must not mention the names of other products. The list is not stored as text:
`xtask/banned-terms.sha256` holds SHA-256 digests of normalised terms (lowercase, split on any
non-alphanumeric character, joined by single spaces). The check scans every tracked and
untracked-but-not-ignored file, plus file paths, skipping binary files and `Cargo.lock`. It
hashes every window of one to three consecutive tokens, even across line breaks, and for
two- and three-token windows also the tokens joined with no separator, so "foo-bar" is caught
when "foobar" is listed (windows containing a one-letter word are not joined, so ordinary
prose does not collide with short names). The generic phrases "text expander", "text expanders" and "text
expansion" are allowed; they are how we describe our own category.

A hit prints `path:line:col: banned term (digest abcd1234...)` and exits with status 1. The
matched text is never printed.

To add a term without it entering the repository, pass it on the command line only (not in a
script or file that is committed):

    cargo xtask banned-terms add "some product name"

This appends the digest to the list, keeping it sorted. Commit the changed digest file.
`cargo xtask banned-terms hash "..."` prints a digest without changing anything.

## Branch protection

The repository owner must set this in GitHub: Settings, Branches, add a rule for `main`, enable
"Require status checks to pass before merging" and select the `ci-ok` check (it appears after
the workflow has run once). Also enable "Require branches to be up to date" if desired. Without
this rule a failing pull request can still be merged; workflow files alone cannot enforce it.
