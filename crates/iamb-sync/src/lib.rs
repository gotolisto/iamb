//! The sync agent (task 4.4): push, pull, retry with backoff and live updates between the local
//! library and the Iamb server (`docs/sync-design.md`, ADR-0027 to ADR-0029).
//!
//! The agent is its own process, started by the supervisor (architecture §3). It has the network
//! and no store; the service has the store and no network (ADR-0008). Everything it learns from
//! the server it hands to the service over the local endpoint with the role `sync`, and the store
//! applies the rules of §9 of the design.
//!
//! | Module | What |
//! | --- | --- |
//! | [`api`] | The call layer: every function of the server's `api` schema, its arguments and its answer, over a [`api::Transport`] |
//! | [`agent`] | The sync loop of §6.7: `hello`, pull, push, pull, resync (D7), the access check, D17 |
//! | [`schedule`] | When to act: backoff (§6.8), polling and reconnecting (§6.6), the debounce |
//! | [`device`] | What the agent asks of the service (§7.2), and [`device::IpcDevice`] |
//! | [`http`] | The transport over HTTPS to the REST interface (`ureq`, `rustls`) |
//! | [`realtime`] | The live channel over the realtime WebSocket (`tungstenite`, `rustls`) |
//! | [`auth`] | Tokens: the platform's secret store behind [`auth::TokenStore`], and their refresh |
//! | `model` | A model of the server's rules (feature `model`), for the scenario replay and the two-device simulation |
//!
//! Nothing here logs a token or a row's content: logs carry message names, counts, durations and
//! codes (architecture §6.1, §11.1 of the design).

pub mod agent;
pub mod api;
pub mod auth;
pub mod device;
pub mod http;
#[cfg(feature = "model")]
pub mod model;
pub mod realtime;
pub mod schedule;

pub use agent::{Agent, AgentConfig, RoundError, RoundReport, Stop};
pub use api::{CallError, Failure, Refusal, Transport};
pub use device::{Device, DeviceError, IpcDevice};

/// A UUID version 7 from the system clock and the system's random source.
#[must_use]
pub fn new_v7() -> iamb_core::model::Id {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    let mut random = [0u8; 10];
    if getrandom::fill(&mut random).is_err() {
        // Without the system's random source, the time and the process id still make a new id.
        random[..4].copy_from_slice(&std::process::id().to_le_bytes());
    }
    iamb_core::model::Id::v7(ms, random)
}
