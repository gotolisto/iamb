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
//! | [`setup`] | Imports the fixture library (and, for the latency run, its 1,000 snippets) into a data directory |
//! | [`probe`] | The probe window of the end-to-end latency measurement (`iamb-probe`) |
//! | [`latency`] | `iamb-compat latency`: drives the probe and writes the `e2e-plain-p95` result |
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
pub mod latency;
pub mod probe;
pub mod report;
pub mod results;
pub mod run;
pub mod setup;
pub mod types;
pub mod typist;
