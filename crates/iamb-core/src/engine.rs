//! The engine pipeline (task 1.6): one synchronous state machine, events in, actions out.
//!
//! Implements `docs/architecture.md` §4 with ADR-0004, ADR-0010, ADR-0012, ADR-0013 and
//! ADR-0019. [`Engine::handle`] is the only entry point; the engine never performs an action
//! and reads the outside world only through [`Environment`].
//!
//! ```
//! use iamb_core::datetime::LocalDateTime;
//! use iamb_core::engine::{Action, Engine, EngineSettings, Environment, Event, SecureState, Snapshot};
//! use iamb_core::model::{Abbreviation, Id, SyncMeta, Snippet, Timestamp};
//!
//! struct Clock;
//! impl Environment for Clock {
//!     fn now(&self) -> LocalDateTime { LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("valid") }
//!     fn clipboard_text(&self) -> Option<String> { None }
//! }
//!
//! let id = Id::from_bytes([1; 16]);
//! let brb = Snippet::new_defaults(
//!     SyncMeta::new(id, Timestamp(0)), Id::from_bytes([0; 16]),
//!     Abbreviation::new("brb")?, "", "be right back", None,
//! );
//! let mut engine = Engine::new(Snapshot::build([brb], EngineSettings::default()));
//! engine.handle(Event::SecureState(SecureState::Normal), &Clock);
//! let mut out = Vec::new();
//! for (seq, ch) in "brb ".chars().enumerate() {
//!     out.extend(engine.handle(Event::char(ch, seq as u64), &Clock));
//! }
//! assert_eq!(out[0], Action::DeleteBack(4));
//! assert!(matches!(&out[1], Action::TypeText { text, .. } if text == "be right back"));
//! assert!(matches!(&out[2], Action::TypeText { text, .. } if text == " "));
//! assert!(matches!(out[3], Action::Expanded { snippet, chars_saved: 10, .. } if snippet == id));
//! # Ok::<(), iamb_core::model::ModelError>(())
//! ```
//!
//! # The keystroke buffer
//!
//! The engine owns the [`KeyBuffer`] (66 characters: one before the abbreviation, 64 and a
//! delimiter; boxed, so moving the engine does not copy typed characters, and
//! [`Engine::buffer_region`] gives the service the allocation's address range to lock against
//! swapping). Per event (architecture §4.1):
//!
//! | Event | Buffer |
//! | --- | --- |
//! | `Char` | Appended, then matched (dropped unstored while suspended) |
//! | `Backspace` | Last character removed; if the buffer was empty or the character was the NUL placeholder, the start of the buffer is marked unknown, so a `word_start` abbreviation typed next does not fire until a boundary is typed |
//! | `Boundary` | Enter and Tab are first matched as the delimiters `\n` and `\t` when the delimiter set holds them (it does not by default, ADR-0026); then cleared |
//! | `FocusChanged`, `SecureState`, `Composing`, `Paused`, `Overflow` | Cleared |
//! | `SnapshotChanged` | Kept; the next `Char` is matched against the new snapshot |
//! | an expansion | Cleared; then, unless suspended, if the expansion leaves a known non-boundary character before the caret, a NUL placeholder is pushed so a following `word_start` abbreviation does not fire mid-word. NUL never matches (abbreviations contain no control characters). |
//! | a typed match that fails (resolve or render error) | Kept: the abbreviation is still on screen, so the buffer still describes what precedes the caret |
//! | a submitted form that fails to render | Stays empty (it was cleared when the form opened); after an immediate trigger the start is marked unknown, since the abbreviation's last character precedes the caret, while after a delimiter the delimiter does, a boundary |
//!
//! # Suspension
//!
//! The engine starts suspended and is suspended after `FocusChanged` until `SecureState(Normal)`
//! arrives, while the verdict is anything else, while `Composing(true)` and while
//! `Paused(true)`. While suspended, `Char`, `Backspace` and `Boundary` produce no actions and
//! store nothing. A `SecureState` of `Secure`, `Excluded` or `Unknown` emits one
//! [`Reason::Suspended`] diagnostic naming the app (not `OwnWindow`, which every form and
//! palette causes).
//!
//! # Explicit requests
//!
//! `ExpandRequest` (a palette pick), `FormSubmitted` and `ReopenLastForm` are not typed keys,
//! but they type into the focused field, so they never type into a field Iamb has not confirmed
//! as normal (owner decision on review 1.9 finding 5):
//!
//! - With the verdict `Normal` they run at once, **also while paused**: pause blocks typed
//!   abbreviations, not an explicit pick or submit.
//! - With a verdict of `Secure`, `Excluded`, `Unknown` or `OwnWindow`, or while an input method
//!   is composing, they are dropped with [`Reason::RefusedSuspended`] naming the snippet and the
//!   app.
//! - After `FocusChanged` and before the verdict (the UI host hands focus back before it sends
//!   the request, often before the verdict for the target window has arrived), the request is
//!   **held**: the next `SecureState` runs it (`Normal`) or refuses it (anything else).
//! - Each request belongs to one application (review 1.9 follow-up N4). A submitted form
//!   belongs to the application the form was opened for: where its abbreviation was matched,
//!   or where the palette pick or reopen that opened it ran. A palette pick or reopen belongs to
//!   the application the focus is in when it arrives, which for a held request is the one
//!   named by the `FocusChanged` whose verdict is pending; if no `FocusChanged` has been seen
//!   yet, it belongs to the first one after it. A request whose application is not the focused
//!   one when it arrives, and a held request when a `FocusChanged` names another application,
//!   is dropped with [`Reason::RequestCancelled`] naming the snippet and the application the
//!   focus is in; a `FocusChanged` naming the same application keeps it held. So a submit
//!   never deletes or types in an application other than its own.
//! - The engine expects the service to send the `FocusChanged` for the window it hands focus
//!   back to (and its verdict, when known) before it forwards the request from the UI host:
//!   the request comes over IPC and the focus event from the focus thread, so only the service
//!   can order them. A request that arrives first finds the verdict `OwnWindow` (the form or
//!   palette still has focus) and is refused at once; its values are still remembered, so
//!   `ReopenLastForm` works.
//! - A newer explicit request always replaces a held one, whatever happens to the newer one
//!   (it may be held, run or be refused at once): the held one is dropped with
//!   [`Reason::RequestCancelled`]. `Overflow` drops it the same way, and so does
//!   `FormCancelled` for the session whose submit is held.
//! - A submitted form's values are remembered for `ReopenLastForm` when the submit arrives,
//!   whether it then runs, is held, refused or cancelled; the form session closes at once.
//!   [`Engine::forget_form_values`] also drops a held submit.
//! - A form answer (`FormSubmitted` or `FormCancelled`) for a session that is not open is not a
//!   request: the session was already answered or never shown, so the answer carries nothing
//!   the user asked for now. It emits [`Reason::UnknownFormSession`] and changes nothing; in
//!   particular it neither replaces nor drops a held request (the one exception is
//!   `FormCancelled` for the session whose submit is held, above). The architecture says "a
//!   newer request replaces a waiting one" without saying whether a stale answer counts; this
//!   reading is the safe one, because the held request still belongs to its own application
//!   and runs only there, so keeping it can never type into an application it did not belong
//!   to (review of the property-test counterexample in
//!   `tests/engine_props.proptest-regressions`).
//!
//! # Expansion
//!
//! A typed match emits `DeleteBack(n)` (`n` = typed abbreviation characters plus the
//! delimiter), the rendered actions (adaptive case applied to `TypeText` outside verbatim
//! ranges), the delimiter again for delimiter-mode snippets when no `{cursor}` was rendered (a
//! `PressKey` for Enter and Tab, text otherwise), `MoveCaretBack(k)` when a cursor was rendered
//! with `k > 0`, and finally `Expanded`, which carries the [`ExpansionId`]. Palette expansions
//! (`ExpandRequest`) and reopened forms emit the same without `DeleteBack` and without case or
//! delimiter. The clock is read once per expansion at match time, the clipboard once and only if
//! the template contains `{clipboard}`. A typed match whose template fails to resolve (it parses,
//! so it is matchable; architecture §4.4), and any render error, emit only a `Diagnostic` with
//! the error code; the typed abbreviation stays on screen and in the buffer.
//!
//! Rich snippets are expanded as plain templates through `TypeText` until task 3.8.
//!
//! # Forms (architecture §4.5, ADR-0013)
//!
//! A match whose template has fields captures the clock and clipboard, opens the one form
//! session and emits only `ShowForm` (the abbreviation stays on screen; the buffer is cleared).
//! While a session is open, any further typed match, palette request or reopen is dropped with
//! [`Reason::FormOpen`]. `FormSubmitted` renders with the captured time and clipboard and emits
//! the expansion; `FormCancelled` emits nothing (or drops a held submit of that session, see
//! above); an answer for a session that is neither open nor held emits
//! [`Reason::UnknownFormSession`]. The values of each submitted form are kept in memory per
//! snippet: every `ShowForm` carries them as `last_values`, and `ReopenLastForm` shows the most
//! recently submitted form again (as a palette expansion: nothing is deleted). The service
//! expires them after 30 minutes without use, on screen lock and on sign-out with
//! [`Engine::forget_form_values`].
//!
//! # Typing during injection (architecture §4.6, ADR-0019)
//!
//! Every emitted expansion puts the engine in flight until the injector has reported on it. The
//! service must send, for each `Expanded { expansion }`, either `InjectionStarted` and then
//! `InjectionFinished` (the markers the listener reports in stream order), or
//! `InjectionCancelled`, even when the expansion has nothing to inject.
//!
//! ```text
//!              expansion emitted (typed match, form submit, palette)
//!   Idle ─────────────────────────────────────────────────────────▶ Pending(id)
//!    ▲                                                                │  │
//!    │ InjectionCancelled(id): clear buffer, Diagnostic               │  │ InjectionStarted(id):
//!    │◀───────────────────────────────────────────────────────────────┘  │ clear buffer if disturbed
//!    │                                                                   ▼
//!    │ InjectionFinished(id)                                         Injecting(id)
//!    │◀────────────────────────────────────────────────────────────────┤
//!    │ InjectionCancelled(id): clear buffer, Diagnostic                │
//!    │◀────────────────────────────────────────────────────────────────┘
//!    │ Overflow (markers may have been dropped): clear buffer, forget the expansion
//! ```
//!
//! - **Forgotten** (after `Overflow`). The engine remembers the ids of the expansions it forgot
//!   (the most recent [`FORGOTTEN_EXPANSIONS`]). The service still answers every expansion, so
//!   a late `InjectionFinished` or `InjectionCancelled` for one of them is ignored silently, with
//!   no diagnostic and no change to the buffer, and ends the protocol for that id (a second
//!   answer for it is unexpected again). A late `InjectionStarted` for it is still
//!   [`Reason::UnexpectedInjectionEvent`]: the engine no longer tracks that injection, so it
//!   cannot tell keys typed between the markers apart, and the diagnostic records that.
//!
//! - **Pending** (between the trigger and the start marker). Real keys are already in the
//!   application after the abbreviation; the injector counts them (`k`), deletes `n + k`, types
//!   the expansion and types the `k` characters again. The engine stores them as usual, so when
//!   the start marker arrives its buffer holds them as they appear after the expansion text.
//!   Nothing is matched while pending (one expansion in flight at a time): the next match can
//!   happen on the first `Char` after the end marker. A `Boundary`, a `Backspace` that would
//!   delete more than was typed since the trigger, or any event that clears the buffer marks the
//!   expansion *disturbed*. The injector cancels in the same situations (a boundary among the
//!   keys, or a net negative count); the service then sends `InjectionCancelled`, and the engine
//!   clears its buffer (the abbreviation is still on screen, so what precedes the caret is not
//!   known) and emits [`Reason::InjectionCancelled`]. If the start marker arrives for a disturbed
//!   expansion anyway (the key arrived in the sub-millisecond window between counting and
//!   sending), the buffer is cleared.
//! - **Injecting** (between the markers). Real keys cannot be repaired: the first `Char`,
//!   `Backspace` or `Boundary` clears the buffer and emits [`Reason::TypedDuringInjection`];
//!   later ones in the same window are dropped unstored.
//! - A marker or cancellation that does not name the expansion in flight emits
//!   [`Reason::UnexpectedInjectionEvent`] and changes nothing, except `InjectionFinished` for the
//!   pending expansion (its start marker was lost), which ends the flight and clears the buffer,
//!   and `InjectionFinished` or `InjectionCancelled` for a forgotten expansion (above), which is
//!   ignored silently.
//! - `ExpandRequest`, `FormSubmitted` and `ReopenLastForm` that would emit an expansion while
//!   one is in flight (when they run, after the focus gate) are dropped with
//!   [`Reason::InjectionBusy`] (a submitted form's values are
//!   still remembered, so it can be reopened).
//! - Focus, secure-state, composition and pause events do not end the flight; the markers still
//!   arrive.

