//! Events in, actions out (architecture §4.1, §4.2) and diagnostics (§8 rule 4).

use std::fmt;
use std::ops::Range;

use super::snapshot::Snapshot;
use crate::model::{Id, Sha256};
use crate::sensitive::Sensitive;
use crate::template::{FieldSpec, FormValues, KeyName};

/// Identifies one fill-in form session. Unique per engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormSessionId(u64);

impl FormSessionId {
    /// Wraps a raw value (for tests and IPC decoding; the engine allocates real ones).
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw value, for IPC encoding.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Identifies one expansion: the injection actions the engine emitted for it, and the injector's
/// start and end markers around them (architecture §4.6). Unique per engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExpansionId(u64);

impl ExpansionId {
    /// Wraps a raw value (for tests and for the injector's markers; the engine allocates real
    /// ones).
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw value.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Identifies the application that has the focus: a bundle id or executable name, never a
/// window title (titles can contain document text).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppId(pub String);

/// A key or click that produced no text (architecture §4.1 `Boundary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundaryKind {
    /// Enter or Return. Counts as the delimiter `\n` first.
    Enter,
    /// Tab. Counts as the delimiter `\t` first.
    Tab,
    /// Escape.
    Escape,
    /// An arrow key.
    Arrow,
    /// Home, End, Page Up, Page Down and similar navigation keys.
    Navigation,
    /// A mouse click.
    Click,
    /// A shortcut chord that produced no text.
    Shortcut,
    /// Any other key that produced no text.
    Other,
}

impl BoundaryKind {
    /// The delimiter character this boundary counts as before it clears the buffer, if any.
    #[must_use]
    pub const fn as_delimiter(self) -> Option<char> {
        match self {
            Self::Enter => Some('\n'),
            Self::Tab => Some('\t'),
            _ => None,
        }
    }
}

/// The focus thread's verdict on the focused window (architecture §4.1 `SecureState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecureState {
    /// An ordinary text field: typing is matched.
    Normal,
    /// A password or other secure field.
    Secure,
    /// An app the user excluded.
    Excluded,
    /// One of Iamb's own windows (except the onboarding test field).
    OwnWindow,
    /// The state could not be determined (timeout, or a window Iamb may not inspect). Fails
    /// closed.
    Unknown,
}

/// One input to [`Engine::handle`](super::Engine::handle).
///
/// `seq` is the listener's sequence number, carried for ordering; the engine does not depend
/// on it. `Debug` never shows typed characters or form values.
pub enum Event {
    /// A key produced text (after layout and dead-key handling).
    Char {
        /// The character.
        ch: Sensitive<char>,
        /// Listener sequence number.
        seq: u64,
    },
    /// Backspace was pressed.
    Backspace {
        /// Listener sequence number.
        seq: u64,
    },
    /// A key or click that produced no text.
    Boundary {
        /// Which kind.
        kind: BoundaryKind,
        /// Listener sequence number.
        seq: u64,
    },
    /// Another window or app took focus. The engine is suspended until [`Event::SecureState`].
    FocusChanged(AppId),
    /// The focus thread's verdict for the focused window.
    SecureState(SecureState),
    /// An input method started (`true`) or finished composing.
    Composing(bool),
    /// Global pause toggled on (`true`) or off.
    Paused(bool),
    /// The listener dropped events.
    Overflow,
    /// The injector's start marker for an expansion, in listener order.
    InjectionStarted {
        /// The expansion the injector is about to send (from [`Action::Expanded`]).
        expansion: ExpansionId,
        /// Listener sequence number of the marker.
        seq: u64,
    },
    /// The injector's end marker for an expansion, in listener order.
    InjectionFinished {
        /// The expansion the injector has sent.
        expansion: ExpansionId,
        /// Listener sequence number of the marker.
        seq: u64,
    },
    /// The injector did not send an expansion (a boundary or an over-deleting Backspace among
    /// the keys typed after the trigger, or it gave up), or stopped part-way. Sent by the
    /// service instead of, or after, [`Event::InjectionStarted`].
    InjectionCancelled {
        /// The expansion that was abandoned.
        expansion: ExpansionId,
    },
    /// The search palette or the command-line tool asked to expand a snippet. The service keeps
    /// the focus token; the engine does not need it.
    ExpandRequest {
        /// The snippet to expand.
        snippet: Id,
    },
    /// The user submitted a fill-in form; focus is back in the target window.
    FormSubmitted {
        /// The session from [`Action::ShowForm`].
        session: FormSessionId,
        /// The values entered.
        values: Sensitive<FormValues>,
    },
    /// The user cancelled a fill-in form.
    FormCancelled {
        /// The session from [`Action::ShowForm`].
        session: FormSessionId,
    },
    /// Show the most recently submitted form again, pre-filled with its last values
    /// (architecture §4.5 `ReopenLastForm`).
    ReopenLastForm,
    /// New library content or engine settings.
    SnapshotChanged(Snapshot),
}

