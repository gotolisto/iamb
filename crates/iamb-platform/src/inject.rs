//! Injection: what the injector is asked to send and how it answers (architecture §4.2, §4.6,
//! §5; ADR-0019).
//!
//! # Why steps and not engine actions
//!
//! The injector takes [`InjectionStep`]s, not [`Action`]s:
//!
//! - `Action` also has `ShowForm`, `Expanded` and `Diagnostic`, which are the service's
//!   business; with its own type, an OS crate has no arm for them to get wrong;
//! - a `Paste` action names attachments by hash. Only the service can turn that into
//!   clipboard content (it owns the store); the step carries the finished
//!   [`ClipboardContent`];
//! - the `verbatim` ranges of `TypeText` matter to the engine's case rules only.
//!
//! [`split_actions`] does the conversion and groups the steps of each expansion.
//!
//! # One call per expansion
//!
//! Architecture §5 lists the injector's operations as `delete_back`, `type_text`, `paste`,
//! `press_key`, `move_caret` and `mark`. They are the step kinds and the two markers here, but
//! the trait has a single method that takes the whole expansion: only then can Windows send
//! the deletion, the text and the end marker as one batch that real keys cannot interleave
//! (§4.6), and only then is the marker protocol the injector's own duty and not something
//! every caller must repeat.

use std::fmt;

use iamb_core::engine::{Action, AppId, ExpansionId};
use iamb_core::model::Id;
use iamb_core::template::KeyName;

use crate::clipboard::ClipboardContent;
use crate::error::{self, PlatformError};
use crate::handoff::{CancelReason, TypedHandoff};

/// One thing to send to the focused application.
///
/// `Debug` never shows text: it can contain form values or clipboard content.
#[derive(Clone, PartialEq, Eq)]
pub enum InjectionStep {
    /// Press Backspace this many times.
    DeleteBack(usize),
    /// Insert plain text (LF line endings). How it gets there, synthetic Unicode keys or a
    /// paste for long text, is the injector's choice.
    TypeText(String),
    /// Insert this content through the clipboard, then restore the clipboard.
    Paste(ClipboardContent),
    /// Press a named key.
    PressKey {
        /// The key.
        key: KeyName,
        /// Hold Shift.
        shift: bool,
        /// How many presses.
        times: u8,
    },
    /// Press Left this many times.
    MoveCaretBack(usize),
    /// Pause this many milliseconds before the next step.
    Wait(u32),
}

impl InjectionStep {
    /// The step for an injection action; any other action is handed back.
    ///
    /// A `Paste` action becomes a step with its plain text and HTML as they are. Its
    /// attachment references are not resolved here: the service replaces the content with the
    /// resolved one (HTML with embedded images, or a file) before it calls the injector
    /// (architecture §9, task 3.8).
    ///
    /// # Errors
    ///
    /// The action itself, when it is `ShowForm`, `Expanded` or `Diagnostic`.
    pub fn from_action(action: Action) -> Result<Self, Action> {
        Ok(match action {
            Action::DeleteBack(n) => Self::DeleteBack(n),
            Action::TypeText { text, .. } => Self::TypeText(text),
            Action::Paste {
                plain,
                markdown_html,
                ..
            } => Self::Paste(ClipboardContent::rich(plain, markdown_html)),
            Action::PressKey { key, shift, times } => Self::PressKey { key, shift, times },
            Action::MoveCaretBack(n) => Self::MoveCaretBack(n),
            Action::Wait(ms) => Self::Wait(ms),
            other @ (Action::ShowForm { .. } | Action::Expanded { .. } | Action::Diagnostic(_)) => {
                return Err(other);
            }
        })
    }

