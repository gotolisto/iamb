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
//! # Positions in text
//!
//! Every position in a template, a preview or a search hit is a UTF-16 code unit offset, the
//! unit of JavaScript string indices, so the editor and the palette slice with them directly.
//! (The verbatim ranges of [`WireAction::TypeText`], which only the command-line tool reads,
//! stay byte offsets.)
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
use iamb_core::model::{Folder, Id, Sha256, Snippet, SyncMeta, Tag, Timestamp};
use iamb_core::sync::{
    AccessRemoval, AccessRoot, AckOutcome, ApplyMode, ApplyStatus, Change, KeptChange, LostLine,
    OpResult, OutboxPhase, ReconcileScope, RemoteRow, RowRef, SyncInfo,
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

/// Which snippets [`RequestBody::ListSnippets`] and [`RequestBody::ListSnippetSummaries`]
/// return.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetQuery {
    /// Only snippets in this folder (not its subfolders); all folders when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Id>,
    /// Only snippets that carry this tag, in any folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Id>,
}

/// The eight colours a tag may have (`docs/ux/design-system.md`, token `tag`): the stored
/// `Tag.color` is one of these names, so a tag looks right in both themes.
pub const TAG_COLORS: [&str; 8] = [
    "red", "orange", "yellow", "green", "teal", "blue", "purple", "grey",
];

/// A tag with the number of live snippets that carry it ([`ResponseBody::Tags`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEntry {
    /// The tag.
    pub tag: Tag,
    /// How many snippets carry it.
    pub snippets: u64,
}

/// One snippet without its body, for a long list ([`ResponseBody::SnippetSummaries`]).
/// Library content, not typed text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetSummary {
    /// The snippet.
    pub id: Id,
    /// Its folder.
    pub folder: Id,
    /// What the user types.
    pub abbreviation: String,
    /// The label.
    pub label: String,
    /// The first line of the body, at most [`SUMMARY_LINE_CHARS`] characters.
    pub first_line: String,
    /// `plain` or `rich`.
    pub kind: String,
    /// `immediate` or `delimiter`.
    pub trigger: String,
    /// Whether it expands.
    pub enabled: bool,
    /// Its tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Id>,
    /// Whether expanding it asks for values: a `{field}` or `{if}` anywhere in the template,
    /// in an included snippet too (the first line alone cannot tell). Absent when it does not;
    /// an older service never sends it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_fields: bool,
    /// Why it does not expand although enabled: `duplicate` or a template error code
    /// (`E_…`), as in [`HealthReport`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken: Option<String>,
    /// How many times it expanded on this computer (device-local, ADR-0015; never synced).
    /// Absent when the service does not count: a client must not read it as "never used".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uses: Option<u64>,
    /// When it last expanded on this computer; absent when it has not since the count began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<Timestamp>,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// The most characters of a body a [`SnippetSummary`] carries.
pub const SUMMARY_LINE_CHARS: usize = 120;

/// Rows a library change touched ([`EventBody::LibraryChanged`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedRows {
    /// Snippets written, deleted or restored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<Id>,
    /// Folders written or deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<Id>,
    /// Tags written or deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Id>,
}

/// One of Iamb's two panels that can hold the keyboard while another application stays the
/// active one (macOS non-activating panels, §4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    /// The fill-in form.
    Form,
    /// The search palette.
    Palette,
}

impl PanelKind {
    /// The panel's name on the wire, for logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Form => "form",
            Self::Palette => "palette",
        }
    }
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

/// Which field of a snippet a search hit matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    /// The abbreviation.
    Abbreviation,
    /// The label.
    Label,
    /// The body (the template source).
    Body,
    /// The name of a tag it carries.
    Tag,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// The snippet.
    pub snippet: Id,
    /// Higher is better.
    pub score: u32,
    /// The field that matched, which decided the score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<SearchField>,
    /// The matched ranges `[start, end)` in that field, in UTF-16 code units (for a tag, in
    /// the tag's name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<[usize; 2]>,
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
    /// For a broken reference (`E_SNIPPET_NOT_FOUND`, `E_SNIPPET_CYCLE`, or `E_TOO_DEEP` found
    /// at a `{snippet}`): the abbreviation the failing `{snippet}` names, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// For a template error found in an included snippet: that snippet's abbreviation. Absent
    /// when the mistake is in the snippet's own content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub included: Option<String>,
}