impl Event {
    /// A `Char` event.
    #[must_use]
    pub const fn char(ch: char, seq: u64) -> Self {
        Self::Char {
            ch: Sensitive::new(ch),
            seq,
        }
    }
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char { seq, .. } => f
                .debug_struct("Char")
                .field("ch", &Redacted)
                .field("seq", seq)
                .finish(),
            Self::Backspace { seq } => f.debug_struct("Backspace").field("seq", seq).finish(),
            Self::Boundary { kind, seq } => f
                .debug_struct("Boundary")
                .field("kind", kind)
                .field("seq", seq)
                .finish(),
            Self::FocusChanged(app) => f.debug_tuple("FocusChanged").field(app).finish(),
            Self::SecureState(s) => f.debug_tuple("SecureState").field(s).finish(),
            Self::Composing(b) => f.debug_tuple("Composing").field(b).finish(),
            Self::Paused(b) => f.debug_tuple("Paused").field(b).finish(),
            Self::Overflow => f.write_str("Overflow"),
            Self::InjectionStarted { expansion, seq } => f
                .debug_struct("InjectionStarted")
                .field("expansion", expansion)
                .field("seq", seq)
                .finish(),
            Self::InjectionFinished { expansion, seq } => f
                .debug_struct("InjectionFinished")
                .field("expansion", expansion)
                .field("seq", seq)
                .finish(),
            Self::InjectionCancelled { expansion } => f
                .debug_struct("InjectionCancelled")
                .field("expansion", expansion)
                .finish(),
            Self::ExpandRequest { snippet } => f
                .debug_struct("ExpandRequest")
                .field("snippet", snippet)
                .finish(),
            Self::FormSubmitted { session, .. } => f
                .debug_struct("FormSubmitted")
                .field("session", session)
                .field("values", &Redacted)
                .finish(),
            Self::FormCancelled { session } => f
                .debug_struct("FormCancelled")
                .field("session", session)
                .finish(),
            Self::ReopenLastForm => f.write_str("ReopenLastForm"),
            Self::SnapshotChanged(s) => f.debug_tuple("SnapshotChanged").field(s).finish(),
        }
    }
}

/// One output of [`Engine::handle`](super::Engine::handle).
///
/// The injection actions of one expansion ([`DeleteBack`](Self::DeleteBack),
/// [`TypeText`](Self::TypeText), [`Paste`](Self::Paste), [`PressKey`](Self::PressKey),
/// [`MoveCaretBack`](Self::MoveCaretBack), [`Wait`](Self::Wait)) are always followed by the
/// [`Expanded`](Self::Expanded) action that closes the group and names its [`ExpansionId`]. The
/// service hands each group to the injector as one unit.
///
/// `Debug` never shows text, which can contain form values or clipboard content.
#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    /// Press Backspace this many times: one per `Char` event consumed by the abbreviation and
    /// delimiter.
    DeleteBack(usize),
    /// Insert plain text.
    TypeText {
        /// The text, with LF line endings.
        text: String,
        /// Byte ranges of `text` that came from field values or the clipboard, ascending.
        verbatim: Vec<Range<usize>>,
    },
    /// Insert rich content through the clipboard, then restore the clipboard. Not emitted until
    /// task 3.8; rich snippets expand as plain `TypeText` for now.
    Paste {
        /// The plain-text version.
        plain: String,
        /// The HTML version.
        markdown_html: String,
        /// Attachments the HTML references.
        attachments: Vec<Sha256>,
    },
    /// Press a named key.
    PressKey {
        /// The key.
        key: KeyName,
        /// Hold Shift.
        shift: bool,
        /// How many presses.
        times: u8,
    },
    /// Press Left this many times: one per extended grapheme cluster after the `{cursor}`.
    MoveCaretBack(usize),
    /// Pause this many milliseconds before the next action.
    Wait(u32),
    /// Ask the UI host to show a fill-in form. The service attaches the focus token.
    ShowForm {
        /// The session to answer with `FormSubmitted` or `FormCancelled`.
        session: FormSessionId,
        /// The snippet the form belongs to.
        snippet: Id,
        /// The fields, in order.
        spec: Vec<FieldSpec>,
        /// The values last submitted for this snippet, if still remembered, to pre-fill.
        last_values: Option<Sensitive<FormValues>>,
    },
    /// An expansion was emitted. Closes the group of injection actions before it; the service
    /// records usage once the injector reports it done (not if it is cancelled).
    Expanded {
        /// The expansion, for the injector's markers.
        expansion: ExpansionId,
        /// The snippet expanded.
        snippet: Id,
        /// Characters the user did not have to type: grapheme clusters of rendered text minus
        /// the typed abbreviation's characters, at least 0.
        chars_saved: usize,
    },
    /// Why something did or did not expand.
    Diagnostic(Diagnostic),
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeleteBack(n) => f.debug_tuple("DeleteBack").field(n).finish(),
            Self::TypeText { text, verbatim } => f
                .debug_struct("TypeText")
                .field("len", &text.len())
                .field("verbatim", verbatim)
                .finish_non_exhaustive(),
            Self::Paste {
                plain,
                markdown_html,
                attachments,
            } => f
                .debug_struct("Paste")
                .field("plain_len", &plain.len())
                .field("html_len", &markdown_html.len())
                .field("attachments", &attachments.len())
                .finish_non_exhaustive(),
            Self::PressKey { key, shift, times } => f
                .debug_struct("PressKey")
                .field("key", key)
                .field("shift", shift)
                .field("times", times)
                .finish(),
            Self::MoveCaretBack(n) => f.debug_tuple("MoveCaretBack").field(n).finish(),
            Self::Wait(ms) => f.debug_tuple("Wait").field(ms).finish(),
            Self::ShowForm {
                session,
                snippet,
                spec,
                last_values,
            } => f
                .debug_struct("ShowForm")
                .field("session", session)
                .field("snippet", snippet)
                .field("fields", &spec.len())
                .field("last_values", &last_values.as_ref().map(|_| Redacted))
                .finish(),
            Self::Expanded {
                expansion,
                snippet,
                chars_saved,
            } => f
                .debug_struct("Expanded")
                .field("expansion", expansion)
                .field("snippet", snippet)
                .field("chars_saved", chars_saved)
                .finish(),
            Self::Diagnostic(d) => f.debug_tuple("Diagnostic").field(d).finish(),
        }
    }
}