    /// True when sending this step may move the focus by itself, so that a
    /// [`FocusGuard`] must stop comparing after it: a key press other than Backspace, Delete
    /// and Left (Tab, Enter, Escape, Space on a button, an arrow in a list), and text with a
    /// line break or a tab in it. Deleting, moving the caret back, waiting and the paste
    /// shortcut do not move it.
    #[must_use]
    pub fn may_move_focus(&self) -> bool {
        match self {
            Self::DeleteBack(_) | Self::MoveCaretBack(_) | Self::Wait(_) | Self::Paste(_) => false,
            Self::TypeText(text) => text.contains(['\n', '\r', '\t']),
            Self::PressKey { key, .. } => {
                !matches!(key, KeyName::Backspace | KeyName::Delete | KeyName::Left)
            }
        }
    }
}

impl fmt::Debug for InjectionStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeleteBack(n) => f.debug_tuple("DeleteBack").field(n).finish(),
            Self::TypeText(text) => f
                .debug_struct("TypeText")
                .field("len", &text.len())
                .finish_non_exhaustive(),
            Self::Paste(content) => f.debug_tuple("Paste").field(content).finish(),
            Self::PressKey { key, shift, times } => f
                .debug_struct("PressKey")
                .field("key", key)
                .field("shift", shift)
                .field("times", times)
                .finish(),
            Self::MoveCaretBack(n) => f.debug_tuple("MoveCaretBack").field(n).finish(),
            Self::Wait(ms) => f.debug_tuple("Wait").field(ms).finish(),
        }
    }
}

/// The injection steps of one expansion, with what its closing `Expanded` action said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionBatch {
    /// The expansion, for the markers and the hand-off.
    pub expansion: ExpansionId,
    /// The snippet expanded (for usage statistics once the injection has finished).
    pub snippet: Id,
    /// Characters the user did not have to type.
    pub chars_saved: usize,
    /// What to send, in order. May be empty; the markers are still due (§4.6).
    pub steps: Vec<InjectionStep>,
}

/// Splits what [`Engine::handle`](iamb_core::engine::Engine::handle) returned into the
/// expansions to inject and everything else (`ShowForm`, `Diagnostic`), each in order.
///
/// The engine closes every group of injection actions with `Expanded`. Injection actions with
/// no `Expanded` after them would be a broken engine; they are returned with the other actions
/// and never injected.
#[must_use]
pub fn split_actions(actions: Vec<Action>) -> (Vec<ExpansionBatch>, Vec<Action>) {
    let mut batches = Vec::new();
    let mut other = Vec::new();
    let mut open: Vec<Action> = Vec::new();
    for action in actions {
        match action {
            Action::Expanded {
                expansion,
                snippet,
                chars_saved,
            } => batches.push(ExpansionBatch {
                expansion,
                snippet,
                chars_saved,
                steps: open
                    .drain(..)
                    .filter_map(|a| InjectionStep::from_action(a).ok())
                    .collect(),
            }),
            Action::ShowForm { .. } | Action::Diagnostic(_) => other.push(action),
            step => open.push(step),
        }
    }
    other.append(&mut open);
    (batches, other)
}

/// How text reaches the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextMethod {
    /// Synthetic Unicode key events.
    Keys,
    /// The clipboard and the paste shortcut.
    Paste,
}

/// The user's overrides of the injector's per-application strategy table (architecture §4.2;
/// stored in settings as "per-app injection overrides"). `None` leaves the choice to the
/// table in the OS crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct InjectionOverrides {
    /// Force how `TypeText` is sent.
    pub text_method: Option<TextMethod>,
    /// Pause this many milliseconds between synthetic key events (slow terminals and remote
    /// desktops).
    pub key_delay_ms: Option<u32>,
}

/// One expansion for the injector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectionRequest {
    /// The expansion: the markers carry this id and the hand-off is armed for it.
    pub expansion: ExpansionId,
    /// What to send, in order.
    pub steps: Vec<InjectionStep>,
    /// The application the expansion is for, as the engine knows it. The injector looks its
    /// strategy up under this name; `None` uses the defaults.
    pub app: Option<AppId>,
    /// The user's overrides for that application.
    pub overrides: InjectionOverrides,
}

