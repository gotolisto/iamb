//! The messages of the service endpoint, every group of architecture §6.2.
//!
//! [`RequestBody`] (tagged by `type`), [`ResponseBody`] (tagged by `type`) and [`EventBody`]
//! (tagged by `topic`, with its fields under `body`). Library rows travel as the model types of
//! `iamb-core` (`Snippet`, `Folder`, `Tag`, …), which are plain serde data; everything else has a
//! wire type here. `tests/golden/` holds one JSON file per message.
//!
//! # Privacy
//!
//! `Debug` of the three enums prints the message name only. Fields that hold what a user typed
//! or entered (form values, a dry run's typed text, a palette query, clipboard text, a preview's
//! values) are [`Secret`]s.
//!
//! # Names across the layers (§6.2)
//!
//! The engine action `ShowForm` becomes [`EventBody::FormRequested`]; the requests
//! [`RequestBody::FormSubmit`] and [`RequestBody::FormCancel`] become the engine events
//! `FormSubmitted` and `FormCancelled`. `SnapshotChanged` stays inside the worker; clients hear
//! [`EventBody::LibraryChanged`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use iamb_core::engine::{Action, Diagnostic};
use iamb_core::model::{
    Attachment, Folder, FolderShare, Id, Membership, Profile, Sha256, Snippet, SnippetAttachment,
    SnippetTag, SyncMeta, Tag, Timestamp,
};
use iamb_core::template::{FieldKind, FieldSpec, KeyName};
use iamb_platform::FocusToken;
use serde::{Deserialize, Serialize};

use crate::protocol::Group;
use crate::secret::Secret;

/// Field name → value of a fill-in form.
pub type WireValues = BTreeMap<String, String>;

// ---------------------------------------------------------------------------------------------
// Wire data types
// ---------------------------------------------------------------------------------------------

/// A focus token on the wire: the process id and the OS window number (§4.5). Plain data, not a
/// secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FocusRef {
    /// The process that owns the window.
    pub pid: u32,
    /// The OS window number.
    pub window: u64,
}

impl From<FocusToken> for FocusRef {
    fn from(token: FocusToken) -> Self {
        Self {
            pid: token.pid(),
            window: token.window(),
        }
    }
}

impl From<FocusRef> for FocusToken {
    fn from(focus: FocusRef) -> Self {
        Self::from_raw(focus.pid, focus.window)
    }
}

/// Which snippets [`RequestBody::ListSnippets`] returns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetQuery {
    /// Only snippets in this folder (not its subfolders); all folders when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Id>,
}

/// The file format of an import or export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferFormat {
    /// The lossless JSON library document.
    Json,
    /// One row per snippet.
    Csv,
}

/// What an import does with an abbreviation that is already in use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Keep the existing snippet and skip the new one.
    #[default]
    Skip,
    /// Stop and write nothing.
    Fail,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// The snippet.
    pub snippet: Id,
    /// Higher is better.
    pub score: u32,
}

/// An immediate abbreviation that hides a longer one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowedEntry {
    /// The immediate snippet.
    pub by: Id,
    /// The hidden snippet.
    pub hidden: Id,
}

/// A snippet that is not matchable for a reason other than being disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedEntry {
    /// The snippet.
    pub snippet: Id,
    /// `duplicate` or a template error code (`E_…`).
    pub reason: String,
    /// For `duplicate`: the snippet that holds the abbreviation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Id>,
}

/// Library health (task 3.10 adds conflicts, broken references and unused snippets).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    /// Immediate abbreviations that hide longer ones.
    pub shadowed: Vec<ShadowedEntry>,
    /// Duplicates and templates that do not parse.
    pub excluded: Vec<ExcludedEntry>,
}

/// A snippet revision written by someone else that waits for the user's approval (§8 rule 10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingReview {
    /// The snippet.
    pub snippet: Id,
    /// The revision to approve.
    pub rev: u64,
    /// The author's user id.
    pub author: Id,
}

/// Where the engine is in the injection protocol (§4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionStatus {
    /// No expansion in flight.
    Idle,
    /// An expansion waits for its start marker.
    Pending,
    /// Between the markers.
    Injecting,
}