mod io;
mod output;
mod snapshot;

use std::collections::HashMap;

pub use io::{
    Action, AppId, BoundaryKind, Diagnostic, Event, ExpansionId, FormSessionId, Reason,
    SecureState, SuspendReason,
};
pub use snapshot::{EngineSettings, Exclusion, ExclusionReason, Snapshot};

use crate::datetime::LocalDateTime;
use crate::matcher::{CasePattern, KeyBuffer};
use crate::model::Id;
use crate::sensitive::Sensitive;
use crate::template::{FormValues, KeyName, RenderContext, RenderedAction, Resolved};

/// How many expansions forgotten because of `Overflow` the engine remembers, so that their late
/// injector answers are ignored silently (see the [module docs](self)). Only one expansion is
/// in flight at a time, so this bounds a run of overflows, each forgetting a new expansion,
/// before the injector answers any of them.
pub const FORGOTTEN_EXPANSIONS: usize = 8;

/// Everything the engine needs from outside (architecture §4.3).
pub trait Environment {
    /// The local date and time. Read once per expansion, at the match.
    fn now(&self) -> LocalDateTime;
    /// The clipboard's text, if it holds any. Read once per expansion, at the match, and only
    /// if the template contains `{clipboard}`.
    fn clipboard_text(&self) -> Option<String>;
}

