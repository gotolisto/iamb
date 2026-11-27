//! Iamb's local messaging (architecture §6, ADR-0006).
//!
//! - [`messages`]: the message types of every group of the service endpoint (§6.2):
//!   [`RequestBody`], [`ResponseBody`], [`EventBody`] and their wire data types.
//! - [`protocol`]: the frame envelope ([`Wire`]), the handshake ([`Hello`], [`Welcome`]), the
//!   [`Role`]s and which [`Group`]s each may call, and the error codes.
//! - [`frame`]: a 4-byte little-endian length and one JSON document of at most 8 MB.
//! - [`Client`] and [`ServerConnection`]: blocking helpers over any
//!   [`LocalStream`](iamb_platform::LocalStream).
//! - [`native_endpoint`]: the [`LocalEndpoint`](iamb_platform::LocalEndpoint) of this OS: the
//!   named pipe of `iamb-platform-windows`, the Unix socket of `iamb-platform-macos`, or on
//!   Linux the std-only socket in [`linux`] (a development platform; see its docs for what it
//!   does not verify).
//!
//! # Rules
//!
//! - Blocking only; no async runtime, no networking crate and no `std::net` apart from the
//!   `Shutdown` enum that Unix sockets take (ADR-0008, ADR-0009).
//! - Message bodies are never logged: the helpers report each frame's name, size and duration
//!   to a [`MessageLog`]. `Debug` of a message prints its name only, and user-entered values
//!   are [`Secret`]s.

pub mod client;
pub mod error;
pub mod frame;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod log;
pub mod messages;
pub mod protocol;
pub mod roles;
pub mod secret;
pub mod server;

pub use client::{Client, DEFAULT_CALL_TIMEOUT};
pub use error::IpcError;
pub use frame::{FrameError, MAX_FRAME_LEN, read_frame, write_frame};
pub use log::{Direction, MessageLog, MessageRecord, NoLog};
pub use messages::{
    AttachmentRef, ChangedRows, ConflictPolicy, DiagnosticDetail, EventBody, ExcludedEntry,
    FocusRef, GrantState, HealthReport, InjectionStatus, PanelKind, PendingReview, PermissionName,
    PreviewBlank, PreviewCursor, PreviewPart, RequestBody, ResponseBody, SUMMARY_LINE_CHARS,
    SearchField, SearchHit, ServiceStatus, ShadowedEntry, SnippetQuery, SnippetSummary, SyncStatus,
    TAG_COLORS, TagEntry, TemplateAttribute, TemplateErrorInfo, TemplateToken, TransferFormat,
    UnusedEntry, WireAction, WireAttachment, WireDiagnostic, WireExclusion, WireField, WireValues,
    key_named,
};
pub use protocol::{
    DecodeError, ErrorBody, ErrorCode, Group, Hello, PROTOCOL_VERSION, Request, Response, Role,
    Welcome, Wire,
};
pub use roles::{CLIENT_BINARIES, RoleRefusal, check_role, role_of_binary};
pub use secret::Secret;
pub use server::{ConnectionWriter, HELLO_TIMEOUT, ServerConnection};

/// How long one `PanelFocus { shown: true }` holds without being sent again
/// ([`RequestBody::PanelFocus`]).
pub const PANEL_FOCUS_LEASE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long an acknowledged fill-in form stays open without another `FormAck`
/// ([`RequestBody::FormAck`]).
pub const FORM_LEASE: std::time::Duration = std::time::Duration::from_mins(1);

/// The build version, sent in `hello` and `welcome`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The local endpoint of this operating system (see the [crate docs](self)).
#[must_use]
pub fn native_endpoint() -> std::sync::Arc<dyn iamb_platform::LocalEndpoint> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(iamb_platform_windows::Platform::new())
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(iamb_platform_macos::Platform::new())
    }
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(linux::LinuxEndpoint::new())
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        std::sync::Arc::new(Unsupported)
    }
}

/// Reads this process's own signing state now, for the installed-binary check of the native
/// endpoint, and keeps it for the life of the process; returns the line for the log. Call it
/// first in `main` (`iamb_platform::installation`, "This process's own signing state"; the
/// strict review of finding 11, finding 1).
#[must_use]
pub fn read_own_signing() -> String {
    #[cfg(windows)]
    {
        iamb_platform_windows::read_own_signing()
    }
    #[cfg(target_os = "macos")]
    {
        iamb_platform_macos::read_own_signing()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "no installed-binary check on this system".to_owned()
    }
}

/// The endpoint of an OS Iamb does not support: every call is `Unsupported`.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
struct Unsupported;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
impl iamb_platform::LocalEndpoint for Unsupported {
    fn bind(
        &self,
        _name: &iamb_platform::EndpointName,
        _policy: iamb_platform::PeerPolicy,
    ) -> iamb_platform::Result<Box<dyn iamb_platform::LocalListener>> {
        Err(iamb_platform::PlatformError::Unsupported)
    }

    fn connect(
        &self,
        _name: &iamb_platform::EndpointName,
        _policy: iamb_platform::PeerPolicy,
        _timeout: std::time::Duration,
    ) -> iamb_platform::Result<Box<dyn iamb_platform::LocalStream>> {
        Err(iamb_platform::PlatformError::Unsupported)
    }
}
