# Iamb

Iamb is a free, open-source text expander for desktop. Type a short abbreviation and get the full snippet back. The core is written in Rust; a Tauri-based UI will follow.

Status: pre-alpha, nothing usable yet.

## Build

```sh
cargo build --workspace
```

## Test

```sh
cargo test --workspace
```

## Repository layout

- `crates/iamb-core`: pure engine (data model, matcher, template language, date math). No I/O, no OS code.
- `crates/iamb-store`: local SQLite store, import and export.
- `crates/iamb-cli`: the `iamb` command-line binary.
- `docs/`: documentation.

## Contributing

Contribution guidelines are coming soon.

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE).