impl InjectionRequest {
    /// A request with the default strategy.
    #[must_use]
    pub fn new(expansion: ExpansionId, steps: Vec<InjectionStep>) -> Self {
        Self {
            expansion,
            steps,
            app: None,
            overrides: InjectionOverrides::default(),
        }
    }

    /// The request for a batch from [`split_actions`].
    #[must_use]
    pub fn for_batch(batch: &ExpansionBatch) -> Self {
        Self::new(batch.expansion, batch.steps.clone())
    }

    /// The same request for `app`.
    #[must_use]
    pub fn in_app(mut self, app: AppId) -> Self {
        self.app = Some(app);
        self
    }
}

/// The steps of an expansion, adjusted for the `k` characters typed after its trigger (§4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compensated<'a> {
    /// Backspaces to send first: the expansion's own deletion plus `k`.
    pub delete_first: usize,
    /// The remaining steps, in order.
    pub rest: &'a [InjectionStep],
    /// Characters to type again after `rest`: `k`.
    pub retype: usize,
}

/// Adjusts `steps` for `typed_after` characters that reached the application after the
/// trigger: they sit between the abbreviation and the caret, so they are deleted with it and
/// typed again after the last step, where the caret then is. An expansion with no leading
/// `DeleteBack` (a palette pick, a submitted form) deletes just those characters.
///
/// Shared by the OS injectors and the mock so they compensate alike.
#[must_use]
pub fn compensate(steps: &[InjectionStep], typed_after: usize) -> Compensated<'_> {
    let (own, rest) = match steps.split_first() {
        Some((InjectionStep::DeleteBack(n), rest)) => (*n, rest),
        _ => (0, steps),
    };
    Compensated {
        delete_first: own + typed_after,
        rest,
        retype: typed_after,
    }
}

/// Stops an injection whose keys would land somewhere else than at its start marker (§4.6,
/// the 2.8 review's finding 1).
///
/// The service cancels an expansion when it hears of a focus change before the start marker
/// has come back (the hand-off). Once the marker is back, the injector is on its own: the
/// trigger key (a Tab, an Enter) may have moved the focus while the marker travelled, and a
/// `Wait` or a paste keeps the injection open for as long as they last. So each injector
/// notes where the keys go right before its start marker (`F`: window, process and, where the
/// OS can say it cheaply, the field) and asks the guard before each batch or step it sends.
///
/// # What `F` may cost
///
/// Reading `F` must never wait for the target application (a hung one would hold the
/// injection) and must take microseconds, not milliseconds: it is read once per batch, which
/// for most expansions means twice. The OS crates document what theirs holds.
///
/// # The injector's own moves
///
/// A step can move the focus itself: `{key tab}` in a form snippet, a line break that submits
/// a single-line field. The application handles it some time after it was sent, so from
/// then on a change the guard sees cannot be told from the injector's own. After output for
/// which [`InjectionStep::may_move_focus`] (or its OS-level equivalent) holds, the injector
/// calls [`own_move`](Self::own_move) and the guard stops checking for the rest of that
/// expansion.
///
/// ```
/// use iamb_platform::{CancelReason, FocusGuard};
///
/// let mut guard = FocusGuard::new(("editor", 7));
/// assert_eq!(guard.check(|| ("editor", 7)), Ok(()));
/// assert_eq!(guard.check(|| ("editor", 8)), Err(CancelReason::FocusMoved));
///
/// let mut guard = FocusGuard::new(("form", 1));
/// guard.own_move();
/// assert_eq!(guard.check(|| ("form", 2)), Ok(()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusGuard<F> {
    at_start: F,
    watching: bool,
}

impl<F: PartialEq> FocusGuard<F> {
    /// A guard for the focus `at_start`, read right before the start marker. "No focus" is a
    /// value like any other: a guard that started without one stops at the first focus.
    pub const fn new(at_start: F) -> Self {
        Self {
            at_start,
            watching: true,
        }
    }