/// The engine's injection state (see the [module docs](self)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionState {
    /// No expansion in flight.
    Idle,
    /// An expansion was emitted and its start marker has not arrived.
    Pending(ExpansionId),
    /// Between the start and end markers of an expansion.
    Injecting(ExpansionId),
}

/// What the focus thread last said.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// After a focus change (or at start), before the verdict.
    Awaiting,
    Verdict(SecureState),
}

/// The delimiter to type again after a delimiter-mode expansion.
#[derive(Clone, Copy)]
enum Retype {
    Char(char),
    Key(KeyName),
}

/// How an expansion was triggered: what to delete and how to adapt the output.
#[derive(Clone, Copy)]
struct Origin {
    delete: usize,
    typed_chars: usize,
    delimiter: Option<Retype>,
    case: CasePattern,
}

impl Origin {
    /// A palette request or reopened form: nothing typed, nothing to delete.
    const REQUEST: Origin = Origin {
        delete: 0,
        typed_chars: 0,
        delimiter: None,
        case: CasePattern::AsWritten,
    };
}

/// What was read from the environment at match time.
struct Capture {
    now: LocalDateTime,
    clipboard: Option<Sensitive<String>>,
}

/// An explicit request waiting for the focus verdict (see "Explicit requests" in the module
/// docs).
enum Held {
    /// A palette pick.
    Expand(Id),
    /// A submitted form: the closed session and its values.
    Submit {
        session: FormSession,
        values: Sensitive<FormValues>,
    },
    /// `ReopenLastForm`.
    Reopen,
}