/// What `GetStatus` reports.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags of a wire record, not a state machine"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// The service's build version.
    pub version: String,
    /// The worker's process id.
    pub pid: u32,
    /// Global pause is on.
    pub paused: bool,
    /// Typed keys are dropped unstored right now (focus not confirmed normal, composing, paused).
    pub suspended: bool,
    /// The injection protocol's state.
    pub injection: InjectionStatus,
    /// Snippets in the current snapshot.
    pub snippets: u64,
    /// The library revision ([`EventBody::LibraryChanged`]).
    pub revision: u64,
    /// The key listener is installed.
    pub listener: bool,
    /// The keystroke buffer and the typed-characters hand-off are locked in memory.
    pub memory_locked: bool,
    /// Connected IPC clients.
    pub clients: u32,
    /// A UI host is connected.
    pub ui_connected: bool,
}

/// A fill-in field on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireField {
    /// The label, also the key of the values.
    pub name: String,
    /// `text`, `area`, `choice` or `toggle`.
    pub kind: String,
    /// The options of a `choice`, in order; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The default value.
    pub default: String,
}

impl From<&FieldSpec> for WireField {
    fn from(spec: &FieldSpec) -> Self {
        Self {
            name: spec.name.clone(),
            kind: spec.kind.as_str().to_owned(),
            options: match &spec.kind {
                FieldKind::Choice(options) => options.clone(),
                _ => Vec::new(),
            },
            default: spec.default.clone(),
        }
    }
}

/// A diagnostic on the wire (§8 rule 4): a reason code and, only where the rule allows, the
/// snippet, the expansion and the application. Never typed text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDiagnostic {
    /// The reason code: an engine reason (`injection_cancelled`, `E_…`, …) or a service code
    /// (`form_no_ui_host`, `form_not_acknowledged`, `request_focus_timeout`,
    /// `keys_for_unreported_window`, …).
    pub code: String,
    /// The snippet, when a match or request named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Id>,
    /// The expansion, for injection diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<u64>,
    /// The application, for suspensions and refused requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// When it was recorded (UTC milliseconds), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

impl From<&Diagnostic> for WireDiagnostic {
    fn from(d: &Diagnostic) -> Self {
        Self {
            code: d.reason.code().to_owned(),
            snippet: d.snippet,
            expansion: d.expansion.map(iamb_core::engine::ExpansionId::raw),
            app: d.app.as_ref().map(|a| a.0.clone()),
            at: None,
        }
    }
}

impl WireDiagnostic {
    /// A diagnostic with only a code.
    pub fn code(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            snippet: None,
            expansion: None,
            app: None,
            at: None,
        }
    }
}

/// An engine action on the wire (the result of `DryRun`). Text it carries came from the library,
/// from the values the caller supplied, or from the clipboard text the caller supplied.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WireAction {
    /// Press Backspace `n` times.
    DeleteBack {
        /// How many.
        n: usize,
    },
    /// Insert plain text.
    TypeText {
        /// The text.
        text: String,
        /// Byte ranges `[start, end)` that came from field values or the clipboard.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        verbatim: Vec<[usize; 2]>,
    },
    /// Insert rich content through the clipboard.
    Paste {
        /// The plain-text version.
        plain: String,
        /// The HTML version.
        markdown_html: String,
        /// Attachments the HTML references.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Sha256>,
    },
    /// Press a named key (`enter`, `tab`, …).
    PressKey {
        /// The key's template name.
        key: String,
        /// Hold Shift.
        shift: bool,
        /// How many presses.
        times: u8,
    },
    /// Press Left `n` times.
    MoveCaretBack {
        /// How many.
        n: usize,
    },
    /// Pause.
    Wait {
        /// Milliseconds.
        ms: u32,
    },
    /// A fill-in form would be shown.
    ShowForm {
        /// The form session.
        session: u64,
        /// The snippet.
        snippet: Id,
        /// Its fields.
        fields: Vec<WireField>,
    },
    /// An expansion closed.
    Expanded {
        /// The expansion.
        expansion: u64,
        /// The snippet.
        snippet: Id,
        /// Characters saved.
        chars_saved: usize,
    },
    /// A diagnostic.
    Diagnostic {
        /// The entry.
        diagnostic: WireDiagnostic,
    },
}

impl fmt::Debug for WireAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeleteBack { n } => write!(f, "DeleteBack({n})"),
            Self::TypeText { text, .. } => write!(f, "TypeText(len {})", text.len()),
            Self::Paste { plain, .. } => write!(f, "Paste(len {})", plain.len()),
            Self::PressKey { key, shift, times } => {
                write!(f, "PressKey({key}, shift {shift}, x{times})")
            }
            Self::MoveCaretBack { n } => write!(f, "MoveCaretBack({n})"),
            Self::Wait { ms } => write!(f, "Wait({ms})"),
            Self::ShowForm { session, .. } => write!(f, "ShowForm({session})"),
            Self::Expanded { expansion, .. } => write!(f, "Expanded({expansion})"),
            Self::Diagnostic { diagnostic } => write!(f, "Diagnostic({})", diagnostic.code),
        }
    }
}

