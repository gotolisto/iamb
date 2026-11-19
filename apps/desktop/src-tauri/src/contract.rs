//! What the front end sees, and how it maps onto the service's messages.
//!
//! The front end never speaks IPC (architecture §6.2, ADR-0017). It calls the Tauri commands in
//! `app::commands` and listens to two Tauri events; this module holds their data types (mirrored
//! by hand in `apps/desktop/src/ipc.ts`) and the pure functions that turn them into
//! `iamb-ipc` requests and back:
//!
//! | Command | Request sent | Response read |
//! | --- | --- | --- |
//! | `status` | `GetStatus` | `Status` |
//! | `set_paused` | `SetPaused` | `Status` |
//! | `list_snippets` | `ListSnippets` | `Snippets` |
//! | `get_snippet` | `GetSnippet` | `Snippet` |
//!
//! | Service event | Front-end event ([`UiEvent`] on `iamb:event`, or [`StatusView`] on `iamb:status`) |
//! | --- | --- |
//! | `status_changed` | `iamb:status` |
//! | `library_changed` | `library_changed` |
//! | `form_requested` | `form_requested` (session, snippet, fields; the focus token stays in Rust, which hands focus back, and last values are not forwarded until the form exists, task 3.5) |
//! | `palette_requested` | `palette_requested` (the focus token stays in Rust) |
//! | `diagnostic_added` | `diagnostic_added` (code and snippet) |
//! | `settings_changed` | `settings_changed` |
//!
//! `outbox_changed` belongs to the sync group, which the `ui` role does not hear.
//!
//! `tests/contract.rs` runs these functions against `iamb-ipc`'s golden files, so a protocol
//! change that the bridge does not follow breaks a test.

use iamb_core::model::{Id, Snippet, SnippetKind, Trigger};
use iamb_ipc::{
    ErrorBody, EventBody, RequestBody, ResponseBody, ServiceStatus, SnippetQuery, WireDiagnostic,
    WireField,
};
use serde::{Deserialize, Serialize};

/// The Tauri commands the front end may call, by name: the list in `app/mod.rs`'s
/// `generate_handler!` and `COMMANDS` in `apps/desktop/src/ipc.ts` (`tests/front_end_contract.rs`
/// compares the three).
pub const COMMANDS: [&str; 7] = [
    "status",
    "refresh_status",
    "set_paused",
    "list_snippets",
    "get_snippet",
    "report_first_paint",
    "dismiss",
];

/// The requests the bridge sends, by wire name (the `type` of each).
pub const REQUESTS: [&str; 4] = ["get_status", "set_paused", "list_snippets", "get_snippet"];

/// The service events the bridge forwards, by wire name (the `topic` of each).
pub const EVENTS: [&str; 6] = [
    "library_changed",
    "status_changed",
    "palette_requested",
    "form_requested",
    "diagnostic_added",
    "settings_changed",
];

/// The Tauri event that carries a [`StatusView`].
pub const STATUS_EVENT: &str = "iamb:status";

/// The Tauri event that carries a [`UiEvent`].
pub const UI_EVENT: &str = "iamb:event";

/// Whether the bridge holds a connection to the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// Trying to connect (at start, and after a connection ended).
    Connecting,
    /// Connected and welcomed.
    Connected,
    /// The last attempt failed; the next one follows after a backoff.
    Disconnected,
}

impl ConnectionState {
    /// The state's name, for logs and the tray.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
        }
    }
}

/// The part of the service's status the UI shows.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags of a status record, as in iamb_ipc::ServiceStatus"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    /// The service's build version.
    pub version: String,
    /// Global pause is on.
    pub paused: bool,
    /// Typed keys are dropped right now (a secure field, an excluded app, pause, …).
    pub suspended: bool,
    /// Snippets in the current snapshot.
    pub snippets: u64,
    /// The library revision.
    pub revision: u64,
    /// The key listener is installed.
    pub listener: bool,
    /// The keystroke buffers are locked in memory.
    pub memory_locked: bool,
}

impl From<&ServiceStatus> for ServiceView {
    fn from(s: &ServiceStatus) -> Self {
        Self {
            version: s.version.clone(),
            paused: s.paused,
            suspended: s.suspended,
            snippets: s.snippets,
            revision: s.revision,
            listener: s.listener,
            memory_locked: s.memory_locked,
        }
    }
}

/// The connection and, while connected, the service's status: the `status` command's answer
/// and the payload of `iamb:status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    /// The connection.
    pub connection: ConnectionState,
    /// The service's status, while connected and once known.
    pub service: Option<ServiceView>,
}

