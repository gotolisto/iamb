//! Test-only helpers for Iamb (never shipped): a fake [`Environment`](iamb_core::engine::Environment)
//! and the scripted typing sessions of task 1.6.
//!
//! - [`env::FakeEnv`]: a fixed clock and an optional clipboard that count their calls.
//! - [`session`]: the TOML session format, a loader and a runner. The sessions live in
//!   `crates/iamb-testkit/sessions/`; `crates/iamb-core/tests/engine_sessions.rs` runs them all.

pub mod env;
pub mod session;