impl From<&Action> for WireAction {
    fn from(action: &Action) -> Self {
        match action {
            Action::DeleteBack(n) => Self::DeleteBack { n: *n },
            Action::TypeText { text, verbatim } => Self::TypeText {
                text: text.clone(),
                verbatim: verbatim.iter().map(|r| [r.start, r.end]).collect(),
            },
            Action::Paste {
                plain,
                markdown_html,
                attachments,
            } => Self::Paste {
                plain: plain.clone(),
                markdown_html: markdown_html.clone(),
                attachments: attachments.clone(),
            },
            Action::PressKey { key, shift, times } => Self::PressKey {
                key: key.as_str().to_owned(),
                shift: *shift,
                times: *times,
            },
            Action::MoveCaretBack(n) => Self::MoveCaretBack { n: *n },
            Action::Wait(ms) => Self::Wait { ms: *ms },
            Action::ShowForm {
                session,
                snippet,
                spec,
                ..
            } => Self::ShowForm {
                session: session.raw(),
                snippet: *snippet,
                fields: spec.iter().map(WireField::from).collect(),
            },
            Action::Expanded {
                expansion,
                snippet,
                chars_saved,
            } => Self::Expanded {
                expansion: expansion.raw(),
                snippet: *snippet,
                chars_saved: *chars_saved,
            },
            Action::Diagnostic(d) => Self::Diagnostic {
                diagnostic: WireDiagnostic::from(d),
            },
        }
    }
}

/// The named key with this template name.
#[must_use]
pub fn key_named(name: &str) -> Option<KeyName> {
    KeyName::ALL.into_iter().find(|k| k.as_str() == name)
}

/// The state of a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    /// Granted.
    Granted,
    /// Not granted.
    Denied,
    /// This OS has no such permission.
    NotRequired,
}

impl From<iamb_platform::Grant> for GrantState {
    fn from(g: iamb_platform::Grant) -> Self {
        match g {
            iamb_platform::Grant::Granted => Self::Granted,
            iamb_platform::Grant::Denied => Self::Denied,
            iamb_platform::Grant::NotRequired => Self::NotRequired,
        }
    }
}

/// A permission the worker needs (macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionName {
    /// Accessibility.
    Accessibility,
    /// Input Monitoring.
    InputMonitoring,
}

impl From<PermissionName> for iamb_platform::PermissionKind {
    fn from(p: PermissionName) -> Self {
        match p {
            PermissionName::Accessibility => Self::Accessibility,
            PermissionName::InputMonitoring => Self::InputMonitoring,
        }
    }
}

/// One token of a parsed template, for the editor's chips (task 3.4 fixes the kinds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateToken {
    /// `text`, or the token's name (`date`, `field`, `cursor`, …).
    pub kind: String,
    /// Byte offset of the start in the source.
    pub start: usize,
    /// Byte offset after the end.
    pub end: usize,
}

/// A template error: its code and byte position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateErrorInfo {
    /// The error code (`E_…`).
    pub code: String,
    /// Byte offset in the source, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
}

/// An app exclusion on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireExclusion {
    /// Bundle id (macOS) or executable name (Windows).
    pub app_id: String,
    /// The name shown to the user.
    pub display_name: String,
    /// When it was added.
    pub created_at: Timestamp,
}

/// A pending local change, for the sync agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireOutboxEntry {
    /// Position in the outbox.
    pub seq: i64,
    /// The table (`snippet`, `folder`, …).
    pub entity: String,
    /// The row.
    pub row_id: Id,
    /// `put` or `delete`.
    pub op: String,
    /// The server revision the change was based on.
    pub base_rev: u64,
    /// When the change was made.
    pub created_at: Timestamp,
}

/// A row from the server, for `ApplyRemote`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entity", content = "row", rename_all = "snake_case")]
pub enum RemoteRow {
    /// A profile.
    Profile(Profile),
    /// A membership.
    Membership(Membership),
    /// A folder.
    Folder(Folder),
    /// A folder share.
    FolderShare(FolderShare),
    /// A snippet.
    Snippet(Snippet),
    /// A tag.
    Tag(Tag),
    /// A snippet–tag link.
    SnippetTag(SnippetTag),
    /// Attachment metadata.
    Attachment(Attachment),
    /// A snippet–attachment link.
    SnippetAttachment(SnippetAttachment),
}