impl StatusView {
    /// The view of `connection` with `service`'s status (dropped unless connected).
    #[must_use]
    pub fn new(connection: ConnectionState, service: Option<&ServiceStatus>) -> Self {
        Self {
            connection,
            service: match connection {
                ConnectionState::Connected => service.map(ServiceView::from),
                ConnectionState::Connecting | ConnectionState::Disconnected => None,
            },
        }
    }

    /// Pause is on, as far as the UI knows.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.service.as_ref().is_some_and(|s| s.paused)
    }
}

/// One snippet in a list. Library content, not typed text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetSummary {
    /// The snippet's id.
    pub id: String,
    /// Its folder's id.
    pub folder_id: String,
    /// What the user types.
    pub abbreviation: String,
    /// The label.
    pub label: String,
    /// `plain` or `rich`.
    pub kind: String,
    /// Whether it expands.
    pub enabled: bool,
}

impl From<&Snippet> for SnippetSummary {
    fn from(s: &Snippet) -> Self {
        Self {
            id: s.meta.id.to_string(),
            folder_id: s.folder_id.to_string(),
            abbreviation: s.abbreviation.as_str().to_owned(),
            label: s.label.clone(),
            kind: match s.kind {
                SnippetKind::Plain => "plain",
                SnippetKind::Rich => "rich",
            }
            .to_owned(),
            enabled: s.enabled,
        }
    }
}

/// One snippet with its template, for the editor (task 3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetDetail {
    /// The list fields.
    #[serde(flatten)]
    pub summary: SnippetSummary,
    /// The template source.
    pub body: String,
    /// `immediate` or `delimiter`.
    pub trigger: String,
    /// The library revision of the row.
    pub rev: u64,
}

impl From<&Snippet> for SnippetDetail {
    fn from(s: &Snippet) -> Self {
        Self {
            summary: SnippetSummary::from(s),
            body: s.body.clone(),
            trigger: match s.trigger {
                Trigger::Immediate => "immediate",
                Trigger::Delimiter => "delimiter",
            }
            .to_owned(),
            rev: s.meta.rev,
        }
    }
}

/// A fill-in field, as the form panel will show it (task 3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldView {
    /// The label, also the key of the values.
    pub name: String,
    /// `text`, `area`, `choice` or `toggle`.
    pub kind: String,
    /// The options of a `choice`.
    pub options: Vec<String>,
    /// The default value (from the template, library content).
    pub default: String,
}

impl From<&WireField> for FieldView {
    fn from(f: &WireField) -> Self {
        Self {
            name: f.name.clone(),
            kind: f.kind.clone(),
            options: f.options.clone(),
            default: f.default.clone(),
        }
    }
}

/// A service event, as the front end hears it on `iamb:event`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    /// Library content changed.
    LibraryChanged {
        /// The new library revision.
        revision: u64,
    },
    /// A fill-in form is wanted.
    FormRequested {
        /// The form session.
        session: u64,
        /// The snippet's id.
        snippet: String,
        /// Its fields, in order.
        fields: Vec<FieldView>,
    },
    /// The palette hotkey was pressed.
    PaletteRequested {},
    /// A diagnostic was recorded.
    DiagnosticAdded {
        /// Its reason code.
        code: String,
        /// The snippet it names, if any.
        snippet: Option<String>,
    },
    /// Settings or exclusions changed.
    SettingsChanged {},
}

/// What the bridge does with a service event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forward {
    /// A new status: emit [`STATUS_EVENT`].
    Status(ServiceStatus),
    /// Emit [`UI_EVENT`] with this.
    Ui(UiEvent),
    /// Not for the UI.
    Ignore,
}

/// How the bridge forwards `event` (module docs).
#[must_use]
pub fn forward(event: &EventBody) -> Forward {
    match event {
        EventBody::StatusChanged { status } => Forward::Status(status.clone()),
        EventBody::LibraryChanged { revision, .. } => Forward::Ui(UiEvent::LibraryChanged {
            revision: *revision,
        }),
        EventBody::FormRequested {
            session,
            snippet,
            spec,
            focus: _,
            last_values: _,
        } => Forward::Ui(UiEvent::FormRequested {
            session: *session,
            snippet: snippet.to_string(),
            fields: spec.iter().map(FieldView::from).collect(),
        }),
        EventBody::PaletteRequested { focus: _ } => Forward::Ui(UiEvent::PaletteRequested {}),
        EventBody::DiagnosticAdded { entry } => Forward::Ui(diagnostic(entry)),
        EventBody::SettingsChanged {} => Forward::Ui(UiEvent::SettingsChanged {}),
        EventBody::OutboxChanged { .. } => Forward::Ignore,
    }
}