/// What to do with an explicit request now.
enum Gate {
    Run,
    Hold,
    Refuse(SuspendReason),
}

struct FormSession {
    id: FormSessionId,
    /// The application the form was opened for (where the abbreviation was matched, or where
    /// the palette pick or reopen ran): a submit types only there.
    app: Option<AppId>,
    /// The snapshot at match time: the form renders the template that was matched.
    snapshot: Snapshot,
    snippet: Id,
    capture: Capture,
    origin: Origin,
}

enum Flight {
    Pending {
        id: ExpansionId,
        snippet: Id,
        /// Characters stored since the trigger, net of backspaces.
        typed_after: usize,
        disturbed: bool,
    },
    Injecting {
        id: ExpansionId,
        snippet: Id,
        interleaved: bool,
    },
}

/// The engine state machine. See the [module docs](self).
pub struct Engine {
    snapshot: Snapshot,
    buffer: Box<KeyBuffer>,
    focus: Focus,
    app: Option<AppId>,
    composing: bool,
    paused: bool,
    form: Option<FormSession>,
    flight: Option<Flight>,
    /// Expansions forgotten because of `Overflow` whose injector answer has not arrived yet,
    /// oldest first, at most [`FORGOTTEN_EXPANSIONS`].
    forgotten: Vec<ExpansionId>,
    /// The held request and the application it belongs to (`None`: not known yet; the first
    /// `FocusChanged` decides).
    held: Option<(Held, Option<AppId>)>,
    last_values: HashMap<Id, Sensitive<FormValues>>,
    last_form: Option<Id>,
    next_session: u64,
    next_expansion: u64,
}

