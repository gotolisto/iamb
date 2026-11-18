# Continuous integration

CI is defined in `.github/workflows/ci.yml` and runs on every pull request, on pushes to `main`
and on manual dispatch. The toolchain version comes from `rust-toolchain.toml`; warnings are
errors (`RUSTFLAGS=-D warnings`). The pipeline should finish in under ten minutes.

## Jobs

- `lint` (Linux): formatting, clippy with `-D warnings`, the banned-term check, and the template
  spec examples checked against the reference implementation.
- `test` (Linux, Windows, macOS, not fail-fast): `cargo build` and `cargo test` for the whole
  workspace with `--locked`.
- `coverage` (Linux): `cargo llvm-cov` on `iamb-core`; fails below 85% line coverage.
- `ci-ok`: succeeds only if `lint`, `coverage` and every `test` leg succeeded. This is the single
  check to require in branch protection.

`.github/workflows/bench.yml` runs the benchmarks in quick mode on every pull request and uploads
the results. It is report-only and never blocks a merge (see `docs/test-strategy.md` §3).

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
