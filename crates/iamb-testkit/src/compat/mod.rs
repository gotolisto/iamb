//! The compatibility harness (task 2.7): walks the owner of a real Windows or macOS machine
//! through the matrix of `docs/compatibility-matrix.md` and keeps the results.
//!
//! | Module | Does |
//! | --- | --- |
//! | [`apps`] | The 20 apps per OS (`fixtures/compat-apps.toml`) |
//! | [`types`] | The five snippet types and their exact expected text (`fixtures/compat-expected.toml`) |
//! | [`results`] | The append-only JSON-lines results file |
//! | [`report`] | The Markdown results table and the pass rate |
//! | [`typist`] | Typing and copying through the operating system, without Iamb's marker |
//! | [`run`] | The interactive walk: automatic and manual cells |
//! | [`setup`] | Imports the fixture library into a data directory |
//! | [`cli`] | The `iamb-compat` command line |
//!
//! Everything here is test tooling: it is never shipped and never part of the service.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    reason = "test tooling: errors are readable strings for the person running the harness"
)]

pub mod apps;
pub mod cli;
pub mod report;
pub mod results;
pub mod run;
pub mod setup;
pub mod types;
pub mod typist;
