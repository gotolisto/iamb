//! `iamb-desktop`, the UI host (architecture §3, §10; ADR-0017).
//!
//! A Tauri 2 shell around the React front end in `apps/desktop/src`. It starts hidden, stays
//! resident, keeps the tray or menu-bar icon, creates the library window on demand and keeps
//! the fill-in form and the search palette pre-warmed. It never contains engine or store logic
//! (architecture §2): everything the front end shows comes from the service through the
//! [`bridge`], the only part of the UI host that speaks IPC.
//!
//! - [`bridge`]: the connection to the service with the role `ui`, reconnecting with backoff.
//! - [`contract`]: what the front end sees (commands and events) and how it maps onto
//!   `iamb-ipc`'s messages.
//! - [`app`]: the Tauri application (tray, windows, commands).
//! - [`form`]: the fill-in form's flow (acknowledge, `PanelFocus`, show, hand the focus back,
//!   submit or cancel), independent of Tauri.
//! - [`palette`]: the search palette's flow (`PaletteRequested`, `PanelFocus`, show, pick, hand the
//!   focus back, `ExpandSnippet`), independent of Tauri; [`palette_data`]: its rows.
//! - [`notice`]: the notice for a pick or an insert the service refused, independent of Tauri.
//! - [`crash`], [`log`], [`paths`]: crash handling without heap contents, the log file, and the
//!   data root, as in the service (architecture §3.3, §8).
//! - [`link`]: which addresses a link in the formatted preview may open, and the opening.
//! - [`menu`]: what the tray or menu-bar menu and its icon show, independent of Tauri.
//! - [`timing`]: the cold-open measurement.

pub mod app;
pub mod bridge;
pub mod check;
pub mod contract;
pub mod crash;
pub mod form;
pub mod library;
pub mod link;
pub mod log;
pub mod menu;
pub mod notice;
pub mod onboarding;
pub mod palette;
pub mod palette_data;
pub mod paths;
pub mod settings;
pub mod template;
pub mod timing;

/// The build version, sent in the hello.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
