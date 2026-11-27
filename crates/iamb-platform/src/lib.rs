//! What Iamb needs from an operating system (architecture §5, ADR-0005).
//!
//! This crate is pure Rust: traits, the types they exchange, and an in-memory [`mock`]. It
//! makes no OS calls and contains no `unsafe`. `iamb-platform-windows` and
//! `iamb-platform-macos` implement the traits; the service is written against the traits and
//! tested against the mock on every OS.
//!
//! | Trait | For | Worker thread (§3.1) |
//! | --- | --- | --- |
//! | [`KeyListener`] | observing keys; delivers [`ListenerEvent`]s to an [`EventSink`] | its own OS loop thread |
//! | [`Hotkeys`] | global shortcuts, delivered through the listener | the listener's thread |
//! | [`Injector`] | sending one expansion, with the markers and the typed-characters hand-off of §4.6 | injector |
//! | [`FocusInspector`] | focused application, secure fields, focus tokens | focus (and the UI host) |
//! | [`ClipboardAccess`] | `{clipboard}` and paste with restore | engine (read), injector (paste) |
//! | [`Permissions`] | the macOS grants | readers (`GetPermissions`) |
//! | [`LoginItem`] | start at login | the UI host |
//! | [`MemoryLock`] | keeping typed characters out of swap | worker start-up |
//! | [`SessionEvents`] | screen lock, session switch, sign-out ([`session`]) | its own OS thread |
//! | [`LocalEndpoint`] | the socket or pipe under `iamb-ipc` | readers |
//!
//! [`Platform`] is all of them together. Every public item is re-exported at the crate root;
//! the modules are public for their documentation, which holds the reasoning: [`listener`]
//! (why the listener has its own event type), [`inject`] (why steps and not engine actions,
//! and the injector's protocol), [`injection_settings`] (the one format of the per-app
//! injection settings, ADR-0025), [`handoff`] (who writes and who reads the characters typed
//! after a trigger), [`marker`] (the marker encoding), [`focus`], [`endpoint`], [`mock`].
//!
//! # Design rules
//!
//! - **Object safe, `&self`, `Send + Sync`.** Every trait can be used as `Arc<dyn Trait>`: the
//!   service picks the implementation at run time (the OS crate, or the mock in tests) and
//!   shares it between its threads. Nothing here is generic over the platform, so the service
//!   compiles once and its tests run the same code paths as production.
//! - **No async** (ADR-0009). Calls block; the two calls that can wait on another thread
//!   ([`TypedHandoff::wait_sealed`], [`FocusInspector::secure_state`]) take a timeout.
//! - **Typed text stays put** (§8). Types that carry typed characters or clipboard content
//!   have a `Debug` that shows sizes only; errors carry no user data.
//! - **Shared decisions live here.** Marker encoding ([`Marker`]), named-key classification
//!   ([`ListenerEvent::named_key`]), the overflow rule ([`OverflowSink`]), compensation for
//!   keys typed after a trigger ([`compensate`]), the focus verdict ([`verdict`]), the format
//!   of the injection settings ([`InjectionSettings::parse`]) and peer verification
//!   ([`PeerPolicy::check`]) are pure functions in this crate, so both OS crates and the mock
//!   behave alike.

pub mod clipboard;
pub mod endpoint;
pub mod error;
pub mod focus;
pub mod handoff;
pub mod hotkeys;
pub mod inject;
pub mod injection_settings;
pub mod installation;
pub mod listener;
pub mod login_item;
pub mod marker;
pub mod memory;
pub mod mock;
pub mod permissions;
pub mod platform;
pub mod session;

pub use clipboard::{
    ClipboardAccess, ClipboardContent, ClipboardFlavour, ClipboardFormat, ClipboardSnapshot,
};
pub use endpoint::{
    EndpointKind, EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity,
    PeerPolicy, PeerRejection,
};
pub use error::{PlatformError, Result};
pub use focus::{
    AppIdentity, FOCUS_READ_ATTEMPTS, FieldSecurity, FocusChange, FocusInspector, FocusSink,
    FocusToken, RESTORE_FOCUS_TIMEOUT, SECURE_STATE_TIMEOUT, UnknownReason, verdict,
};
pub use handoff::{
    CancelReason, HANDOFF_CAPACITY, HandoffStatus, START_MARKER_TIMEOUT, TypedHandoff,
};
pub use hotkeys::{Chord, ChordError, HotkeyId, HotkeyKey, Hotkeys, Modifiers};
pub use inject::{
    Compensated, ExpansionBatch, FocusGuard, InjectionOutcome, InjectionOverrides,
    InjectionRequest, InjectionStep, Injector, TextMethod, compensate, split_actions,
};
pub use injection_settings::{
    INJECTION_SETTINGS_KEY, InjectionEntry, InjectionMethod, InjectionSettings, LineBreakChoice,
    MAX_APPS, PasteShortcut, SettingsBounds, SettingsDialect, SettingsError,
};
pub use listener::{
    ChannelSink, DEFAULT_EVENT_CAPACITY, EventSink, KeyListener, KeyTarget, ListenerEvent,
    OverflowSink, Routed, SeqSource, event_channel,
};
pub use login_item::{LoginItem, LoginItemStatus};
pub use marker::{Marker, PAYLOAD_BITS};
pub use memory::{MemoryLock, MemoryRegion};
pub use permissions::{Grant, PermissionKind, PermissionStatus, Permissions};
pub use platform::Platform;
pub use session::{SessionEvent, SessionEvents, SessionSink};

/// The engine types the platform layer exchanges with the service, re-exported so an OS crate
/// needs no direct dependency on `iamb-core`.
pub mod core_types {
    pub use iamb_core::engine::{AppId, BoundaryKind, ExpansionId, SecureState};
    pub use iamb_core::sensitive::Sensitive;
    pub use iamb_core::template::KeyName;
}