/// A snippet nobody used on this computer for 90 days (device-local, ADR-0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnusedEntry {
    /// The snippet.
    pub snippet: Id,
    /// When it last expanded; absent when it never did since it was created (or since the
    /// counts were reset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<Timestamp>,
}

/// Library health (spec §4.9): one list per kind of problem. Every list is empty when nothing
/// is wrong; every addition after protocol 3 is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    /// "Never reached": immediate abbreviations that hide longer ones.
    pub shadowed: Vec<ShadowedEntry>,
    /// "Will not expand": abbreviations taken by another snippet, templates that do not parse,
    /// and broken references to other snippets (missing, cyclic, nested too deep).
    pub excluded: Vec<ExcludedEntry>,
    /// "Not used on this computer for 90 days": enabled snippets that expand but were not used,
    /// oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unused: Vec<UnusedEntry>,
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
    /// The pause was kept from before this worker started (it outlives a quit, a logout and a
    /// restart of the machine) and nobody has paused or resumed since. Optional on the wire:
    /// absent reads as `false`.
    #[serde(default)]
    pub paused_kept: bool,
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
    /// `keys_for_unreported_window`, `panel_focus_expired`, …).
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
    /// When it was recorded (UTC milliseconds). Every entry carries it (since protocol 3): the
    /// service stamps each one as it records it, and a dry run's diagnostics with the time of
    /// the dry run.
    pub at: Timestamp,
}

impl WireDiagnostic {
    /// A diagnostic with only a code, recorded at `at`.
    pub fn code(code: impl Into<String>, at: Timestamp) -> Self {
        Self {
            code: code.into(),
            snippet: None,
            expansion: None,
            app: None,
            at,
        }
    }

    /// The engine's diagnostic `d`, recorded at `at`.
    #[must_use]
    pub fn from_engine(d: &Diagnostic, at: Timestamp) -> Self {
        Self {
            code: d.reason.code().to_owned(),
            snippet: d.snippet,
            expansion: d.expansion.map(iamb_core::engine::ExpansionId::raw),
            app: d.app.as_ref().map(|a| a.0.clone()),
            at,
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

impl WireAction {
    /// The engine's `action`; a diagnostic among them is stamped with `at`.
    #[must_use]
    pub fn from_engine(action: &Action, at: Timestamp) -> Self {
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
                diagnostic: WireDiagnostic::from_engine(d, at),
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

/// One attribute of a macro token, as written (quotes removed, escapes resolved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateAttribute {
    /// The attribute's name (`fmt`, `name`, …).
    pub name: String,
    /// Its value.
    pub value: String,
}

/// One token of a template, for the editor's chips: literal text, or one macro with its
/// attributes. Library content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateToken {
    /// `text`, or the macro's name: `date`, `time`, `clipboard`, `cursor`, `snippet`, `field`,
    /// `key`, `wait`, `if`, `else`, `endif`, `lbrace`.
    pub kind: String,
    /// UTF-16 offset of the start in the source.
    pub start: usize,
    /// UTF-16 offset after the end (a macro's `}` included).
    pub end: usize,
    /// A macro's attributes, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<TemplateAttribute>,
}

/// A template error: its code and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateErrorInfo {
    /// The error code (`E_…`).
    pub code: String,
    /// UTF-16 offset in the source where the mistake starts; absent for errors that only
    /// rendering finds (`E_DATE_RANGE`, `E_WAIT_TOTAL`, …), and for an error inside an
    /// included snippet (then [`snippet`](Self::snippet) names it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<usize>,
    /// UTF-16 offset after the mistake: the end of the macro it is in, when it is in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<usize>,
    /// The abbreviation of the included snippet whose body holds the mistake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// A range of preview text that a fill-in field produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewBlank {
    /// UTF-16 offset of the start in the part's text.
    pub start: usize,
    /// UTF-16 offset after the end.
    pub end: usize,
    /// The field's name.
    pub field: String,
    /// True when no value was given and the preview shows a sample (the field's name): the
    /// editor draws it as a grey name tag.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sample: bool,
}

/// One part of a preview, in output order.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum PreviewPart {
    /// Text that would be typed.
    Text {
        /// The text (it can contain the values given).
        text: String,
        /// Ranges a text or area field produced.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blanks: Vec<PreviewBlank>,
        /// Ranges `[start, end)` (UTF-16) that came from the clipboard; the preview shows the
        /// sample `Clipboard`, never the real clipboard.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        clipboard: Vec<[usize; 2]>,
    },
    /// A key press, drawn as a key badge.
    Key {
        /// The key's template name (`enter`, `tab`, …).
        key: String,
        /// Hold Shift.
        shift: bool,
        /// How many presses.
        times: u8,
    },
    /// A wait, drawn as "· 0.5 s ·".
    Wait {
        /// Milliseconds.
        ms: u32,
    },
    /// A run of a rich snippet's text, as it would be pasted (task 3.8; only when the request
    /// asked for `rich`).
    Formatted {
        /// The sanitised HTML (`iamb_core::rich`); images reference attachments as
        /// `attachment:<sha256>`.
        html: String,
        /// The plain-text version.
        plain: String,
        /// The attachments the HTML references, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<WireAttachment>,
    },
}

/// An attachment on the wire (task 3.8): what it is and, when its bytes are on this device,
/// where (the UI host reads them from there to show an image; bulk data travels as a path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireAttachment {
    /// The hash that names it.
    pub sha256: Sha256,
    /// Its media type; empty when the profile has no such attachment.
    pub mime: String,
    /// Its size in bytes.
    pub size: u64,
    /// The original file name.
    pub file_name: String,
    /// Where its bytes are, checked against the hash; absent when they are not on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

impl fmt::Debug for PreviewPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text, blanks, .. } => {
                write!(f, "Text(len {}, blanks {})", text.len(), blanks.len())
            }
            Self::Formatted {
                html, attachments, ..
            } => write!(
                f,
                "Formatted(html len {}, attachments {})",
                html.len(),
                attachments.len()
            ),
            Self::Key { key, shift, times } => write!(f, "Key({key}, shift {shift}, x{times})"),
            Self::Wait { ms } => write!(f, "Wait({ms})"),
        }
    }
}