/// An attachment by profile and content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// The profile.
    pub profile: Id,
    /// The content hash.
    pub sha256: Sha256,
}

/// What the sync agent reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    /// An account is signed in.
    pub signed_in: bool,
    /// The last completed sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<Timestamp>,
    /// The last error code, if the last sync failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

/// What a client asks. Grouped as in §6.2; [`group`](Self::group) says which group a request
/// belongs to and so which roles may call it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestBody {
    // ---- Library ----------------------------------------------------------------------------
    /// The personal profile's folders. → [`ResponseBody::Folders`]
    ListFolders {},
    /// Snippets of the personal profile. → [`ResponseBody::Snippets`]
    ListSnippets {
        /// Which.
        #[serde(default)]
        query: SnippetQuery,
    },
    /// One snippet. → [`ResponseBody::Snippet`]
    GetSnippet {
        /// The snippet.
        id: Id,
    },
    /// Inserts or updates a snippet. → [`ResponseBody::Saved`]
    PutSnippet {
        /// The row.
        snippet: Snippet,
    },
    /// Soft-deletes a snippet. → [`ResponseBody::Done`]
    DeleteSnippet {
        /// The snippet.
        id: Id,
    },
    /// Inserts or updates a folder. → [`ResponseBody::Saved`]
    PutFolder {
        /// The row.
        folder: Folder,
    },
    /// Soft-deletes a folder. → [`ResponseBody::Done`]
    DeleteFolder {
        /// The folder.
        id: Id,
    },
    /// Inserts or updates a tag. → [`ResponseBody::Saved`]
    PutTag {
        /// The row.
        tag: Tag,
    },
    /// The palette search, over the snapshot. → [`ResponseBody::SearchResults`]
    Search {
        /// The query (user-entered).
        text: Secret<String>,
        /// At most this many hits.
        limit: u32,
    },
    /// Imports a file. → [`ResponseBody::Imported`]
    Import {
        /// The file to read (bulk data travels as a path, §6.1).
        path: PathBuf,
        /// Its format.
        format: TransferFormat,
        /// What to do with abbreviations in use.
        #[serde(default)]
        on_conflict: ConflictPolicy,
    },
    /// Exports the personal profile. → [`ResponseBody::Exported`]
    Export {
        /// The file to write.
        path: PathBuf,
        /// Its format.
        format: TransferFormat,
    },
    /// Library health. → [`ResponseBody::Health`]
    Health {},
    /// Revisions waiting for review. → [`ResponseBody::PendingReviews`]
    ListPendingReviews {},
    /// Approves a revision. → [`ResponseBody::Done`]
    ApproveSnippet {
        /// The snippet.
        id: Id,
        /// The revision.
        rev: u64,
    },

    // ---- Engine control ---------------------------------------------------------------------
    /// The service's state. → [`ResponseBody::Status`]
    GetStatus {},
    /// Turns global pause on or off. → [`ResponseBody::Status`]
    SetPaused {
        /// The new state.
        paused: bool,
    },
    /// Expands a snippet at the focus (a palette pick, or the command-line tool). The service
    /// forwards it once the focus change for `focus` has reached the engine (§4.5). →
    /// [`ResponseBody::Done`]
    ExpandSnippet {
        /// The snippet.
        id: Id,
        /// The window to type into; the focused one when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<FocusRef>,
    },
    /// Feeds `typed` to a fresh engine and returns the actions; nothing is injected. →
    /// [`ResponseBody::DryRun`]
    DryRun {
        /// What to type (user-entered).
        typed: Secret<String>,
        /// The clock as `YYYY-MM-DDTHH:MM:SS`; the service's clock when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        now: Option<String>,
        /// The clipboard text; an empty clipboard when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clipboard: Option<Secret<String>>,
        /// Form values: a form that appears is submitted with them; left open when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Secret<WireValues>>,
    },
    /// The worker's permissions. → [`ResponseBody::Permissions`]
    GetPermissions {},
    /// Opens the system settings page of a permission. → [`ResponseBody::Done`]
    OpenPermissionSettings {
        /// Which.
        kind: PermissionName,
    },
    /// Declares (or clears) the onboarding test field, the one own window where typing expands.
    /// → [`ResponseBody::Done`]
    SetTestField {
        /// The window; none clears it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<FocusRef>,
    },

    // ---- Forms ------------------------------------------------------------------------------
    /// The UI host shows the form of `session` (within 2 s of `FormRequested`, or the expansion
    /// is abandoned). → [`ResponseBody::Done`]
    FormAck {
        /// The session.
        session: u64,
    },
    /// The form was submitted and focus is back in the target window. → [`ResponseBody::Done`]
    FormSubmit {
        /// The session.
        session: u64,
        /// The values entered.
        values: Secret<WireValues>,
    },
    /// The form was cancelled. → [`ResponseBody::Done`]
    FormCancel {
        /// The session.
        session: u64,
    },
    /// Shows the most recently submitted form again. → [`ResponseBody::Done`]
    ReopenLastForm {},

    // ---- Templates --------------------------------------------------------------------------
    /// Parses a template for the editor. → [`ResponseBody::TemplateTokens`]
    ParseTemplate {
        /// The template source.
        source: String,
    },
    /// Renders a template for the editor's preview. → [`ResponseBody::Preview`]
    RenderPreview {
        /// The template source.
        source: String,
        /// Form values to render with.
        #[serde(default)]
        values: Secret<WireValues>,
    },

    // ---- Diagnostics ------------------------------------------------------------------------
    /// The diagnostics ring (at most 200 entries, oldest first). → [`ResponseBody::Diagnostics`]
    RecentDiagnostics {},

    // ---- Settings ---------------------------------------------------------------------------
    /// Every setting. → [`ResponseBody::Settings`]
    GetSettings {},
    /// Sets a setting, or removes it when `value` is absent. → [`ResponseBody::Done`]
    PutSetting {
        /// The key.
        key: String,
        /// The value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// The app exclusions. → [`ResponseBody::Exclusions`]
    ListExclusions {},
    /// Adds (`excluded: true`) or removes an app exclusion. → [`ResponseBody::Exclusions`]
    /// with the row written, or the rows removed (none when there was none).
    ///
    /// The service stores the id in the form the focus inspector reports it (Windows: the
    /// executable's file name in lower case, a path reduced to it; macOS: the bundle
    /// identifier, or the executable's file name for a program without one, compared without
    /// regard to ASCII case), so the answer may spell it differently from the request. An id
    /// no application of the system can have (empty, a control character, a path on macOS, a
    /// character no Windows file name holds) is refused with `invalid`. Since protocol 2; in
    /// protocol 1 the answer was `Done`.
    PutExclusion {
        /// Bundle id (macOS) or executable name (Windows).
        app_id: String,
        /// The name shown to the user.
        display_name: String,
        /// Add or remove.
        excluded: bool,
    },

    // ---- Sync -------------------------------------------------------------------------------
    /// The oldest pending local changes. → [`ResponseBody::Outbox`]
    OutboxRead {
        /// At most this many.
        limit: u32,
    },
    /// Removes pushed changes from the outbox. → [`ResponseBody::Acked`]
    OutboxAck {
        /// Their `seq` values.
        ids: Vec<i64>,
    },
    /// Applies rows from the server. → [`ResponseBody::Done`]
    ApplyRemote {
        /// The rows.
        batch: Vec<RemoteRow>,
    },
    /// Removes a folder whose access ended, with its snippets and cached attachments. →
    /// [`ResponseBody::Done`]
    PurgeFolder {
        /// The folder.
        id: Id,
    },
    /// An attachment's bytes are now in `attachments/`. → [`ResponseBody::Done`]
    AttachmentAvailable {
        /// The profile.
        profile: Id,
        /// The content hash.
        sha256: Sha256,
    },
    /// Attachments of the effective library that the device lacks. →
    /// [`ResponseBody::MissingAttachments`]
    MissingAttachments {},
    /// The sync agent's state, for the UI. → [`ResponseBody::Done`]
    SetSyncStatus {
        /// The state.
        status: SyncStatus,
    },
    /// The plan's features. → [`ResponseBody::Done`]
    SetEntitlements {
        /// Feature → value.
        entitlements: BTreeMap<String, String>,
    },
}