fn diagnostic(entry: &WireDiagnostic) -> UiEvent {
    UiEvent::DiagnosticAdded {
        code: entry.code.clone(),
        snippet: entry.snippet.map(|id| id.to_string()),
    }
}

/// `GetStatus`.
#[must_use]
pub const fn get_status() -> RequestBody {
    RequestBody::GetStatus {}
}

/// `SetPaused`.
#[must_use]
pub const fn set_paused(paused: bool) -> RequestBody {
    RequestBody::SetPaused { paused }
}

/// `ListSnippets`, of one folder or of all.
#[must_use]
pub const fn list_snippets(folder: Option<Id>) -> RequestBody {
    RequestBody::ListSnippets {
        query: SnippetQuery { folder, tag: None },
    }
}

/// `GetSnippet`.
#[must_use]
pub const fn get_snippet(id: Id) -> RequestBody {
    RequestBody::GetSnippet { id }
}

/// A response of another type than the request is answered with: the service and the bridge
/// disagree about the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unexpected {
    /// The response's wire name.
    pub got: &'static str,
}

/// The status in a `Status` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn status_from(response: ResponseBody) -> Result<ServiceStatus, Unexpected> {
    match response {
        ResponseBody::Status { status } => Ok(status),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The snippets in a `Snippets` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn snippets_from(response: ResponseBody) -> Result<Vec<SnippetSummary>, Unexpected> {
    match response {
        ResponseBody::Snippets { snippets, .. } => {
            Ok(snippets.iter().map(SnippetSummary::from).collect())
        }
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The snippet in a `Snippet` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn snippet_from(response: ResponseBody) -> Result<SnippetDetail, Unexpected> {
    match response {
        ResponseBody::Snippet { snippet, .. } => Ok(SnippetDetail::from(&snippet)),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// An error as the front end receives it from a command: a stable code and a sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandError {
    /// `disconnected`, `timeout`, `invalid`, `unexpected_response`, or the service's error
    /// code (`not_found`, `unavailable`, …).
    pub code: String,
    /// For people. Written by the service or the bridge; never typed text or form values.
    pub message: String,
}

impl CommandError {
    /// An error with this code and message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<ErrorBody> for CommandError {
    fn from(e: ErrorBody) -> Self {
        Self::new(e.code.as_str(), e.message)
    }
}

impl From<Unexpected> for CommandError {
    fn from(e: Unexpected) -> Self {
        Self::new(
            "unexpected_response",
            format!("the service answered with {}", e.got),
        )
    }
}

/// Parses an id the front end passed.
///
/// # Errors
///
/// `invalid` when it is not an id.
pub fn parse_id(text: &str) -> Result<Id, CommandError> {
    text.parse()
        .map_err(|_| CommandError::new("invalid", "not a snippet or folder id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_part_is_shown_only_while_connected() {
        let status = ServiceStatus {
            version: "1".to_owned(),
            pid: 1,
            paused: true,
            suspended: false,
            injection: iamb_ipc::InjectionStatus::Idle,
            snippets: 2,
            revision: 3,
            listener: true,
            memory_locked: true,
            clients: 1,
            ui_connected: true,
        };
        assert!(StatusView::new(ConnectionState::Connected, Some(&status)).paused());
        assert_eq!(
            StatusView::new(ConnectionState::Disconnected, Some(&status)).service,
            None
        );
        assert!(!StatusView::new(ConnectionState::Connecting, None).paused());
    }

    #[test]
    fn ids_are_checked() {
        assert!(parse_id("01010101-0101-0101-0101-010101010101").is_ok());
        assert_eq!(
            parse_id("nope").map_err(|e| e.code),
            Err("invalid".to_owned())
        );
    }

    #[test]
    fn front_end_payloads_have_stable_shapes() {
        let view = StatusView::new(ConnectionState::Connecting, None);
        assert_eq!(
            serde_json::to_value(&view).expect("json"),
            serde_json::json!({"connection": "connecting", "service": null})
        );
        assert_eq!(
            serde_json::to_value(UiEvent::PaletteRequested {}).expect("json"),
            serde_json::json!({"type": "palette_requested"})
        );
    }
}
