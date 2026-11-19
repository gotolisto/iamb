# Continuous integration

CI is defined in `.github/workflows/ci.yml` and runs on every pull request, on pushes to `main`
and on manual dispatch. The toolchain version comes from `rust-toolchain.toml`; warnings are
errors (`RUSTFLAGS=-D warnings`). The pipeline should finish in under ten minutes.

## Jobs

- `lint` (Linux): formatting, clippy with `-D warnings`, clippy of the two OS crates for their
  own targets (`iamb-platform-windows` for `x86_64-pc-windows-msvc`, `iamb-platform-macos` for
  `aarch64-apple-darwin`; see "Checking Windows and macOS code on Linux" below), the banned-term
  check, and the template spec examples checked against the reference implementation.
- `test` (Linux, Windows, macOS, not fail-fast): `cargo build` and `cargo test` for the whole
  workspace with `--locked`. The Windows and macOS legs then run clippy with `-D warnings` for
  the whole workspace, so code that exists only on one OS is linted, with its tests, on that
  OS; the step runs even when a test failed. The Linux leg skips it because `lint` already
  does it.
  The `probe` feature (the probe window of the latency measurement, off by default) has steps
  of its own: `lint` runs clippy on `iamb-testkit` with it, on the two OS crates for their
  targets with it, and on `iamb-testkit --no-default-features --features probe --lib --bins`
  for both targets; each `test` leg runs `cargo test -p iamb-testkit --features probe --test
  probe_browser`, and the Windows and macOS legs also the OS crate's `--features probe --test
  probe` (Windows with `IAMB_REQUIRE_DESKTOP`).
- `coverage` (Linux): `cargo llvm-cov` on `iamb-core`; fails below 85% line coverage.
- `ci-ok`: succeeds only if `lint`, `coverage` and every `test` leg succeeded. This is the single
  check to require in branch protection.

`.github/workflows/bench.yml` runs the four benchmarks in quick mode and uploads the results. On
pushes to `main` and manual runs it runs them once, report only. On a pull request it runs them
twice and then `cargo xtask bench-compare`, which fails the `bench` job when the same bench
regresses against `crates/iamb-bench/baselines/ci-ubuntu.jsonl` in both runs, and passes ("not
armed yet") while that file does not exist (see `docs/test-strategy.md` §3, "Regression rule").
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

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo build --workspace --all-targets --locked
    cargo test --workspace --locked
    cargo xtask banned-terms
    python3 spec/template/reference.py
    cargo llvm-cov -p iamb-core --locked --summary-only --fail-under-lines 85

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