impl RequestBody {
    /// Every request name, in declaration order (the golden tests cover each).
    pub const NAMES: [&'static str; 40] = [
        "list_folders",
        "list_snippets",
        "get_snippet",
        "put_snippet",
        "delete_snippet",
        "put_folder",
        "delete_folder",
        "put_tag",
        "search",
        "import",
        "export",
        "health",
        "list_pending_reviews",
        "approve_snippet",
        "get_status",
        "set_paused",
        "expand_snippet",
        "dry_run",
        "get_permissions",
        "open_permission_settings",
        "set_test_field",
        "form_ack",
        "form_submit",
        "form_cancel",
        "reopen_last_form",
        "parse_template",
        "render_preview",
        "recent_diagnostics",
        "get_settings",
        "put_setting",
        "list_exclusions",
        "put_exclusion",
        "outbox_read",
        "outbox_ack",
        "apply_remote",
        "purge_folder",
        "attachment_available",
        "missing_attachments",
        "set_sync_status",
        "set_entitlements",
    ];

    /// The request's name on the wire (its `type`), for logs.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ListFolders {} => "list_folders",
            Self::ListSnippets { .. } => "list_snippets",
            Self::GetSnippet { .. } => "get_snippet",
            Self::PutSnippet { .. } => "put_snippet",
            Self::DeleteSnippet { .. } => "delete_snippet",
            Self::PutFolder { .. } => "put_folder",
            Self::DeleteFolder { .. } => "delete_folder",
            Self::PutTag { .. } => "put_tag",
            Self::Search { .. } => "search",
            Self::Import { .. } => "import",
            Self::Export { .. } => "export",
            Self::Health {} => "health",
            Self::ListPendingReviews {} => "list_pending_reviews",
            Self::ApproveSnippet { .. } => "approve_snippet",
            Self::GetStatus {} => "get_status",
            Self::SetPaused { .. } => "set_paused",
            Self::ExpandSnippet { .. } => "expand_snippet",
            Self::DryRun { .. } => "dry_run",
            Self::GetPermissions {} => "get_permissions",
            Self::OpenPermissionSettings { .. } => "open_permission_settings",
            Self::SetTestField { .. } => "set_test_field",
            Self::FormAck { .. } => "form_ack",
            Self::FormSubmit { .. } => "form_submit",
            Self::FormCancel { .. } => "form_cancel",
            Self::ReopenLastForm {} => "reopen_last_form",
            Self::ParseTemplate { .. } => "parse_template",
            Self::RenderPreview { .. } => "render_preview",
            Self::RecentDiagnostics {} => "recent_diagnostics",
            Self::GetSettings {} => "get_settings",
            Self::PutSetting { .. } => "put_setting",
            Self::ListExclusions {} => "list_exclusions",
            Self::PutExclusion { .. } => "put_exclusion",
            Self::OutboxRead { .. } => "outbox_read",
            Self::OutboxAck { .. } => "outbox_ack",
            Self::ApplyRemote { .. } => "apply_remote",
            Self::PurgeFolder { .. } => "purge_folder",
            Self::AttachmentAvailable { .. } => "attachment_available",
            Self::MissingAttachments {} => "missing_attachments",
            Self::SetSyncStatus { .. } => "set_sync_status",
            Self::SetEntitlements { .. } => "set_entitlements",
        }
    }

    /// The group the request belongs to (§6.2).
    #[must_use]
    pub const fn group(&self) -> Group {
        match self {
            Self::ListFolders {}
            | Self::ListSnippets { .. }
            | Self::GetSnippet { .. }
            | Self::PutSnippet { .. }
            | Self::DeleteSnippet { .. }
            | Self::PutFolder { .. }
            | Self::DeleteFolder { .. }
            | Self::PutTag { .. }
            | Self::Search { .. }
            | Self::Import { .. }
            | Self::Export { .. }
            | Self::Health {}
            | Self::ListPendingReviews {}
            | Self::ApproveSnippet { .. } => Group::Library,
            Self::GetStatus {}
            | Self::SetPaused { .. }
            | Self::ExpandSnippet { .. }
            | Self::DryRun { .. }
            | Self::GetPermissions {}
            | Self::OpenPermissionSettings { .. }
            | Self::SetTestField { .. } => Group::EngineControl,
            Self::FormAck { .. }
            | Self::FormSubmit { .. }
            | Self::FormCancel { .. }
            | Self::ReopenLastForm {} => Group::Forms,
            Self::ParseTemplate { .. } | Self::RenderPreview { .. } => Group::Templates,
            Self::RecentDiagnostics {} => Group::Diagnostics,
            Self::GetSettings {}
            | Self::PutSetting { .. }
            | Self::ListExclusions {}
            | Self::PutExclusion { .. } => Group::Settings,
            Self::OutboxRead { .. }
            | Self::OutboxAck { .. }
            | Self::ApplyRemote { .. }
            | Self::PurgeFolder { .. }
            | Self::AttachmentAvailable { .. }
            | Self::MissingAttachments {}
            | Self::SetSyncStatus { .. }
            | Self::SetEntitlements { .. } => Group::Sync,
        }
    }
}

