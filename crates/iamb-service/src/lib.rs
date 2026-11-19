//! The Iamb background service (architecture §3, ADR-0001, ADR-0009, ADR-0011).
//!
//! One binary, two processes:
//!
//! - [`supervisor`] (`iamb-service --supervise`): the login item. Single instance, starts the
//!   worker, restarts it on abnormal exit with backoff and when its heartbeat stops.
//! - [`worker`] (`iamb-service --worker`): listener → engine → injector, the focus thread, the
//!   store thread and the service endpoint (§3.1).
//!
//! The library part exists so tests can run a worker in-process against the mock platform.
//!
//! # Rules
//!
//! - No network code of any kind (ADR-0008): no networking crate, no `std::net` (checked by
//!   `tests/no_network.rs`).
//! - No async runtime (ADR-0009).
//! - Typed characters, form values, clipboard text and IPC bodies never reach the log
//!   ([`log`], §8).
//! - Each process opts out of the operating system's crash dumps first thing in `main`, and a
//!   panic writes a heap-free local report and exits instead of aborting ([`crash`], §8
//!   rule 9).

pub mod clock;
pub mod crash;
pub mod library;
pub mod log;
pub mod paths;
pub mod platform;
pub mod supervisor;
pub mod templates;
pub mod worker;

/// The build version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The exit status of a worker that wants to be restarted (its listener is dead).
pub const EXIT_RESTART: u8 = 75;
