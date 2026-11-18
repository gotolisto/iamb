//! Permissions: Accessibility and Input Monitoring (architecture §5, ADR-0021).
//!
//! # Whose permissions
//!
//! Every answer here is about **the process that asks**. macOS grants Accessibility and Input
//! Monitoring to one signed program, and `AXIsProcessTrusted` and
//! `CGPreflightListenEventAccess` report on the caller. The grants that matter are those of the
//! worker, the signed helper app that creates the event tap (architecture §3.2, ADR-0021), so
//! the worker asks and reports over IPC; the UI host never asks about itself. Nothing in this
//! module looks at another process.
//!
//! # What the calls do
//!
//! | Call | Accessibility | Input Monitoring | Shows a prompt |
//! | --- | --- | --- | --- |
//! | `permission_status` | `AXIsProcessTrusted` | `CGPreflightListenEventAccess` | never |
//! | `open_settings(kind)`, permission missing | `AXIsProcessTrustedWithOptions` with the prompt option | `CGRequestListenEventAccess` | the system may, once |
//! | `open_settings(kind)`, then | opens the Accessibility page | opens the Input Monitoring page | no |
//!
//! Requesting is never a side effect of a status check: the rules are in
//! [`policy`], which is plain Rust and tested on every system. The listener uses the same
//! checks before it creates its tap, because creating a tap without the permission would make
//! the system show its prompt uninvited.
//!
//! Nothing is cached: a grant given while the worker runs is seen by the next call.
//!
//! # Limits
//!
//! - macOS does not say whether a missing permission was refused or never asked for; both are
//!   [`Grant::Denied`](iamb_platform::Grant).
//! - macOS applies a change of Input Monitoring to a running process only in part: a tap
//!   created before the change keeps its old rights until the process restarts. The worker
//!   restarts its listener when the status changes, and the supervisor restarts the worker
//!   when the self-test keeps failing (architecture §3.2).

pub(crate) mod policy;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub(crate) use mac::{State, SystemProbe};