/// Where the cursor of a preview is: in part `part` (a text part), at UTF-16 offset `offset`.
/// `part` is the number of parts when no text surrounds the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewCursor {
    /// The part.
    pub part: usize,
    /// The offset in its text.
    pub offset: usize,
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
    ///
    /// The id of a deleted snippet restores it (Undo), with its usage and its tags.
    PutSnippet {
        /// The row.
        snippet: Snippet,
        /// The snippet's tags, all of them: tags it carries and that are not listed are
        /// detached, in the same write. Absent: its tags stay as they are.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tags: Option<Vec<Id>>,
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
    /// Inserts or updates (renames, recolours) a tag. → [`ResponseBody::Saved`]
    ///
    /// The colour must be one of [`TAG_COLORS`]; the name must not be empty or be the name of
    /// another tag of the profile, compared without regard to case (`conflict`).
    PutTag {
        /// The row.
        tag: Tag,
    },
    /// The personal profile's tags, each with its count, by name. → [`ResponseBody::Tags`]
    ListTags {},
    /// Soft-deletes a tag and detaches it from every snippet. → [`ResponseBody::Done`]
    DeleteTag {
        /// The tag.
        id: Id,
    },
    /// One page of snippet summaries (no bodies), in the library's order. →
    /// [`ResponseBody::SnippetSummaries`]
    ListSnippetSummaries {
        /// Which.
        #[serde(default)]
        query: SnippetQuery,
        /// How many matching snippets to skip.
        #[serde(default)]
        offset: u32,
        /// At most this many (at most 1000).
        limit: u32,
    },
    /// What saving a snippet with this abbreviation would run into, without saving anything
    /// (the editor's warnings, §4.4). → [`ResponseBody::AbbreviationCheck`]
    CheckAbbreviation {
        /// The abbreviation.
        abbreviation: String,
        /// The trigger it would have: `immediate` or `delimiter`.
        trigger: String,
        /// The snippet being edited, left out of the comparison.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snippet: Option<Id>,
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
    /// The snippets used most recently on this computer, newest first (the palette with
    /// nothing typed, §4.5): a hit has no matched field and no ranges. →
    /// [`ResponseBody::SearchResults`]
    RecentSnippets {
        /// At most this many hits.
        limit: u32,
    },
    /// Forgets every usage count and last-used time on this computer (Settings → Privacy).
    /// Nothing is synced and no snippet changes. → [`ResponseBody::Done`]
    ResetUsage {},
    /// Revisions waiting for review. → [`ResponseBody::PendingReviews`]
    ListPendingReviews {},
    /// The changes the server refused, or that lost their folder (D5). →
    /// [`ResponseBody::KeptChanges`]
    ListKeptChanges {},
    /// Removes one kept change. → [`ResponseBody::Done`]
    DismissKeptChange {
        /// The kept change.
        id: Id,
    },
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
    ///
    /// Sent again while the form stays open, it renews the form's lease
    /// ([`FORM_LEASE`](crate::FORM_LEASE); every 10 seconds is enough): a form whose UI host
    /// stops renewing it is cancelled by the service, with the diagnostic `form_expired`, so
    /// that a form the UI host no longer shows cannot keep typed abbreviations refused with
    /// `form_open`. An acknowledgement for a session that is not open changes nothing.
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
    ///
    /// `focus` names the window it is for, as `ExpandSnippet`'s does: the request waits for that
    /// window's focus change and runs under its verdict (the tray's menu and the palette send
    /// the window they hand the focus back to; the beta review's re-verification, N4). Without
    /// it the request runs against the focused window, and waits for the next window that is
    /// not Iamb's when one of Iamb's own has the focus. Optional, so a client that leaves it out
    /// is still understood (§6.1).
    ReopenLastForm {
        /// The window to show the form for; the focused one when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<FocusRef>,
    },
    /// One of Iamb's panels has the keyboard (`shown: true`) or no longer has it (§4.5; the
    /// 2.8 review's finding 16). While a panel has it, keys typed are typed into Iamb's own
    /// window: never buffered, matched, expanded or handed off, whatever the focused
    /// application. The service answers once the engine has taken it in, so the UI host sends
    /// `shown: true` and waits for the answer before the panel takes the keyboard, and sends
    /// `shown: false` once it has given the keyboard back (before `FormSubmit` or
    /// `ExpandSnippet`).
    ///
    /// `shown: true` holds for [`PANEL_FOCUS_LEASE`](crate::PANEL_FOCUS_LEASE): the UI host
    /// sends it again while the panel stays open (every 10 seconds is enough). A lease that
    /// runs out, and the UI host disconnecting, end it as `shown: false` would, with the
    /// diagnostic `panel_focus_expired` for the first. `FormSubmit` and `FormCancel` end a form
    /// panel; `ExpandSnippet` and `ReopenLastForm` end a palette panel. → [`ResponseBody::Done`]
    PanelFocus {
        /// Which panel.
        panel: PanelKind,
        /// Whether it has the keyboard.
        shown: bool,
    },

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
        /// Render as a rich snippet (task 3.8): each run of text becomes a
        /// [`PreviewPart::Formatted`] with the sanitised HTML. An older service ignores it and
        /// answers with plain parts.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rich: bool,
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
    /// Changes ready to send, built under D1 (`docs/sync-design.md` §7.2); the entries they cover
    /// are marked as handed out in the same transaction. → [`ResponseBody::Changes`]
    OutboxRead {
        /// At most this many rows (and the rows they refer to that the server never saw).
        limit: u32,
        /// Which changes (in a new epoch, what took access away goes first, §6.7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<OutboxPhase>,
    },
    /// Acts on the server's result for each change (D4, D5). → [`ResponseBody::SyncAcked`]
    OutboxAck {
        /// The results, each with the outbox entries it covers.
        results: Vec<OpResult>,
    },
    /// Applies rows from the server (D2, D3, D7). → [`ResponseBody::Applied`]
    ApplyRemote {
        /// The rows; a null row removes the local row.
        batch: Vec<RemoteRow>,
        /// The position to store in the same transaction.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<String>,
        /// `feed`, `reconcile` or `epoch`.
        #[serde(default)]
        mode: ApplyMode,
    },
    /// Removes a tree whose access ended, keeping unsent changes for the user (D6). →
    /// [`ResponseBody::Done`]
    PurgeFolder {
        /// The folder.
        id: Id,
        /// The sub-trees that stay readable.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        except: Vec<Id>,
    },
    /// The store's sync state. → [`ResponseBody::SyncState`]
    SyncState {},
    /// Records the account, its personal profile and the device id. → [`ResponseBody::Done`]
    SetAccount {
        /// The account.
        user: Id,
        /// Its personal profile.
        personal_profile: Id,
        /// This installation's device id.
        device: Id,
    },
    /// Replaces the device id (C25). → [`ResponseBody::Done`]
    SetDeviceId {
        /// The new id.
        device: Id,
    },
    /// Stores the position in the feed. → [`ResponseBody::Done`]
    SetPosition {
        /// The position, or none.
        position: Option<String>,
    },
    /// Begins a reconcile (D7). → [`ResponseBody::ReconcileToken`]
    ReconcileBegin {
        /// Everything, or one tree.
        scope: ReconcileScope,
        /// After a restore of the server: the lines of the restore.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lost: Option<LostLine>,
    },
    /// Ends a reconcile (D7). → [`ResponseBody::AccessRemovals`]
    ReconcileEnd {
        /// The token `ReconcileBegin` answered.
        token: String,
        /// The position to store.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<String>,
    },
    /// Forgets the lines of a restore. → [`ResponseBody::Done`]
    ClearLostLines {},
    /// The user's access roots (`SetAccess`). → [`ResponseBody::Done`]
    SetAccess {
        /// The roots outside the personal profile.
        roots: Vec<AccessRoot>,
    },
    /// Marks a change that keeps failing as blocked (D17). → [`ResponseBody::Done`]
    SetBlocked {
        /// The entries of the change.
        seqs: Vec<i64>,
        /// Why (`server_error`).
        reason: String,
    },
    /// Offers blocked changes again. → [`ResponseBody::Done`]
    Unblock {
        /// Only changes blocked for this reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Notes an account call that takes access away (D7). → [`ResponseBody::Done`]
    NoteAccessRemoval {
        /// The entry.
        entry: AccessRemoval,
        /// The call was refused: the entry goes.
        #[serde(default)]
        refused: bool,
    },
    /// After a sign-out (D14). → [`ResponseBody::Done`]
    SignedOut {
        /// The server confirmed it.
        confirmed: bool,
        /// Keep the unsent changes to shared folders in the list.
        keep: bool,
    },
    /// The folders the device holds outside the personal profile (the access check, D7). →
    /// [`ResponseBody::Ids`]
    ForeignFolders {},
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

    // ---- Library (task 3.8) -----------------------------------------------------------------
    /// Adds a file as an attachment of the profile that owns `folder`, for an image in a rich
    /// snippet. The bytes travel as a path (§6.1); the service reads the file, checks its type
    /// and size (`iamb_store::attachments`) and keeps it under its hash. The same content again
    /// gives back the attachment that exists. → [`ResponseBody::Attachment`]
    PutAttachment {
        /// The file to read.
        path: PathBuf,
        /// The name to keep with it (the original file name).
        file_name: String,
        /// The folder of the snippet that will use it.
        folder: Id,
    },
}

impl RequestBody {
    /// Every request name, in declaration order (the golden tests cover each).
    pub const NAMES: [&'static str; 63] = [
        "list_folders",
        "list_snippets",
        "get_snippet",
        "put_snippet",
        "delete_snippet",
        "put_folder",
        "delete_folder",
        "put_tag",
        "list_tags",
        "delete_tag",
        "list_snippet_summaries",
        "check_abbreviation",
        "search",
        "import",
        "export",
        "health",
        "list_pending_reviews",
        "list_kept_changes",
        "dismiss_kept_change",
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
        "panel_focus",
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
        "sync_state",
        "set_account",
        "set_device_id",
        "set_position",
        "reconcile_begin",
        "reconcile_end",
        "clear_lost_lines",
        "set_access",
        "set_blocked",
        "unblock",
        "note_access_removal",
        "signed_out",
        "foreign_folders",
        "put_attachment",
        "recent_snippets",
        "reset_usage",
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
            Self::ListTags {} => "list_tags",
            Self::DeleteTag { .. } => "delete_tag",
            Self::ListSnippetSummaries { .. } => "list_snippet_summaries",
            Self::CheckAbbreviation { .. } => "check_abbreviation",
            Self::Search { .. } => "search",
            Self::Import { .. } => "import",
            Self::Export { .. } => "export",
            Self::Health {} => "health",
            Self::RecentSnippets { .. } => "recent_snippets",
            Self::ResetUsage {} => "reset_usage",
            Self::ListPendingReviews {} => "list_pending_reviews",
            Self::ListKeptChanges {} => "list_kept_changes",
            Self::DismissKeptChange { .. } => "dismiss_kept_change",
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
            Self::ReopenLastForm { .. } => "reopen_last_form",
            Self::PanelFocus { .. } => "panel_focus",
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
            Self::SyncState {} => "sync_state",
            Self::SetAccount { .. } => "set_account",
            Self::SetDeviceId { .. } => "set_device_id",
            Self::SetPosition { .. } => "set_position",
            Self::ReconcileBegin { .. } => "reconcile_begin",
            Self::ReconcileEnd { .. } => "reconcile_end",
            Self::ClearLostLines {} => "clear_lost_lines",
            Self::SetAccess { .. } => "set_access",
            Self::SetBlocked { .. } => "set_blocked",
            Self::Unblock { .. } => "unblock",
            Self::NoteAccessRemoval { .. } => "note_access_removal",
            Self::SignedOut { .. } => "signed_out",
            Self::ForeignFolders {} => "foreign_folders",
            Self::PutAttachment { .. } => "put_attachment",
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
            | Self::ListTags {}
            | Self::DeleteTag { .. }
            | Self::ListSnippetSummaries { .. }
            | Self::CheckAbbreviation { .. }
            | Self::Search { .. }
            | Self::Import { .. }
            | Self::Export { .. }
            | Self::Health {}
            | Self::RecentSnippets { .. }
            | Self::ListPendingReviews {}
            | Self::ListKeptChanges {}
            | Self::DismissKeptChange { .. }
            | Self::ApproveSnippet { .. }
            | Self::PutAttachment { .. } => Group::Library,
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
            | Self::ReopenLastForm { .. }
            | Self::PanelFocus { .. } => Group::Forms,
            Self::ParseTemplate { .. } | Self::RenderPreview { .. } => Group::Templates,
            Self::RecentDiagnostics {} => Group::Diagnostics,
            Self::GetSettings {}
            | Self::PutSetting { .. }
            | Self::ListExclusions {}
            | Self::PutExclusion { .. }
            | Self::ResetUsage {} => Group::Settings,
            Self::OutboxRead { .. }
            | Self::OutboxAck { .. }
            | Self::ApplyRemote { .. }
            | Self::PurgeFolder { .. }
            | Self::AttachmentAvailable { .. }
            | Self::MissingAttachments {}
            | Self::SetSyncStatus { .. }
            | Self::SetEntitlements { .. }
            | Self::SyncState {}
            | Self::SetAccount { .. }
            | Self::SetDeviceId { .. }
            | Self::SetPosition { .. }
            | Self::ReconcileBegin { .. }
            | Self::ReconcileEnd { .. }
            | Self::ClearLostLines {}
            | Self::SetAccess { .. }
            | Self::SetBlocked { .. }
            | Self::Unblock { .. }
            | Self::NoteAccessRemoval { .. }
            | Self::SignedOut { .. }
            | Self::ForeignFolders {} => Group::Sync,
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
        /// The tags of each snippet that has any.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        tags: BTreeMap<Id, Vec<Id>>,
    },
    /// One snippet.
    Snippet {
        /// The snippet.
        snippet: Snippet,
        /// Its tags.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<Id>,
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
    /// A rendered preview: what an expansion would type (text, key presses, waits), where
    /// the cursor would be and which fields the form would ask for. Empty, with the error,
    /// when the template has one. Since protocol 3 (before: the text only).
    Preview {
        /// The parts, in output order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<PreviewPart>,
        /// Where the cursor would be.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<PreviewCursor>,
        /// The fill-in fields, in form order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<WireField>,
        /// The error, if the template has one.
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
    /// Changes ready to send (`OutboxRead`).
    Changes {
        /// The changes, in dependency order.
        changes: Vec<Change>,
        /// Rows whose last entry was dropped and that are behind what a pull skipped (D15).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        behind: Vec<RowRef>,
    },
    /// What `OutboxAck` did (D15, D7).
    SyncAcked {
        /// Rows now behind, and left-alone rows removed.
        outcome: AckOutcome,
    },
    /// What `ApplyRemote` did with each row.
    Applied {
        /// One status per row, in the order sent.
        status: Vec<ApplyStatus>,
    },
    /// The store's sync state.
    SyncState {
        /// The state.
        info: SyncInfo,
    },
    /// The token of a reconcile.
    ReconcileToken {
        /// The token.
        token: String,
    },
    /// The access removals a restore lost (`ReconcileEnd`).
    AccessRemovals {
        /// The entries.
        entries: Vec<AccessRemoval>,
    },
    /// Ids.
    Ids {
        /// The ids.
        ids: Vec<Id>,
    },
    /// The kept changes.
    KeptChanges {
        /// The kept changes.
        changes: Vec<KeptChange>,
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
    /// The tags, by name.
    Tags {
        /// The tags.
        tags: Vec<TagEntry>,
    },
    /// One page of snippet summaries.
    SnippetSummaries {
        /// The library revision the page was read from ([`EventBody::LibraryChanged`]).
        revision: u64,
        /// How many snippets match the query in all.
        total: u64,
        /// This page.
        summaries: Vec<SnippetSummary>,
        /// The offset of the next page; absent on the last.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<u32>,
    },
    /// What saving an abbreviation would run into.
    AbbreviationCheck {
        /// The snippet that has this abbreviation already (compared case-folded, §4.4):
        /// saving would be refused with `conflict`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conflict: Option<Id>,
        /// Snippets it would hide: it expands immediately and their abbreviations start with
        /// it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hides: Vec<Id>,
        /// Snippets that would hide it: they expand immediately and it starts with their
        /// abbreviation.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hidden_by: Vec<Id>,
    },
    /// An attachment that was added or found (task 3.8).
    Attachment {
        /// The attachment.
        attachment: WireAttachment,
    },
}

impl ResponseBody {
    /// Every response name, in declaration order.
    pub const NAMES: [&'static str; 32] = [
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
        "changes",
        "sync_acked",
        "applied",
        "sync_state",
        "reconcile_token",
        "access_removals",
        "ids",
        "kept_changes",
        "acked",
        "missing_attachments",
        "tags",
        "snippet_summaries",
        "abbreviation_check",
        "attachment",
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
            Self::Changes { .. } => "changes",
            Self::SyncAcked { .. } => "sync_acked",
            Self::Applied { .. } => "applied",
            Self::SyncState { .. } => "sync_state",
            Self::ReconcileToken { .. } => "reconcile_token",
            Self::AccessRemovals { .. } => "access_removals",
            Self::Ids { .. } => "ids",
            Self::KeptChanges { .. } => "kept_changes",
            Self::Acked { .. } => "acked",
            Self::MissingAttachments { .. } => "missing_attachments",
            Self::Tags { .. } => "tags",
            Self::SnippetSummaries { .. } => "snippet_summaries",
            Self::AbbreviationCheck { .. } => "abbreviation_check",
            Self::Attachment { .. } => "attachment",
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
        /// The rows the change touched; absent when it may have touched any (an import, a
        /// change from sync), so the client reloads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changed: Option<ChangedRows>,
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
        let w = WireDiagnostic::from_engine(&d, Timestamp(5));
        assert_eq!(w.code, "injection_cancelled");
        assert_eq!(w.expansion, Some(4));
        assert_eq!(w.at, Timestamp(5));
        assert_eq!(
            WireAction::from_engine(&Action::Diagnostic(d), Timestamp(6)),
            WireAction::Diagnostic {
                diagnostic: WireDiagnostic {
                    at: Timestamp(6),
                    ..w
                }
            }
        );
        assert_eq!(
            WireAction::from_engine(
                &Action::PressKey {
                    key: KeyName::Enter,
                    shift: false,
                    times: 1
                },
                Timestamp(0)
            ),
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