impl fmt::Debug for RequestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---------------------------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------------------------

/// What the service answers.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseBody {
    /// Done; nothing to return.
    Done {},
    /// The personal profile and its folders.
    Folders {
        /// The personal profile.
        profile: Id,
        /// Its live folders.
        folders: Vec<Folder>,
    },
    /// Snippets.
    Snippets {
        /// The snippets.
        snippets: Vec<Snippet>,
    },
    /// One snippet.
    Snippet {
        /// The snippet.
        snippet: Snippet,
    },
    /// A row was written.
    Saved {
        /// Its sync fields after the write.
        meta: SyncMeta,
    },
    /// Search results, best first.
    SearchResults {
        /// The hits.
        hits: Vec<SearchHit>,
    },
    /// What an import did.
    Imported {
        /// Snippets written.
        snippets_imported: u64,
        /// Snippets skipped for a conflict.
        snippets_skipped: u64,
        /// Folders created.
        folders_created: u64,
        /// Tags created.
        tags_created: u64,
    },
    /// What an export wrote.
    Exported {
        /// Snippets written.
        snippets: u64,
    },
    /// Library health.
    Health {
        /// The report.
        report: HealthReport,
    },
    /// Revisions waiting for review.
    PendingReviews {
        /// The revisions.
        reviews: Vec<PendingReview>,
    },
    /// The service's state.
    Status {
        /// The state.
        status: ServiceStatus,
    },
    /// The result of a dry run.
    DryRun {
        /// Every action, in order.
        actions: Vec<WireAction>,
        /// Diagnostics the actions do not carry: the parse error of a snippet whose
        /// abbreviation the typed text ends with (such snippets are not matchable, §4.4).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hints: Vec<WireDiagnostic>,
    },
    /// The worker's permissions.
    Permissions {
        /// Accessibility.
        accessibility: GrantState,
        /// Input Monitoring.
        input_monitoring: GrantState,
    },
    /// A parsed template.
    TemplateTokens {
        /// Its tokens, or the error.
        tokens: Vec<TemplateToken>,
        /// The parse error, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<TemplateErrorInfo>,
    },
    /// A rendered preview.
    Preview {
        /// The rendered text (it can contain the values given).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<Secret<String>>,
        /// The error, if rendering failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<TemplateErrorInfo>,
    },
    /// The diagnostics ring, oldest first.
    Diagnostics {
        /// The entries.
        entries: Vec<WireDiagnostic>,
    },
    /// Every setting.
    Settings {
        /// Key → value.
        settings: BTreeMap<String, String>,
    },
    /// The app exclusions.
    Exclusions {
        /// The exclusions.
        exclusions: Vec<WireExclusion>,
    },
    /// Pending local changes, oldest first.
    Outbox {
        /// The entries.
        entries: Vec<WireOutboxEntry>,
    },
    /// Outbox entries removed.
    Acked {
        /// How many.
        count: u64,
    },
    /// Attachments the device lacks.
    MissingAttachments {
        /// The attachments.
        attachments: Vec<AttachmentRef>,
    },
}