    /// Before a batch or a step: reads the focus with `now` and answers
    /// [`FocusMoved`](CancelReason::FocusMoved) when it differs from the one at the start
    /// marker. After [`own_move`](Self::own_move) it reads nothing and answers `Ok`.
    ///
    /// # Errors
    ///
    /// [`CancelReason::FocusMoved`], with which the injector stops.
    pub fn check(&mut self, now: impl FnOnce() -> F) -> Result<(), CancelReason> {
        if !self.watching || now() == self.at_start {
            Ok(())
        } else {
            Err(CancelReason::FocusMoved)
        }
    }

    /// The output just sent may move the focus by itself; see the type's docs.
    pub const fn own_move(&mut self) {
        self.watching = false;
    }

    /// False once [`own_move`](Self::own_move) was called.
    #[must_use]
    pub const fn is_watching(&self) -> bool {
        self.watching
    }
}

/// How an injection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectionOutcome {
    /// Start marker, output and end marker were all posted. The listener reports
    /// `InjectionFinished` in stream order; the service records usage when it arrives.
    Finished {
        /// How many characters typed after the trigger were typed again.
        retyped: usize,
    },
    /// The expansion was not sent, or not to the end: no end marker was posted. For the
    /// reasons of the hand-off nothing but, at most, the start marker reached the OS; for
    /// [`FocusMoved`](CancelReason::FocusMoved) and
    /// [`ClipboardNotRestorable`](CancelReason::ClipboardNotRestorable) the steps before the
    /// stop did.
    Cancelled(CancelReason),
    /// The OS refused part of the output. Some of it may have reached the application; no end
    /// marker was posted.
    Failed(PlatformError),
}

impl InjectionOutcome {
    /// True when the service must send `Event::InjectionCancelled` to the engine: every
    /// outcome but [`Finished`](Self::Finished). After any outcome the service calls
    /// [`TypedHandoff::finish`].
    #[must_use]
    pub const fn needs_cancel_event(&self) -> bool {
        !matches!(self, Self::Finished { .. })
    }
}

/// Sends an expansion to the focused application.
///
/// # The protocol of one call (§4.6)
///
/// 1. If `typed` is already cancelled for this expansion, return
///    [`Cancelled`](InjectionOutcome::Cancelled) without sending anything.
/// 2. Note where the keys go ([`FocusGuard::new`]), then post the start marker: a no-op event
///    marked [`Marker::InjectionStart`](crate::Marker::InjectionStart).
/// 3. [`typed.wait_sealed`](TypedHandoff::wait_sealed) with
///    [`START_MARKER_TIMEOUT`](crate::START_MARKER_TIMEOUT). An error is a
///    [`Cancelled`](InjectionOutcome::Cancelled) outcome; no end marker follows.
/// 4. [`compensate`] the steps for the `k` characters it returned. Send the leading deletion,
///    the remaining steps, the `k` characters ([`typed.read`](TypedHandoff::read): copy them
///    into the input batch inside the closure, send outside it, zero the batch afterwards),
///    and the end marker ([`Marker::InjectionEnd`](crate::Marker::InjectionEnd)). Every output
///    event carries [`Marker::Output`](crate::Marker::Output). Where the OS can deliver a
///    batch without interleaving, everything up to a `Wait` or a `Paste` goes in one batch.
///    Before each batch (and, where every event is posted on its own, before each step) ask
///    the [`FocusGuard`]; when the focus has moved since the start marker, stop there with
///    [`Cancelled`](InjectionOutcome::Cancelled)`(`[`FocusMoved`](CancelReason::FocusMoved)`)`
///    and post no end marker.
/// 5. Return [`Finished`](InjectionOutcome::Finished). If the OS refuses part of the output,
///    stop, post no end marker and return [`Failed`](InjectionOutcome::Failed).
///
/// A paste never replaces clipboard content its snapshot could not save whole (§8 rule 8):
/// plain text is then typed with keys instead, and other content stops the expansion with
/// [`ClipboardNotRestorable`](CancelReason::ClipboardNotRestorable).
///
/// An expansion with no steps still gets both markers. The call blocks for the duration of
/// the injection, including `Wait` steps, so it runs on the worker's injector thread; one
/// expansion is in flight at a time.
///
/// A `Paste` step, and text the strategy sends by paste, use the OS crate's own clipboard
/// code: snapshot, transient write, paste shortcut, restore (§8 rule 8).
///
/// # Settings
///
/// [`configure`](Self::configure) takes the user's per-application settings in the format of
/// [`injection_settings`](crate::injection_settings) (ADR-0025). The service calls it at start
/// and whenever the setting changes; an injection that is already running finishes with the
/// settings it started with.
pub trait Injector: Send + Sync {
    /// Sends one expansion. See the trait docs for the protocol.
    fn inject(&self, request: &InjectionRequest, typed: &TypedHandoff) -> InjectionOutcome;

