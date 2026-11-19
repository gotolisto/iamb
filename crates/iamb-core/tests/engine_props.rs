//! Property tests of the engine: random event streams against random small libraries never
//! panic, never delete more than was typed since the buffer was last cleared, and never act on
//! typed keys while suspended.

#[path = "../../iamb-testkit/src/env.rs"]
#[allow(
    dead_code,
    reason = "shared with the testkit crate; not every helper is used here"
)]
mod env;

use std::collections::HashMap;

use env::FakeEnv;
use iamb_core::engine::{
    Action, AppId, BoundaryKind, Engine, EngineSettings, Event, ExpansionId, FormSessionId,
    InjectionState, SecureState, Snapshot, dry_run,
};
use iamb_core::model::{Abbreviation, CaseMode, Id, Snippet, SyncMeta, Timestamp, Trigger};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::FormValues;
use proptest::prelude::*;

const BODIES: [&str; 8] = [
    "x",
    "Hello world",
    "{cursor}()",
    r#"Hi {field name="N"}!"#,
    "{clipboard}",
    r#"{date shift="+9999y"}"#,
    "{key name=tab}a{wait ms=5}",
    "{bad",
];

#[derive(Debug, Clone)]
struct Spec {
    abbr: String,
    body: usize,
    immediate: bool,
    case: u8,
    word_start: bool,
    enabled: bool,
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        "[abA]{1,3}",
        0..BODIES.len(),
        any::<bool>(),
        0u8..3,
        any::<bool>(),
        prop::bool::weighted(0.85),
    )
        .prop_map(|(abbr, body, immediate, case, word_start, enabled)| Spec {
            abbr,
            body,
            immediate,
            case,
            word_start,
            enabled,
        })
}

fn build(specs: &[Spec]) -> Snapshot {
    let snippets = specs.iter().zip(1u8..).map(|(s, n)| {
        let mut snippet = Snippet::new_defaults(
            SyncMeta::new(Id::from_bytes([n; 16]), Timestamp(0)),
            Id::from_bytes([0; 16]),
            Abbreviation::new(&s.abbr).expect("generated abbreviations are valid"),
            "",
            BODIES[s.body],
            None,
        );
        snippet.trigger = if s.immediate {
            Trigger::Immediate
        } else {
            Trigger::Delimiter
        };
        snippet.case_mode =
            [CaseMode::Exact, CaseMode::Insensitive, CaseMode::Adaptive][usize::from(s.case)];
        snippet.word_start = s.word_start;
        snippet.enabled = s.enabled;
        snippet
    });
    Snapshot::build(snippets, EngineSettings::default())
}

/// A generated event; ids are resolved against what the engine emitted so far.
#[derive(Debug, Clone)]
enum Op {
    Char(char),
    Backspace,
    Boundary(u8),
    /// A focus change to application 0 or 1.
    Focus(u8),
    Secure(u8),
    Pause(bool),
    Compose(bool),
    Overflow,
    /// 0 start, 1 finish, 2 cancel; `true` targets the last expansion, else a raw id.
    Inject(u8, bool, u64),
    /// `true` targets the last session, else a raw id.
    Submit(bool, u64),
    Cancel(bool, u64),
    Expand(u8),
    Reopen,
    Snapshot,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        12 => prop::sample::select(vec!['a', 'b', 'A', 'B', ' ', '.', 'x']).prop_map(Op::Char),
        2 => Just(Op::Backspace),
        2 => (0u8..8).prop_map(Op::Boundary),
        2 => (0u8..2).prop_map(Op::Focus),
        2 => (0u8..5).prop_map(Op::Secure),
        1 => any::<bool>().prop_map(Op::Pause),
        1 => any::<bool>().prop_map(Op::Compose),
        1 => Just(Op::Overflow),
        4 => (0u8..3, prop::bool::weighted(0.8), 0u64..4).prop_map(|(k, l, r)| Op::Inject(k, l, r)),
        2 => (prop::bool::weighted(0.8), 0u64..4).prop_map(|(l, r)| Op::Submit(l, r)),
        1 => (prop::bool::weighted(0.8), 0u64..4).prop_map(|(l, r)| Op::Cancel(l, r)),
        1 => (0u8..6).prop_map(Op::Expand),
        1 => Just(Op::Reopen),
        1 => Just(Op::Snapshot),
    ]
}