impl ResponseBody {
    /// Every response name, in declaration order.
    pub const NAMES: [&'static str; 21] = [
        "done",
        "folders",
        "snippets",
        "snippet",
        "saved",
        "search_results",
        "imported",
        "exported",
        "health",
        "pending_reviews",
        "status",
        "dry_run",
        "permissions",
        "template_tokens",
        "preview",
        "diagnostics",
        "settings",
        "exclusions",
        "outbox",
        "acked",
        "missing_attachments",
    ];

    /// The response's name on the wire (its `type`), for logs.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Done {} => "done",
            Self::Folders { .. } => "folders",
            Self::Snippets { .. } => "snippets",
            Self::Snippet { .. } => "snippet",
            Self::Saved { .. } => "saved",
            Self::SearchResults { .. } => "search_results",
            Self::Imported { .. } => "imported",
            Self::Exported { .. } => "exported",
            Self::Health { .. } => "health",
            Self::PendingReviews { .. } => "pending_reviews",
            Self::Status { .. } => "status",
            Self::DryRun { .. } => "dry_run",
            Self::Permissions { .. } => "permissions",
            Self::TemplateTokens { .. } => "template_tokens",
            Self::Preview { .. } => "preview",
            Self::Diagnostics { .. } => "diagnostics",
            Self::Settings { .. } => "settings",
            Self::Exclusions { .. } => "exclusions",
            Self::Outbox { .. } => "outbox",
            Self::Acked { .. } => "acked",
            Self::MissingAttachments { .. } => "missing_attachments",
        }
    }
}