    /// Replaces the user's per-application settings with `settings_json`, the value of the
    /// [`INJECTION_SETTINGS_KEY`](crate::INJECTION_SETTINGS_KEY) setting (empty when it is not
    /// set). The OS crates read it with
    /// [`InjectionSettings::parse`](crate::InjectionSettings::parse) and layer it over their
    /// built-in strategy tables.
    ///
    /// The default is for an injector that takes no settings: it accepts the empty string and
    /// refuses anything else.
    ///
    /// # Errors
    ///
    /// [`PlatformError::Invalid`] naming what was wrong (never quoting the settings), or, from
    /// the default, [`PlatformError::Unsupported`]. On an error the settings in force stay as
    /// they were.
    fn configure(&self, settings_json: &str) -> error::Result<()> {
        if settings_json.is_empty() {
            Ok(())
        } else {
            Err(PlatformError::Unsupported)
        }
    }
}

#[cfg(test)]
mod tests {
    use iamb_core::engine::{Diagnostic, FormSessionId, Reason};
    use iamb_core::model::Sha256;

    use super::*;

    fn id(n: u8) -> Id {
        Id::from_bytes([n; 16])
    }

    fn text(s: &str) -> Action {
        Action::TypeText {
            text: s.to_owned(),
            verbatim: Vec::new(),
        }
    }

    fn expanded(n: u64) -> Action {
        Action::Expanded {
            expansion: ExpansionId::from_raw(n),
            snippet: id(1),
            chars_saved: 3,
        }
    }

    #[test]
    fn every_injection_action_becomes_a_step() {
        let cases = [
            (Action::DeleteBack(4), InjectionStep::DeleteBack(4)),
            (text("hi"), InjectionStep::TypeText("hi".to_owned())),
            (
                Action::Paste {
                    plain: "p".to_owned(),
                    markdown_html: "<p>p</p>".to_owned(),
                    attachments: vec![Sha256::from_bytes([7; 32])],
                },
                InjectionStep::Paste(ClipboardContent::rich("p", "<p>p</p>")),
            ),
            (
                Action::PressKey {
                    key: KeyName::Tab,
                    shift: true,
                    times: 2,
                },
                InjectionStep::PressKey {
                    key: KeyName::Tab,
                    shift: true,
                    times: 2,
                },
            ),
            (Action::MoveCaretBack(6), InjectionStep::MoveCaretBack(6)),
            (Action::Wait(250), InjectionStep::Wait(250)),
        ];
        for (action, step) in cases {
            assert_eq!(InjectionStep::from_action(action), Ok(step));
        }
    }

    #[test]
    fn other_actions_are_handed_back() {
        let others = [
            expanded(1),
            Action::Diagnostic(Diagnostic::new(Reason::FormOpen)),
            Action::ShowForm {
                session: FormSessionId::from_raw(1),
                snippet: id(2),
                spec: Vec::new(),
                last_values: None,
            },
        ];
        for action in others {
            assert_eq!(InjectionStep::from_action(action.clone()), Err(action));
        }
    }