/// Why the engine is suspended, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuspendReason {
    /// A secure field has the focus.
    SecureField,
    /// The focused app is excluded.
    ExcludedApp,
    /// The focus state could not be determined.
    UnknownFocus,
}

/// The reason code of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The template failed at render time (`E_DATE_RANGE`, `E_CURSOR_BEFORE_KEY`,
    /// `E_WAIT_TOTAL`, `E_OUTPUT_TOO_LARGE`), or (for a palette request) failed to parse or
    /// resolve.
    Template(crate::template::ErrorKind),
    /// A match or request arrived while a fill-in form was open; it was dropped.
    FormOpen,
    /// A form answer named a session that is not open.
    UnknownFormSession,
    /// `ReopenLastForm` with no remembered form, or one whose snippet is gone.
    NoLastForm,
    /// A palette request named a snippet that is not in the reference set (or lost on an
    /// abbreviation clash).
    UnknownSnippet,
    /// A request arrived while another expansion was being injected; it was dropped.
    InjectionBusy,
    /// Real keys arrived between the injector's start and end markers. The buffer was cleared.
    TypedDuringInjection,
    /// The injector cancelled the expansion.
    InjectionCancelled,
    /// An injection marker or cancellation did not match the expansion in flight.
    UnexpectedInjectionEvent,
    /// The engine was suspended by the focus verdict; nothing typed is stored or matched.
    Suspended(SuspendReason),
}

impl Reason {
    /// The stable reason code: the template error code (`E_…`) or a snake-case name.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Template(kind) => kind.code(),
            Self::FormOpen => "form_open",
            Self::UnknownFormSession => "unknown_form_session",
            Self::NoLastForm => "no_last_form",
            Self::UnknownSnippet => "unknown_snippet",
            Self::InjectionBusy => "injection_busy",
            Self::TypedDuringInjection => "typed_during_injection",
            Self::InjectionCancelled => "injection_cancelled",
            Self::UnexpectedInjectionEvent => "unexpected_injection_event",
            Self::Suspended(SuspendReason::SecureField) => "suspended_secure_field",
            Self::Suspended(SuspendReason::ExcludedApp) => "suspended_excluded_app",
            Self::Suspended(SuspendReason::UnknownFocus) => "suspended_unknown_focus",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A diagnostic entry (architecture §8 rule 4). It never contains typed text, form values or
/// clipboard content: a snippet is named only when a match or request named one, and an app
/// only for suspensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Why.
    pub reason: Reason,
    /// The snippet involved, if a match or request named one.
    pub snippet: Option<Id>,
    /// The expansion involved, for injection diagnostics.
    pub expansion: Option<ExpansionId>,
    /// The focused app, for suspension diagnostics.
    pub app: Option<AppId>,
}

impl Diagnostic {
    /// A diagnostic with only a reason.
    #[must_use]
    pub const fn new(reason: Reason) -> Self {
        Self {
            reason,
            snippet: None,
            expansion: None,
            app: None,
        }
    }

    /// The same diagnostic naming `snippet`.
    #[must_use]
    pub const fn with_snippet(mut self, snippet: Id) -> Self {
        self.snippet = Some(snippet);
        self
    }

    /// The same diagnostic naming `expansion`.
    #[must_use]
    pub const fn with_expansion(mut self, expansion: ExpansionId) -> Self {
        self.expansion = Some(expansion);
        self
    }
}

struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