impl Engine {
    /// A new engine over `snapshot`. It is suspended until `SecureState(Normal)` arrives.
    #[must_use]
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            buffer: Box::new(KeyBuffer::new()),
            focus: Focus::Awaiting,
            app: None,
            composing: false,
            paused: false,
            form: None,
            flight: None,
            forgotten: Vec::new(),
            held: None,
            last_values: HashMap::new(),
            last_form: None,
            next_session: 1,
            next_expansion: 1,
        }
    }

    /// Handles one event and returns the actions to perform, in order.
    pub fn handle(&mut self, event: Event, env: &dyn Environment) -> Vec<Action> {
        match event {
            Event::Char { ch, .. } => self.on_char(*ch.expose(), None, env),
            Event::Backspace { .. } => self.on_backspace(),
            Event::Boundary { kind, .. } => self.on_boundary(kind, env),
            Event::FocusChanged(app) => {
                self.clear_buffer();
                self.focus = Focus::Awaiting;
                let out = match self.held.take() {
                    Some((held, Some(owner))) if owner != app => {
                        self.cancelled(&held, Some(app.clone()))
                    }
                    Some((held, owner)) => {
                        self.held = Some((held, owner.or_else(|| Some(app.clone()))));
                        Vec::new()
                    }
                    None => Vec::new(),
                };
                self.app = Some(app);
                out
            }
            Event::SecureState(state) => {
                self.clear_buffer();
                self.focus = Focus::Verdict(state);
                let mut out = Vec::new();
                if let Some(reason) = SuspendReason::of(state)
                    && state != SecureState::OwnWindow
                {
                    let mut d = Diagnostic::new(Reason::Suspended(reason));
                    d.app.clone_from(&self.app);
                    out.push(Action::Diagnostic(d));
                }
                if let Some((held, _)) = self.held.take() {
                    out.extend(self.explicit(held, env));
                }
                out
            }
            Event::Composing(on) => {
                self.clear_buffer();
                self.composing = on;
                Vec::new()
            }
            Event::Paused(on) => {
                self.clear_buffer();
                self.paused = on;
                Vec::new()
            }
            Event::Overflow => {
                self.buffer.clear();
                if let Some(Flight::Pending { id, .. } | Flight::Injecting { id, .. }) =
                    self.flight.take()
                {
                    if self.forgotten.len() == FORGOTTEN_EXPANSIONS {
                        self.forgotten.remove(0);
                    }
                    self.forgotten.push(id);
                }
                match self.held.take() {
                    Some((held, _)) => self.cancelled(&held, None),
                    None => Vec::new(),
                }
            }
            Event::InjectionStarted { expansion, .. } => self.on_started(expansion),
            Event::InjectionFinished { expansion, .. } => self.on_finished(expansion),
            Event::InjectionCancelled { expansion } => self.on_cancelled(expansion),
            Event::ExpandRequest { snippet } => self.explicit(Held::Expand(snippet), env),
            Event::FormSubmitted { session, values } => self.on_submitted(session, values, env),
            Event::FormCancelled { session } => {
                if self.form.take_if(|s| s.id == session).is_some() {
                    Vec::new()
                } else if let Some((held, _)) = self.held.take_if(
                    |(h, _)| matches!(h, Held::Submit { session: s, .. } if s.id == session),
                ) {
                    // The form was cancelled after its submit arrived (review 1.9 follow-up N7).
                    self.cancelled(&held, None)
                } else {
                    diagnostic(Diagnostic::new(Reason::UnknownFormSession))
                }
            }
            Event::ReopenLastForm => self.explicit(Held::Reopen, env),
            Event::SnapshotChanged(snapshot) => {
                self.snapshot = snapshot;
                Vec::new()
            }
        }
    }

    /// Forgets every remembered form value (30 minutes without use, screen lock, sign-out),
    /// including a submitted form still held for the focus verdict.
    pub fn forget_form_values(&mut self) {
        self.last_values.clear();
        self.last_form = None;
        if matches!(self.held, Some((Held::Submit { .. }, _))) {
            self.held = None;
        }
    }

    /// True while an explicit request waits for the focus verdict.
    #[must_use]
    pub fn holds_request(&self) -> bool {
        self.held.is_some()
    }

    /// The current snapshot.
    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// True while typed keys are dropped unstored (see the [module docs](self)).
    #[must_use]
    pub fn is_suspended(&self) -> bool {
        self.focus != Focus::Verdict(SecureState::Normal) || self.composing || self.paused
    }

    /// The open form session, if any.
    #[must_use]
    pub fn open_form(&self) -> Option<FormSessionId> {
        self.form.as_ref().map(|s| s.id)
    }

    /// The injection state.
    #[must_use]
    pub fn injection_state(&self) -> InjectionState {
        match self.flight {
            None => InjectionState::Idle,
            Some(Flight::Pending { id, .. }) => InjectionState::Pending(id),
            Some(Flight::Injecting { id, .. }) => InjectionState::Injecting(id),
        }
    }

    /// The address range of the keystroke buffer's heap allocation, as `(address, length in
    /// bytes)`, so the service can lock it in memory against swapping (architecture §4.1,
    /// ADR-0012). Read it, never write through it: it exposes where the buffer is, not what it
    /// holds.
    ///
    /// The buffer is allocated once in [`Engine::new`] and never replaced or reallocated, so the
    /// range stays the same for the engine's whole life, also when the engine value is moved.
    /// It is never empty.
    #[must_use]
    pub fn buffer_region(&self) -> (usize, usize) {
        let buffer: &KeyBuffer = &self.buffer;
        (
            std::ptr::from_ref(buffer).addr(),
            std::mem::size_of_val(buffer),
        )
    }

    /// Number of characters in the keystroke buffer (never its content). Includes the NUL
    /// placeholder an expansion may leave.
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// True when values are remembered for `snippet`.
    #[must_use]
    pub fn remembers_form_values(&self, snippet: Id) -> bool {
        self.last_values.contains_key(&snippet)
    }

    // -----------------------------------------------------------------------------------------
    // Typed keys
    // -----------------------------------------------------------------------------------------

    fn on_char(&mut self, ch: char, key: Option<KeyName>, env: &dyn Environment) -> Vec<Action> {
        if self.is_suspended() {
            return Vec::new();
        }
        match &mut self.flight {
            Some(Flight::Injecting { .. }) => return self.interleaved(),
            Some(Flight::Pending { typed_after, .. }) => {
                self.buffer.push(ch);
                *typed_after += 1;
                return Vec::new();
            }
            None => {}
        }
        self.buffer.push(ch);
        let Some(m) = self.snapshot.matcher().lookup(&self.buffer) else {
            return Vec::new();
        };
        if self.form.is_some() {
            self.buffer.clear();
            return diagnostic(Diagnostic::new(Reason::FormOpen).with_snippet(m.snippet));
        }
        // The buffer keeps the typed abbreviation until the expansion replaces it: if the
        // template fails to resolve or render, the text stays on screen and the buffer still
        // describes what precedes the caret (review 1.9 finding 13).
        let origin = Origin {
            delete: m.backspaces(),
            typed_chars: m.typed_chars,
            delimiter: m
                .delimiter
                .map(|d| key.map_or(Retype::Char(d), Retype::Key)),
            case: m.case,
        };
        self.start(m.snippet, origin, env)
    }

    fn on_backspace(&mut self) -> Vec<Action> {
        if self.is_suspended() {
            return Vec::new();
        }
        match &mut self.flight {
            Some(Flight::Injecting { .. }) => return self.interleaved(),
            Some(Flight::Pending {
                typed_after,
                disturbed,
                ..
            }) => match typed_after.checked_sub(1) {
                Some(n) => *typed_after = n,
                None => *disturbed = true,
            },
            None => {}
        }
        // Deleting a character the buffer never held (it is empty), or the placeholder for the
        // last character of an expansion, leaves an unknown character before the caret: the
        // start of the buffer is then not a word start (review 1.9 finding 13).
        let unknown_before = self.buffer.chars().last().is_none_or(|&c| c == '\0');
        self.buffer.backspace();
        if unknown_before {
            self.buffer.mark_unknown_prefix();
        }
        Vec::new()
    }

    fn on_boundary(&mut self, kind: BoundaryKind, env: &dyn Environment) -> Vec<Action> {
        if self.is_suspended() {
            return Vec::new();
        }
        match self.flight {
            Some(Flight::Injecting { .. }) => return self.interleaved(),
            Some(Flight::Pending { .. }) => {
                self.clear_buffer();
                return Vec::new();
            }
            None => {}
        }
        if let Some(d) = kind.as_delimiter() {
            let key = if kind == BoundaryKind::Enter {
                KeyName::Enter
            } else {
                KeyName::Tab
            };
            let actions = self.on_char(d, Some(key), env);
            if !actions.is_empty() {
                return actions;
            }
        }
        self.buffer.clear();
        Vec::new()
    }

    /// A real key between the injector's markers.
    fn interleaved(&mut self) -> Vec<Action> {
        self.buffer.clear();
        if let Some(Flight::Injecting {
            id,
            snippet,
            interleaved,
        }) = &mut self.flight
            && !*interleaved
        {
            *interleaved = true;
            return diagnostic(
                Diagnostic::new(Reason::TypedDuringInjection)
                    .with_snippet(*snippet)
                    .with_expansion(*id),
            );
        }
        Vec::new()
    }

    /// Clears the buffer; an expansion waiting for its start marker becomes disturbed.
    fn clear_buffer(&mut self) {
        self.buffer.clear();
        if let Some(Flight::Pending { disturbed, .. }) = &mut self.flight {
            *disturbed = true;
        }
    }

    // -----------------------------------------------------------------------------------------
    // Injection markers
    // -----------------------------------------------------------------------------------------

    fn on_started(&mut self, expansion: ExpansionId) -> Vec<Action> {
        match self.flight {
            Some(Flight::Pending {
                id,
                snippet,
                disturbed,
                ..
            }) if id == expansion => {
                if disturbed {
                    self.buffer.clear();
                }
                self.flight = Some(Flight::Injecting {
                    id,
                    snippet,
                    interleaved: false,
                });
                Vec::new()
            }
            _ => unexpected(expansion),
        }
    }

    fn on_finished(&mut self, expansion: ExpansionId) -> Vec<Action> {
        match self.flight {
            Some(Flight::Injecting { id, .. }) if id == expansion => {
                self.flight = None;
                Vec::new()
            }
            Some(Flight::Pending { id, .. }) if id == expansion => {
                self.flight = None;
                self.buffer.clear();
                unexpected(expansion)
            }
            _ => self.forgotten_answer(expansion),
        }
    }

    fn on_cancelled(&mut self, expansion: ExpansionId) -> Vec<Action> {
        match self.flight {
            Some(Flight::Pending { id, snippet, .. } | Flight::Injecting { id, snippet, .. })
                if id == expansion =>
            {
                self.flight = None;
                self.buffer.clear();
                diagnostic(
                    Diagnostic::new(Reason::InjectionCancelled)
                        .with_snippet(snippet)
                        .with_expansion(id),
                )
            }
            _ => self.forgotten_answer(expansion),
        }
    }

    /// `InjectionFinished` or `InjectionCancelled` for an expansion not in flight: ignored
    /// silently (and the id forgotten for good) if `Overflow` made the engine forget it,
    /// unexpected otherwise.
    fn forgotten_answer(&mut self, expansion: ExpansionId) -> Vec<Action> {
        match self.forgotten.iter().position(|&e| e == expansion) {
            Some(i) => {
                self.forgotten.remove(i);
                Vec::new()
            }
            None => unexpected(expansion),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Requests and forms
    // -----------------------------------------------------------------------------------------

    fn on_request(&mut self, snippet: Id, env: &dyn Environment) -> Vec<Action> {
        if self.form.is_some() {
            return diagnostic(Diagnostic::new(Reason::FormOpen).with_snippet(snippet));
        }
        self.start(snippet, Origin::REQUEST, env)
    }

    /// Whether an explicit request runs now, waits for the focus verdict or is refused.
    fn gate(&self) -> Gate {
        if self.composing {
            return Gate::Refuse(SuspendReason::Composing);
        }
        match self.focus {
            Focus::Awaiting => Gate::Hold,
            Focus::Verdict(state) => SuspendReason::of(state).map_or(Gate::Run, Gate::Refuse),
        }
    }

    /// An explicit request: run it, hold it for the focus verdict or refuse it; a held request
    /// is dropped either way, since this one replaces it (see the module docs).
    fn explicit(&mut self, request: Held, env: &dyn Environment) -> Vec<Action> {
        let mut out = match self.held.take() {
            Some((older, _)) => self.cancelled(&older, None),
            None => Vec::new(),
        };
        // A submit belongs to the application its form was opened for; a pick or reopen to the
        // one the focus is in now.
        let owner = match &request {
            Held::Submit { session, .. } => session.app.clone().or_else(|| self.app.clone()),
            Held::Expand(_) | Held::Reopen => self.app.clone(),
        };
        let elsewhere = matches!((&owner, &self.app), (Some(o), Some(a)) if o != a);
        match self.gate() {
            Gate::Refuse(reason) => {
                let mut d = Diagnostic::new(Reason::RefusedSuspended(reason));
                d.snippet = self.held_snippet(&request);
                d.app.clone_from(&self.app);
                out.push(Action::Diagnostic(d));
            }
            Gate::Run | Gate::Hold if elsewhere => {
                out.extend(self.cancelled(&request, self.app.clone()));
            }
            Gate::Hold => self.held = Some((request, owner)),
            Gate::Run => out.extend(match request {
                Held::Expand(snippet) => self.on_request(snippet, env),
                Held::Submit { session, values } => self.submit(&session, &values),
                Held::Reopen => self.on_reopen(env),
            }),
        }
        out
    }

    /// The diagnostic for an explicit request dropped without running; `app` names the
    /// application the focus moved to when that is why.
    fn cancelled(&self, held: &Held, app: Option<AppId>) -> Vec<Action> {
        let mut d = Diagnostic::new(Reason::RequestCancelled);
        d.snippet = self.held_snippet(held);
        d.app = app;
        vec![Action::Diagnostic(d)]
    }

    fn held_snippet(&self, held: &Held) -> Option<Id> {
        match held {
            Held::Expand(snippet) => Some(*snippet),
            Held::Submit { session, .. } => Some(session.snippet),
            Held::Reopen => self.last_form,
        }
    }

    fn on_submitted(
        &mut self,
        session: FormSessionId,
        values: Sensitive<FormValues>,
        env: &dyn Environment,
    ) -> Vec<Action> {
        let Some(s) = self.form.take_if(|s| s.id == session) else {
            return diagnostic(Diagnostic::new(Reason::UnknownFormSession));
        };
        // Remembered at once, so the form can be reopened even if the submit is refused.
        self.last_form = Some(s.snippet);
        self.last_values.insert(s.snippet, values.clone());
        self.explicit(Held::Submit { session: s, values }, env)
    }

    /// Expands a submitted form into the focused field.
    fn submit(&mut self, s: &FormSession, values: &Sensitive<FormValues>) -> Vec<Action> {
        let plain = values.expose();
        match s.snapshot.compiled(s.snippet) {
            Some(Ok(resolved)) => self.expand(
                &s.snapshot,
                s.snippet,
                resolved,
                &s.capture,
                plain,
                s.origin,
            ),
            // A session is only opened for a snippet that compiled in its snapshot.
            _ => diagnostic(Diagnostic::new(Reason::UnknownSnippet).with_snippet(s.snippet)),
        }
    }

    fn on_reopen(&mut self, env: &dyn Environment) -> Vec<Action> {
        if self.form.is_some() {
            return diagnostic(Diagnostic::new(Reason::FormOpen));
        }
        let Some(snippet) = self.last_form else {
            return diagnostic(Diagnostic::new(Reason::NoLastForm));
        };
        let has_form = matches!(
            self.snapshot.compiled(snippet),
            Some(Ok(r)) if !r.form().is_empty()
        );
        if !has_form {
            self.last_values.remove(&snippet);
            self.last_form = None;
            return diagnostic(Diagnostic::new(Reason::NoLastForm).with_snippet(snippet));
        }
        self.start(snippet, Origin::REQUEST, env)
    }

    // -----------------------------------------------------------------------------------------
    // Expansion
    // -----------------------------------------------------------------------------------------

    /// A match or request for `snippet`: capture the environment, then show a form or expand.
    fn start(&mut self, snippet: Id, origin: Origin, env: &dyn Environment) -> Vec<Action> {
        let snapshot = self.snapshot.clone();
        let resolved = match snapshot.compiled(snippet) {
            Some(Ok(r)) => r,
            Some(Err(e)) => {
                return diagnostic(
                    Diagnostic::new(Reason::Template(e.kind())).with_snippet(snippet),
                );
            }
            None => {
                return diagnostic(Diagnostic::new(Reason::UnknownSnippet).with_snippet(snippet));
            }
        };
        let capture = Capture {
            now: env.now(),
            clipboard: if resolved.needs_clipboard() {
                env.clipboard_text().map(Sensitive::new)
            } else {
                None
            },
        };
        if resolved.form().is_empty() {
            return self.expand(
                &snapshot,
                snippet,
                resolved,
                &capture,
                &FormValues::new(),
                origin,
            );
        }
        let id = FormSessionId::from_raw(self.next_session);
        self.next_session += 1;
        let spec = resolved.form().to_vec();
        self.form = Some(FormSession {
            id,
            app: self.app.clone(),
            snapshot,
            snippet,
            capture,
            origin,
        });
        self.buffer.clear();
        vec![Action::ShowForm {
            session: id,
            snippet,
            spec,
            last_values: self.last_values.get(&snippet).cloned(),
        }]
    }

    /// Renders and emits one expansion, and puts the engine in flight.
    fn expand(
        &mut self,
        snapshot: &Snapshot,
        snippet: Id,
        resolved: &Resolved,
        capture: &Capture,
        values: &FormValues,
        origin: Origin,
    ) -> Vec<Action> {
        if self.flight.is_some() {
            return diagnostic(Diagnostic::new(Reason::InjectionBusy).with_snippet(snippet));
        }
        let settings = snapshot.settings();
        let ctx = RenderContext {
            now: capture.now,
            locale: settings.locale,
            date_format: &settings.date_format,
            time_format: &settings.time_format,
            clipboard: capture.clipboard.as_ref().map(|c| c.expose().as_str()),
        };
        let rendered = match resolved.render(&ctx, values) {
            Ok(r) => r,
            Err(e) => {
                // Nothing was typed or deleted. A typed match still has its abbreviation in the
                // buffer, which stays as it is. A submitted form's abbreviation is still on
                // screen, but the buffer was cleared when the form opened: after a delimiter
                // trigger the delimiter precedes the caret, a boundary, so the empty buffer is
                // right; after an immediate trigger the last character of the abbreviation
                // does, which the buffer no longer holds (review 1.9 finding 13).
                if origin.delete > 0 && origin.delimiter.is_none() && self.buffer.is_empty() {
                    self.buffer.mark_unknown_prefix();
                }
                return diagnostic(
                    Diagnostic::new(Reason::Template(e.kind())).with_snippet(snippet),
                );
            }
        };

        let mut body: Vec<Action> = rendered
            .actions
            .into_iter()
            .map(|a| match a {
                RenderedAction::Text { text, verbatim } => Action::TypeText { text, verbatim },
                RenderedAction::Key { key, shift, times } => Action::PressKey { key, shift, times },
                RenderedAction::Wait { ms } => Action::Wait(ms),
            })
            .collect();
        output::apply_case(&mut body, origin.case);
        let chars_saved = output::typed_graphemes(&body).saturating_sub(origin.typed_chars);

        let mut out = Vec::with_capacity(body.len() + 4);
        if origin.delete > 0 {
            out.push(Action::DeleteBack(origin.delete));
        }
        out.append(&mut body);
        match rendered.cursor_back {
            None => match origin.delimiter {
                Some(Retype::Char(c)) => out.push(Action::TypeText {
                    text: c.to_string(),
                    verbatim: Vec::new(),
                }),
                Some(Retype::Key(key)) => out.push(Action::PressKey {
                    key,
                    shift: false,
                    times: 1,
                }),
                None => {}
            },
            Some(0) => {}
            Some(n) => out.push(Action::MoveCaretBack(n)),
        }

        self.buffer.clear();
        if !self.is_suspended()
            && let Some(c) = output::char_before_caret(&out)
        {
            let delimiters = &snapshot.settings().delimiters;
            if !c.is_whitespace() && !delimiters.contains(c) && !delimiters.contains('\0') {
                self.buffer.push('\0');
            }
        }

        let id = ExpansionId::from_raw(self.next_expansion);
        self.next_expansion += 1;
        self.flight = Some(Flight::Pending {
            id,
            snippet,
            typed_after: 0,
            disturbed: false,
        });
        out.push(Action::Expanded {
            expansion: id,
            snippet,
            chars_saved,
        });
        out
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("snapshot", &self.snapshot)
            .field("suspended", &self.is_suspended())
            .field("open_form", &self.open_form())
            .field("injection", &self.injection_state())
            .finish_non_exhaustive()
    }
}

fn diagnostic(d: Diagnostic) -> Vec<Action> {
    vec![Action::Diagnostic(d)]
}

fn unexpected(expansion: ExpansionId) -> Vec<Action> {
    diagnostic(Diagnostic::new(Reason::UnexpectedInjectionEvent).with_expansion(expansion))
}

/// Feeds each character of `typed` as a `Char` event to a fresh engine whose focus verdict is
/// already `Normal`, and returns every action (the command-line tool and `DryRun`).
///
/// Each expansion is followed at once by its injector markers, as if injection took no time,
/// so later abbreviations in `typed` expand too. When a `ShowForm` appears and `values` is
/// given, the form is submitted with them; without `values` the form stays open, and later
/// matches are dropped with [`Reason::FormOpen`].
#[must_use]
pub fn dry_run(
    snapshot: &Snapshot,
    typed: &str,
    env: &dyn Environment,
    values: Option<&FormValues>,
) -> Vec<Action> {
    let mut engine = Engine::new(snapshot.clone());
    let mut out = engine.handle(Event::SecureState(SecureState::Normal), env);
    for (seq, ch) in (0_u64..).zip(typed.chars()) {
        let mut pending = engine.handle(Event::char(ch, seq), env);
        while !pending.is_empty() {
            let mut follow = Vec::new();
            for action in &pending {
                match action {
                    Action::Expanded { expansion, .. } => {
                        for event in [
                            Event::InjectionStarted {
                                expansion: *expansion,
                                seq,
                            },
                            Event::InjectionFinished {
                                expansion: *expansion,
                                seq,
                            },
                        ] {
                            follow.extend(engine.handle(event, env));
                        }
                    }
                    Action::ShowForm { session, .. } => {
                        if let Some(values) = values {
                            follow.extend(engine.handle(
                                Event::FormSubmitted {
                                    session: *session,
                                    values: Sensitive::new(values.clone()),
                                },
                                env,
                            ));
                        }
                    }
                    _ => {}
                }
            }
            out.append(&mut pending);
            pending = follow;
        }
    }
    out
}