    #[test]
    fn debug_shows_no_text() {
        let shown = format!("{:?}", InjectionStep::TypeText("my password".to_owned()));
        assert_eq!(shown, "TypeText { len: 11, .. }");
        let shown = format!(
            "{:?}",
            InjectionStep::Paste(ClipboardContent::text("my password"))
        );
        assert!(!shown.contains("password"));
        assert_eq!(
            format!("{:?}", InjectionStep::DeleteBack(3)),
            "DeleteBack(3)"
        );
        assert_eq!(
            format!("{:?}", InjectionStep::MoveCaretBack(2)),
            "MoveCaretBack(2)"
        );
        assert_eq!(format!("{:?}", InjectionStep::Wait(9)), "Wait(9)");
        assert_eq!(
            format!(
                "{:?}",
                InjectionStep::PressKey {
                    key: KeyName::Enter,
                    shift: false,
                    times: 1
                }
            ),
            "PressKey { key: Enter, shift: false, times: 1 }"
        );
    }

    #[test]
    fn split_groups_steps_under_their_expansion() {
        let diagnostic = Action::Diagnostic(Diagnostic::new(Reason::RequestCancelled));
        let (batches, other) = split_actions(vec![
            diagnostic.clone(),
            Action::DeleteBack(4),
            text("be right back"),
            text(" "),
            expanded(7),
        ]);
        assert_eq!(other, [diagnostic]);
        assert_eq!(
            batches,
            [ExpansionBatch {
                expansion: ExpansionId::from_raw(7),
                snippet: id(1),
                chars_saved: 3,
                steps: vec![
                    InjectionStep::DeleteBack(4),
                    InjectionStep::TypeText("be right back".to_owned()),
                    InjectionStep::TypeText(" ".to_owned()),
                ],
            }]
        );
        let request = InjectionRequest::for_batch(&batches[0]).in_app(AppId("editor".to_owned()));
        assert_eq!(request.expansion, ExpansionId::from_raw(7));
        assert_eq!(request.steps, batches[0].steps);
        assert_eq!(request.app, Some(AppId("editor".to_owned())));
        assert_eq!(request.overrides, InjectionOverrides::default());
    }

    #[test]
    fn split_handles_empty_expansions_several_and_none() {
        let (batches, other) = split_actions(vec![expanded(1), Action::Wait(5), expanded(2)]);
        assert!(other.is_empty());
        assert_eq!(batches.len(), 2);
        assert!(batches[0].steps.is_empty());
        assert_eq!(batches[1].steps, [InjectionStep::Wait(5)]);

        let (batches, other) = split_actions(Vec::new());
        assert!(batches.is_empty() && other.is_empty());

        let form = Action::ShowForm {
            session: FormSessionId::from_raw(3),
            snippet: id(2),
            spec: Vec::new(),
            last_values: None,
        };
        let (batches, other) = split_actions(vec![form.clone()]);
        assert!(batches.is_empty());
        assert_eq!(other, [form]);
    }

    #[test]
    fn steps_without_an_expanded_are_never_injected() {
        let (batches, other) = split_actions(vec![expanded(1), Action::DeleteBack(2), text("x")]);
        assert_eq!(batches.len(), 1);
        assert!(batches[0].steps.is_empty());
        assert_eq!(other, [Action::DeleteBack(2), text("x")]);
    }

    #[test]
    fn compensation_adds_the_typed_characters_to_the_deletion() {
        let steps = [
            InjectionStep::DeleteBack(4),
            InjectionStep::TypeText("be right back".to_owned()),
            InjectionStep::TypeText(" ".to_owned()),
        ];
        let plan = compensate(&steps, 2);
        assert_eq!(plan.delete_first, 6);
        assert_eq!(plan.rest, &steps[1..]);
        assert_eq!(plan.retype, 2);

        let plan = compensate(&steps, 0);
        assert_eq!((plan.delete_first, plan.retype), (4, 0));
        assert_eq!(plan.rest.len(), 2);
    }