const KINDS: [BoundaryKind; 8] = [
    BoundaryKind::Enter,
    BoundaryKind::Tab,
    BoundaryKind::Escape,
    BoundaryKind::Arrow,
    BoundaryKind::Navigation,
    BoundaryKind::Click,
    BoundaryKind::Shortcut,
    BoundaryKind::Other,
];
const STATES: [SecureState; 5] = [
    SecureState::Normal,
    SecureState::Secure,
    SecureState::Excluded,
    SecureState::OwnWindow,
    SecureState::Unknown,
];

/// Whose a held request is (review 1.9 follow-up N4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Owner {
    /// No request is held.
    #[default]
    Nothing,
    /// The next `FocusChanged` decides.
    Unbound,
    App(String),
}

/// What the test knows independently of the engine.
#[derive(Default)]
struct Model {
    /// The focus verdict: `None` after `FocusChanged` (or at start) and before the next verdict,
    /// when explicit requests wait; `Some(true)` for `Normal`.
    verdict: Option<bool>,
    /// The form session of a submit the engine holds for the verdict, if any.
    held_submit: Option<FormSessionId>,
    /// The open form session: opened by `ShowForm`, closed by the first submit or cancel that
    /// names it.
    open_form: Option<FormSessionId>,
    /// The application of the last `FocusChanged`.
    app: Option<String>,
    /// The application each form session was opened for.
    form_app: HashMap<FormSessionId, Option<String>>,
    /// The application of the request the engine holds.
    held_app: Owner,
    paused: bool,
    composing: bool,
    /// Typed characters (including Enter and Tab, when they are delimiters) since the last event that
    /// certainly cleared the buffer: an upper bound of what the buffer holds.
    typed: usize,
    /// `typed` when each form session was opened.
    at_form: HashMap<FormSessionId, usize>,
    last_session: Option<FormSessionId>,
    last_expansion: Option<ExpansionId>,
}

/// One generated event, translated.
struct Translated {
    event: Event,
    /// A typed key (`Char`, `Backspace`, `Boundary`).
    is_key: bool,
    /// Upper bound of typed characters a `DeleteBack` caused by this event may remove.
    bound: usize,
    /// The session a `FormSubmitted` answers (an open one, or a held submit's on a verdict).
    submitted: Option<FormSessionId>,
    ask: Ask,
}

/// What a generated event is as far as explicit requests go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// Neither a request nor a stale form answer.
    Other,
    /// A palette pick or a reopen.
    Request,
    /// A submit of the open form.
    Submit,
    /// A submit for a session that is not open, or a cancel for one that is neither open nor
    /// held as a submit: not a request (engine module docs, "Explicit requests"), so it must
    /// change nothing.
    Stale,
}

impl Model {
    fn suspended(&self) -> bool {
        self.verdict != Some(true) || self.paused || self.composing
    }

    /// Whether an explicit request arriving now is held for the verdict.
    fn holds(&self) -> bool {
        self.verdict.is_none() && !self.composing
    }

    /// The session a form answer names: the last one shown, or a raw id.
    fn session(&self, last: bool, raw: u64) -> FormSessionId {
        match (last, self.last_session) {
            (true, Some(s)) => s,
            _ => FormSessionId::from_raw(raw),
        }
    }

    /// Checks that a held request is dropped when the focus moves to another application and
    /// runs only in its own, and that a submit types only where its form was opened (review
    /// 1.9 follow-up N4); then records whose request the engine now holds.
    fn check_owner(
        &mut self,
        engine: &Engine,
        types: bool,
        (is_request, is_submit, is_verdict): (bool, bool, bool),
        focus_to: Option<String>,
        held_app: &Owner,
        submitted: Option<FormSessionId>,
    ) -> Result<(), TestCaseError> {
        if let (Some(to), Owner::App(owner)) = (&focus_to, held_app)
            && to != owner
        {
            prop_assert!(!engine.holds_request(), "held across a move to {}", to);
        }
        if is_verdict
            && types
            && let Owner::App(owner) = held_app
        {
            prop_assert_eq!(
                Some(owner),
                self.app.as_ref(),
                "a held request ran elsewhere"
            );
        }
        let form_app = submitted.and_then(|s| self.form_app.get(&s).cloned().flatten());
        if is_submit
            && types
            && let Some(owner) = &form_app
        {
            prop_assert_eq!(Some(owner), self.app.as_ref(), "a submit ran elsewhere");
        }
        if !engine.holds_request() {
            self.held_app = Owner::Nothing;
            self.held_submit = None;
        } else if is_request {
            self.held_app = form_app
                .or_else(|| self.app.clone())
                .map_or(Owner::Unbound, Owner::App);
            self.held_submit = submitted.filter(|_| is_submit);
        } else if let (Some(to), Owner::Unbound) = (focus_to, held_app) {
            self.held_app = Owner::App(to);
        }
        Ok(())
    }

