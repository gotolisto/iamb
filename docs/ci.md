# Continuous integration

CI is defined in `.github/workflows/ci.yml` and runs on every pull request, on pushes to `main`
and on manual dispatch. The toolchain version comes from `rust-toolchain.toml`; warnings are
errors (`RUSTFLAGS=-D warnings`). The pipeline should finish in under ten minutes.

## Jobs

- `lint` (Linux): formatting, clippy with `-D warnings`, the banned-term check, and the template
  spec examples checked against the reference implementation.
- `test` (Linux, Windows, macOS, not fail-fast): `cargo build` and `cargo test` for the whole
  workspace with `--locked`.
- `ci-ok`: succeeds only if `lint` and every `test` leg succeeded. This is the single check to
  require in branch protection.

## Running the checks locally

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo build --workspace --all-targets --locked
    cargo test --workspace --locked
    cargo xtask banned-terms
    python3 spec/template/reference.py

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