    #[test]
    fn compensation_without_a_leading_deletion() {
        // A palette pick: nothing of its own to delete.
        let steps = [InjectionStep::TypeText("hello".to_owned())];
        let plan = compensate(&steps, 3);
        assert_eq!(plan.delete_first, 3);
        assert_eq!(plan.rest, &steps[..]);
        assert_eq!(plan.retype, 3);

        // Only a leading DeleteBack is the expansion's own deletion.
        let steps = [
            InjectionStep::TypeText("a".to_owned()),
            InjectionStep::DeleteBack(1),
        ];
        let plan = compensate(&steps, 1);
        assert_eq!(plan.delete_first, 1);
        assert_eq!(plan.rest.len(), 2);

        let plan = compensate(&[], 0);
        assert_eq!((plan.delete_first, plan.retype), (0, 0));
        assert!(plan.rest.is_empty());
    }

    #[test]
    fn the_guard_stops_at_a_moved_focus_and_reads_nothing_after_an_own_move() {
        let mut guard = FocusGuard::new(Some((100_u32, 7_u64)));
        assert!(guard.is_watching());
        assert_eq!(guard.check(|| Some((100, 7))), Ok(()));
        // Another field of the same window, another window, no focus at all.
        for moved in [Some((100, 8)), Some((200, 7)), None] {
            assert_eq!(guard.check(|| moved), Err(CancelReason::FocusMoved));
        }
        // A guard that started without a focus stops when one appears.
        let mut guard = FocusGuard::new(None::<u32>);
        assert_eq!(guard.check(|| None), Ok(()));
        assert_eq!(guard.check(|| Some(1)), Err(CancelReason::FocusMoved));
        // After the injector's own Tab nothing is compared, and nothing is read.
        let mut guard = FocusGuard::new(1);
        guard.own_move();
        assert!(!guard.is_watching());
        assert_eq!(
            guard.check(|| unreachable!("no read after an own move")),
            Ok(())
        );
    }

    #[test]
    fn steps_that_may_move_the_focus() {
        let moving = [
            InjectionStep::PressKey {
                key: KeyName::Tab,
                shift: false,
                times: 1,
            },
            InjectionStep::PressKey {
                key: KeyName::Enter,
                shift: true,
                times: 1,
            },
            InjectionStep::PressKey {
                key: KeyName::Down,
                shift: false,
                times: 2,
            },
            InjectionStep::TypeText("line\nnext".to_owned()),
            InjectionStep::TypeText("a\rb".to_owned()),
            InjectionStep::TypeText("name\tpass".to_owned()),
        ];
        for step in &moving {
            assert!(step.may_move_focus(), "{step:?}");
        }
        let staying = [
            InjectionStep::DeleteBack(3),
            InjectionStep::MoveCaretBack(2),
            InjectionStep::Wait(10),
            InjectionStep::TypeText("be right back".to_owned()),
            InjectionStep::Paste(ClipboardContent::text("a\nb")),
            InjectionStep::PressKey {
                key: KeyName::Backspace,
                shift: false,
                times: 1,
            },
            InjectionStep::PressKey {
                key: KeyName::Left,
                shift: true,
                times: 1,
            },
            InjectionStep::PressKey {
                key: KeyName::Delete,
                shift: false,
                times: 1,
            },
        ];
        for step in &staying {
            assert!(!step.may_move_focus(), "{step:?}");
        }
    }

    #[test]
    fn only_finished_needs_no_cancel_event() {
        assert!(!InjectionOutcome::Finished { retyped: 0 }.needs_cancel_event());
        assert!(InjectionOutcome::Cancelled(CancelReason::Boundary).needs_cancel_event());
        assert!(InjectionOutcome::Failed(PlatformError::InjectionRefused).needs_cancel_event());
    }

    #[test]
    fn overrides_default_to_the_platform_table() {
        let mut overrides = InjectionOverrides::default();
        assert_eq!(overrides.text_method, None);
        assert_eq!(overrides.key_delay_ms, None);
        overrides.text_method = Some(TextMethod::Paste);
        let mut request = InjectionRequest::new(ExpansionId::from_raw(1), Vec::new());
        request.overrides = overrides;
        assert_eq!(request.overrides.text_method, Some(TextMethod::Paste));
        assert_eq!(request.app, None);
    }
}