    /// An explicit request other than a submit: if held, it replaces a held submit.
    fn request(&mut self) {
        if self.holds() {
            self.held_submit = None;
        }
    }

    /// Turns `op` into an event and updates what the model knows before it is handled.
    fn translate(&mut self, op: &Op, snapshot: &Snapshot) -> Translated {
        let suspended = self.suspended();
        let m = self;
        let mut is_key = false;
        let mut bound = m.typed;
        let mut submitted = None;
        let mut ask = Ask::Other;
        let event = match op {
            Op::Char(c) => {
                is_key = true;
                if !suspended {
                    m.typed += 1;
                }
                bound = m.typed;
                Event::char(*c, 0)
            }
            Op::Backspace => {
                is_key = true;
                Event::Backspace { seq: 0 }
            }
            Op::Boundary(k) => {
                is_key = true;
                let kind = KINDS[usize::from(*k)];
                if !suspended && kind.as_delimiter().is_some() {
                    bound += 1;
                }
                m.typed = 0;
                Event::Boundary { kind, seq: 0 }
            }
            Op::Focus(app) => {
                m.typed = 0;
                m.verdict = None;
                let app = format!("app{app}");
                m.app = Some(app.clone());
                Event::FocusChanged(AppId(app))
            }
            Op::Secure(s) => {
                m.typed = 0;
                let state = STATES[usize::from(*s)];
                m.verdict = Some(state == SecureState::Normal);
                // A held submit runs (or is refused) now.
                submitted = m.held_submit.take();
                Event::SecureState(state)
            }
            Op::Pause(on) => {
                m.typed = 0;
                m.paused = *on;
                Event::Paused(*on)
            }
            Op::Compose(on) => {
                m.typed = 0;
                m.composing = *on;
                Event::Composing(*on)
            }
            Op::Overflow => {
                m.typed = 0;
                m.held_submit = None;
                Event::Overflow
            }
            Op::Inject(kind, last, raw) => m.inject(*kind, *last, *raw),
            Op::Submit(last, raw) => {
                let session = m.session(*last, *raw);
                ask = m.submit(session);
                submitted = (ask == Ask::Submit).then_some(session);
                let values: FormValues = [("N", "v")].into_iter().collect();
                Event::FormSubmitted {
                    session,
                    values: Sensitive::new(values),
                }
            }
            Op::Cancel(last, raw) => {
                let session = m.session(*last, *raw);
                ask = m.cancel(session);
                Event::FormCancelled { session }
            }
            Op::Expand(i) => {
                ask = Ask::Request;
                m.request();
                Event::ExpandRequest {
                    snippet: Id::from_bytes([*i; 16]),
                }
            }
            Op::Reopen => {
                ask = Ask::Request;
                m.request();
                Event::ReopenLastForm
            }
            Op::Snapshot => Event::SnapshotChanged(snapshot.clone()),
        };
        Translated {
            event,
            is_key,
            bound,
            submitted,
            ask,
        }
    }

    /// An injector event for the last expansion, or a raw id.
    fn inject(&self, kind: u8, last: bool, raw: u64) -> Event {
        let expansion = match (last, self.last_expansion) {
            (true, Some(e)) => e,
            _ => ExpansionId::from_raw(raw),
        };
        match kind {
            0 => Event::InjectionStarted { expansion, seq: 0 },
            1 => Event::InjectionFinished { expansion, seq: 0 },
            _ => Event::InjectionCancelled { expansion },
        }
    }

    /// A submit for `session`: only the submit of the open form is a request. An answer for a
    /// session that was already answered or never shown is dropped and leaves a held request
    /// (which belongs to its own application) alone.
    fn submit(&mut self, session: FormSessionId) -> Ask {
        if self.open_form == Some(session) {
            self.open_form = None;
            Ask::Submit
        } else {
            Ask::Stale
        }
    }

    /// A cancel for `session`: it closes the open form or drops a held submit of that session;
    /// for any other session it changes nothing.
    fn cancel(&mut self, session: FormSessionId) -> Ask {
        if self.open_form == Some(session) {
            self.open_form = None;
            Ask::Other
        } else if self.held_submit == Some(session) {
            Ask::Other
        } else {
            Ask::Stale
        }
    }
}

