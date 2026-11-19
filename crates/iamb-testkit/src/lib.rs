//! Test-only helpers for Iamb (never shipped): a fake [`Environment`](iamb_core::engine::Environment)
//! and the scripted typing sessions of task 1.6.
//!
//! - [`env::FakeEnv`]: a fixed clock and an optional clipboard that count their calls.
//! - [`compat`]: the compatibility-matrix harness of task 2.7 (the `iamb-compat` binary); its
//!   fixtures are in `crates/iamb-testkit/fixtures/`. It also holds the probe of the end-to-end
//!   latency measurement ([`compat::probe`], the `iamb-probe` binary with feature `probe`, off
//!   by default) and its driver ([`compat::latency`], `iamb-compat latency`).
//! - [`session`]: the TOML session format, a loader and a runner. The sessions live in
//!   `crates/iamb-testkit/sessions/`; `crates/iamb-core/tests/engine_sessions.rs` runs them all.
//! - [`soak`] (feature `soak`, on by default): the long-run harness of task 2.6 and the canary
//!   privacy scan of task 2.5 (the `iamb-soak` binary).

pub mod compat;
pub mod env;
pub mod session;
#[cfg(feature = "soak")]
pub mod soak;