impl fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------

/// What the service pushes. Each event belongs to a group; a client hears the events of the
/// groups its role may call.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "topic", content = "body", rename_all = "snake_case")]
pub enum EventBody {
    /// Library content changed (Library).
    LibraryChanged {
        /// The new library revision, counted from 1 at worker start.
        revision: u64,
    },
    /// Pause or another part of the status changed (Engine control).
    StatusChanged {
        /// The new state.
        status: ServiceStatus,
    },
    /// The palette hotkey was pressed (Engine control).
    PaletteRequested {
        /// The window that had the focus.
        focus: FocusRef,
    },
    /// Show a fill-in form (Forms): the engine's `ShowForm` with the focus token attached.
    FormRequested {
        /// Answer with `FormAck`, then `FormSubmit` or `FormCancel`.
        session: u64,
        /// The snippet.
        snippet: Id,
        /// The fields, in order.
        spec: Vec<WireField>,
        /// The window to hand the focus back to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<FocusRef>,
        /// The values last submitted for this snippet, to pre-fill.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_values: Option<Secret<WireValues>>,
    },
    /// A diagnostic was recorded (Diagnostics).
    DiagnosticAdded {
        /// The entry.
        entry: WireDiagnostic,
    },
    /// Settings or exclusions changed (Settings).
    SettingsChanged {},
    /// The outbox changed (Sync).
    OutboxChanged {
        /// Pending entries.
        pending: u64,
    },
}

impl EventBody {
    /// Every event name, in declaration order.
    pub const NAMES: [&'static str; 7] = [
        "library_changed",
        "status_changed",
        "palette_requested",
        "form_requested",
        "diagnostic_added",
        "settings_changed",
        "outbox_changed",
    ];

    /// The event's name on the wire (its `topic`), for logs.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::LibraryChanged { .. } => "library_changed",
            Self::StatusChanged { .. } => "status_changed",
            Self::PaletteRequested { .. } => "palette_requested",
            Self::FormRequested { .. } => "form_requested",
            Self::DiagnosticAdded { .. } => "diagnostic_added",
            Self::SettingsChanged {} => "settings_changed",
            Self::OutboxChanged { .. } => "outbox_changed",
        }
    }

    /// The group the event belongs to.
    #[must_use]
    pub const fn group(&self) -> Group {
        match self {
            Self::LibraryChanged { .. } => Group::Library,
            Self::StatusChanged { .. } | Self::PaletteRequested { .. } => Group::EngineControl,
            Self::FormRequested { .. } => Group::Forms,
            Self::DiagnosticAdded { .. } => Group::Diagnostics,
            Self::SettingsChanged {} => Group::Settings,
            Self::OutboxChanged { .. } => Group::Sync,
        }
    }
}

impl fmt::Debug for EventBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use iamb_core::engine::{ExpansionId, Reason};

    use super::*;

    #[test]
    fn debug_shows_names_only() {
        let r = RequestBody::FormSubmit {
            session: 1,
            values: Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())])),
        };
        assert_eq!(format!("{r:?}"), "form_submit");
        let a = WireAction::TypeText {
            text: "secret".to_owned(),
            verbatim: Vec::new(),
        };
        assert_eq!(format!("{a:?}"), "TypeText(len 6)");
        assert_eq!(format!("{:?}", ResponseBody::Done {}), "done");
        assert_eq!(
            format!("{:?}", EventBody::SettingsChanged {}),
            "settings_changed"
        );
    }

    #[test]
    fn actions_and_diagnostics_convert() {
        let d =
            Diagnostic::new(Reason::InjectionCancelled).with_expansion(ExpansionId::from_raw(4));
        let w = WireDiagnostic::from(&d);
        assert_eq!(w.code, "injection_cancelled");
        assert_eq!(w.expansion, Some(4));
        assert_eq!(
            WireAction::from(&Action::PressKey {
                key: KeyName::Enter,
                shift: false,
                times: 1
            }),
            WireAction::PressKey {
                key: "enter".to_owned(),
                shift: false,
                times: 1
            }
        );
        assert_eq!(key_named("enter"), Some(KeyName::Enter));
        assert_eq!(key_named("nope"), None);
    }

    #[test]
    fn focus_tokens_convert_both_ways() {
        let t = FocusToken::from_raw(7, 99);
        let r = FocusRef::from(t);
        assert_eq!(FocusToken::from(r), t);
    }
}