fn check(
    specs: &[Spec],
    start_normal: bool,
    ops: &[Op],
    clipboard: Option<String>,
) -> Result<(), TestCaseError> {
    let snapshot = build(specs);
    let env = FakeEnv::default();
    env.set_clipboard(clipboard);
    let mut engine = Engine::new(snapshot.clone());
    let mut m = Model::default();
    if start_normal {
        engine.handle(Event::SecureState(SecureState::Normal), &env);
        m.verdict = Some(true);
    }
    for op in ops {
        let suspended = m.suspended();
        prop_assert_eq!(engine.is_suspended(), suspended);
        let Translated {
            event,
            is_key,
            bound,
            submitted,
            ask,
        } = m.translate(op, &snapshot);
        let is_submit = ask == Ask::Submit;
        let is_request = matches!(ask, Ask::Request | Ask::Submit);
        let typed_before_event = bound;
        let held_before = engine.holds_request();
        let focus_to = match &event {
            Event::FocusChanged(app) => Some(app.0.clone()),
            _ => None,
        };
        let is_verdict = matches!(event, Event::SecureState(_));
        let held_app = m.held_app.clone();
        let actions = engine.handle(event, &env);
        if is_key && suspended {
            prop_assert!(actions.is_empty(), "actions while suspended: {:?}", actions);
            prop_assert_eq!(engine.buffered_len(), 0);
        }
        let types = actions.iter().any(|a| !matches!(a, Action::Diagnostic(_)));
        // A form answer for a session that is neither open nor held is not a request: it types
        // nothing and neither replaces nor drops a held request.
        if ask == Ask::Stale {
            prop_assert!(!types, "a stale form answer typed: {:?}", actions);
            prop_assert_eq!(engine.holds_request(), held_before);
        }
        m.check_owner(
            &engine,
            types,
            (is_request, is_submit, is_verdict),
            focus_to,
            &held_app,
            submitted,
        )?;
        // Explicit requests never type into a field not confirmed as normal (review 1.9
        // finding 5): any typing action needs the verdict `Normal` and no composition.
        if types {
            prop_assert!(
                m.verdict == Some(true) && !m.composing,
                "typing actions without a normal verdict: {:?}",
                actions
            );
        }
        // Only a pending verdict holds a request.
        prop_assert!(!engine.holds_request() || m.verdict.is_none());
        for action in &actions {
            match action {
                Action::DeleteBack(n) => {
                    let limit = match submitted {
                        Some(s) => m.at_form.get(&s).copied().unwrap_or(0),
                        None => typed_before_event,
                    };
                    prop_assert!(*n >= 1);
                    prop_assert!(*n <= limit, "DeleteBack({}) with {} typed", n, limit);
                }
                Action::ShowForm { session, .. } => {
                    m.at_form.insert(*session, typed_before_event);
                    m.form_app.insert(*session, m.app.clone());
                    m.last_session = Some(*session);
                    m.open_form = Some(*session);
                    m.typed = 0;
                }
                Action::Expanded { expansion, .. } => {
                    m.last_expansion = Some(*expansion);
                    m.typed = 0;
                }
                Action::Diagnostic(d) => {
                    // Diagnostics never carry typed text; they name a snippet only for one the
                    // library or a request named.
                    if let Some(s) = d.snippet {
                        prop_assert!(s.as_bytes()[0] < 7);
                    }
                }
                _ => {}
            }
        }
        prop_assert_eq!(engine.open_form(), m.open_form);
        // A pending or injecting expansion must have been announced.
        if let InjectionState::Pending(e) | InjectionState::Injecting(e) = engine.injection_state()
        {
            prop_assert_eq!(Some(e), m.last_expansion);
        }
        prop_assert!(env.now_calls() <= ops.len());
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_streams_hold_the_invariants(
        specs in prop::collection::vec(spec(), 0..5),
        start_normal in prop::bool::weighted(0.8),
        ops in prop::collection::vec(op(), 0..80),
        clipboard in prop::option::of("[a-zA-Z ]{0,8}"),
    ) {
        check(&specs, start_normal, &ops, clipboard)?;
    }

    #[test]
    fn dry_run_never_panics_and_deletes_only_typed(
        specs in prop::collection::vec(spec(), 0..5),
        typed in "[abAB .x]{0,40}",
        submit in any::<bool>(),
    ) {
        let snapshot = build(&specs);
        let env = FakeEnv::default();
        let values: FormValues = [("N", "v")].into_iter().collect();
        let actions = dry_run(&snapshot, &typed, &env, submit.then_some(&values));
        let total: usize = actions
            .iter()
            .filter_map(|a| match a { Action::DeleteBack(n) => Some(*n), _ => None })
            .sum();
        prop_assert!(total <= typed.chars().count());
    }
}
