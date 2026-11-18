//! The mock platform end to end with the real engine, on the threads of architecture §3.1:
//! mock listener → bounded channel → engine thread → injector thread → mock text field.
//!
//! The `Worker` below is the smallest service that keeps the contracts of `iamb-platform`:
//! the engine thread feeds the typed-characters hand-off with exactly what it feeds the engine
//! (see `TypedHandoff`), arms it when the engine returns an expansion, and answers every
//! expansion with the injector's markers or `InjectionCancelled`. Task 2.6 builds the real one.

use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::{
    Action, Engine, EngineSettings, Environment, Event, ExpansionId, InjectionState, Reason,
    SecureState, Snapshot, SuspendReason,
};
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp, Trigger};
use iamb_core::template::KeyName;
use iamb_platform::mock::{Injected, InjectionPoint, MockKey, MockPlatform, MockWindow};
use iamb_platform::{
    CancelReason, Chord, ClipboardAccess, ClipboardContent, FieldSecurity, FocusChange,
    FocusInspector, FocusToken, Grant, HandoffStatus, HotkeyId, Hotkeys, InjectionOutcome,
    InjectionRequest, Injector, KeyListener, ListenerEvent, MemoryLock, OverflowSink,
    PermissionKind, PlatformError, Routed, SECURE_STATE_TIMEOUT, TypedHandoff, UnknownReason,
    split_actions, verdict,
};

// ---------------------------------------------------------------------------------------------
// A minimal worker
// ---------------------------------------------------------------------------------------------

enum Msg {
    Key(ListenerEvent),
    Focus(FocusChange),
    Request(Id),
    Injected(ExpansionId, InjectionOutcome),
    /// Answered at once: everything sent before it has been handled.
    Sync(Sender<()>),
    /// Answered when no injection is in flight.
    Settle(Sender<()>),
    Stop,
}

/// What the engine thread saw, for assertions.
#[derive(Default)]
struct Report {
    diagnostics: Vec<Reason>,
    outcomes: Vec<InjectionOutcome>,
    /// Snippets whose injection finished: what the service records as usage.
    used: Vec<Id>,
    hotkeys: Vec<HotkeyId>,
    verdicts: Vec<SecureState>,
    final_state: Option<InjectionState>,
}

/// The clock is fixed; the clipboard is the platform's.
struct Env(MockPlatform);

impl Environment for Env {
    fn now(&self) -> LocalDateTime {
        LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("valid date")
    }

    fn clipboard_text(&self) -> Option<String> {
        self.0
            .read_text()
            .ok()
            .flatten()
            .map(|t| t.expose().clone())
    }
}

/// Lets a test hold the injector thread back, to put events between a trigger and the
/// injection.
#[derive(Default)]
struct Gate {
    closed: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn set_closed(&self, closed: bool) {
        *self.closed.lock().unwrap_or_else(PoisonError::into_inner) = closed;
        self.opened.notify_all();
    }

    fn pass(&self) {
        let guard = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
        drop(
            self.opened
                .wait_while(guard, |closed| *closed)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

struct EngineThread {
    os: MockPlatform,
    engine: Engine,
    handoff: Arc<TypedHandoff>,
    to_injector: Sender<InjectionRequest>,
    in_flight: usize,
    waiting: Vec<Sender<()>>,
    snippets: Vec<(ExpansionId, Id)>,
    report: Report,
}

impl EngineThread {
    fn handle(&mut self, event: Event) {
        let env = Env(self.os.clone());
        let actions = self.engine.handle(event, &env);
        let (batches, other) = split_actions(actions);
        for action in other {
            if let Action::Diagnostic(d) = action {
                self.report.diagnostics.push(d.reason);
            }
        }
        for batch in batches {
            // Arm before the injector can run: it must find the hand-off capturing.
            self.handoff.arm(batch.expansion);
            self.snippets.push((batch.expansion, batch.snippet));
            self.in_flight += 1;
            self.to_injector
                .send(InjectionRequest::for_batch(&batch))
                .expect("injector thread runs");
        }
    }

    fn run(mut self, rx: &Receiver<Msg>) -> Report {
        for msg in rx {
            match msg {
                Msg::Key(event) => {
                    // The hand-off sees every listener event, before the engine does.
                    self.handoff.observe(&event);
                    match event.route() {
                        Routed::Engine(event) => self.handle(event),
                        Routed::Hotkey(id) => self.report.hotkeys.push(id),
                    }
                }
                Msg::Focus(change) => {
                    // Anything else that clears the engine's buffer disturbs a capture.
                    self.handoff.disturb();
                    self.handle(Event::FocusChanged(change.app.app.clone()));
                    let field = self.os.secure_state(&change.token, SECURE_STATE_TIMEOUT);
                    let state = verdict(&change.app, field, false, false);
                    self.report.verdicts.push(state);
                    self.handoff.disturb();
                    self.handle(Event::SecureState(state));
                }
                Msg::Request(snippet) => self.handle(Event::ExpandRequest { snippet }),
                Msg::Injected(expansion, outcome) => {
                    self.handoff.finish(expansion);
                    if outcome.needs_cancel_event() {
                        self.handle(Event::InjectionCancelled { expansion });
                    } else if let Some((_, snippet)) =
                        self.snippets.iter().find(|(e, _)| *e == expansion)
                    {
                        self.report.used.push(*snippet);
                    }
                    self.report.outcomes.push(outcome);
                    self.in_flight -= 1;
                    if self.in_flight == 0 {
                        for waiter in self.waiting.drain(..) {
                            waiter.send(()).expect("test waits");
                        }
                    }
                }
                Msg::Sync(done) => done.send(()).expect("test waits"),
                Msg::Settle(done) if self.in_flight == 0 => done.send(()).expect("test waits"),
                Msg::Settle(done) => self.waiting.push(done),
                Msg::Stop => break,
            }
        }
        self.report.final_state = Some(self.engine.injection_state());
        self.report
    }
}

struct Worker {
    os: MockPlatform,
    tx: SyncSender<Msg>,
    handoff: Arc<TypedHandoff>,
    gate: Arc<Gate>,
    engine: JoinHandle<Report>,
    injector: JoinHandle<()>,
}

impl Worker {
    fn start(os: &MockPlatform, snippets: Vec<Snippet>) -> Self {
        let (tx, rx) = sync_channel::<Msg>(256);
        let (to_injector, injector_rx) = channel::<InjectionRequest>();
        let handoff = Arc::new(TypedHandoff::new());
        os.lock(handoff.region()).expect("region locks");
        let gate = Arc::new(Gate::default());

        // Listener → engine: never blocks; a full channel is announced as Overflow.
        let keys = tx.clone();
        os.start(Box::new(OverflowSink::new(move |event| {
            keys.try_send(Msg::Key(event)).map_err(|e| match e {
                TrySendError::Full(Msg::Key(event))
                | TrySendError::Disconnected(Msg::Key(event)) => event,
                _ => ListenerEvent::Overflow,
            })
        })))
        .expect("listener starts");

        let focus = tx.clone();
        os.watch(Box::new(move |change: FocusChange| {
            focus.send(Msg::Focus(change)).expect("engine thread runs");
        }))
        .expect("focus watcher starts");

        let engine_thread = EngineThread {
            os: os.clone(),
            engine: Engine::new(Snapshot::build(snippets, EngineSettings::default())),
            handoff: Arc::clone(&handoff),
            to_injector,
            in_flight: 0,
            waiting: Vec::new(),
            snippets: Vec::new(),
            report: Report::default(),
        };
        let engine = thread::spawn(move || engine_thread.run(&rx));

        let injector = {
            let os = os.clone();
            let handoff = Arc::clone(&handoff);
            let gate = Arc::clone(&gate);
            let done = tx.clone();
            thread::spawn(move || {
                for request in injector_rx {
                    gate.pass();
                    let outcome = os.inject(&request, &handoff);
                    if done
                        .send(Msg::Injected(request.expansion, outcome))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        };

        Self {
            os: os.clone(),
            tx,
            handoff,
            gate,
            engine,
            injector,
        }
    }

    fn ask(&self, msg: impl FnOnce(Sender<()>) -> Msg) {
        let (done, answered) = channel();
        self.tx.send(msg(done)).expect("engine thread runs");
        answered.recv().expect("engine thread answers");
    }

    /// Waits until the engine has handled everything sent so far.
    fn sync(&self) {
        self.ask(Msg::Sync);
    }

    /// Waits until everything typed so far is handled and no injection is in flight.
    fn settle(&self) {
        self.ask(Msg::Settle);
    }

    fn request(&self, snippet: Id) {
        self.tx
            .send(Msg::Request(snippet))
            .expect("engine thread runs");
    }

    fn stop(self) -> Report {
        self.settle();
        self.os.stop().expect("listener stops");
        self.os.unwatch().expect("watcher stops");
        self.tx.send(Msg::Stop).expect("engine thread runs");
        let report = self.engine.join().expect("engine thread did not panic");
        self.injector.join().expect("injector thread did not panic");
        assert_eq!(
            self.handoff.status(),
            HandoffStatus::Idle,
            "nothing is held after the last expansion"
        );
        assert!(self.handoff.is_empty());
        report
    }
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

fn snippet(n: u8, abbr: &str, body: &str) -> Snippet {
    Snippet::new_defaults(
        SyncMeta::new(id(n), Timestamp(0)),
        id(0),
        Abbreviation::new(abbr).expect("abbreviation"),
        "",
        body,
        None,
    )
}

fn library() -> Vec<Snippet> {
    let mut immediate = snippet(4, ";sig", "Kind regards");
    immediate.trigger = Trigger::Immediate;
    immediate.word_start = false;
    vec![
        snippet(1, "brb", "be right back"),
        snippet(2, "omw", "on my way"),
        snippet(3, "greet", "Hi {cursor}, how are you?"),
        immediate,
        snippet(5, "clip", "[{clipboard}]"),
        snippet(6, "addr", "1 Main Street{key name=enter}Springfield"),
    ]
}

/// A machine with a focused editor window and a running worker.
fn setup() -> (MockPlatform, FocusToken, Worker) {
    let os = MockPlatform::new();
    let editor = os.open_window(MockWindow::new("org.example.editor", 100));
    os.focus(editor).expect("window is open");
    let worker = Worker::start(&os, library());
    worker.sync();
    (os, editor, worker)
}

fn text(os: &MockPlatform, window: FocusToken) -> String {
    os.text(window).expect("window is open")
}

const BRB_STEPS: usize = 5; // start marker, delete, text, delimiter, end marker

// ---------------------------------------------------------------------------------------------
// Plain expansion
// ---------------------------------------------------------------------------------------------

#[test]
fn typing_an_abbreviation_replaces_it_in_the_field() {
    let (os, editor, worker) = setup();
    os.type_text("see you, brb ");
    worker.settle();

    assert_eq!(text(&os, editor), "see you, be right back ");
    let log = os.injected();
    assert_eq!(log.len(), BRB_STEPS);
    assert_eq!(log[1], Injected::DeleteBack(4));
    assert_eq!(log[2], Injected::TypeText("be right back".to_owned()));
    assert_eq!(log[3], Injected::TypeText(" ".to_owned()));

    let report = worker.stop();
    assert_eq!(report.outcomes, [InjectionOutcome::Finished { retyped: 0 }]);
    assert_eq!(report.used, [id(1)]);
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.verdicts, [SecureState::Normal]);
    assert_eq!(report.final_state, Some(InjectionState::Idle));
}

#[test]
fn the_typed_characters_region_is_locked() {
    let (os, _editor, worker) = setup();
    assert_eq!(os.locked_regions(), [worker.handoff.region()]);
    worker.stop();
}

#[test]
fn expansions_follow_each_other() {
    let (os, editor, worker) = setup();
    let mut expected = String::new();
    for round in 0..50 {
        os.type_text(if round % 2 == 0 { "brb " } else { "omw." });
        worker.settle();
        expected.push_str(if round % 2 == 0 {
            "be right back "
        } else {
            "on my way."
        });
    }
    assert_eq!(text(&os, editor), expected);
    let report = worker.stop();
    assert_eq!(report.outcomes.len(), 50);
    assert_eq!(report.used.len(), 50);
    assert!(report.diagnostics.is_empty());
}

#[test]
fn an_immediate_snippet_fires_on_its_last_character() {
    let (os, editor, worker) = setup();
    os.type_text("Thanks.\n;sig");
    worker.settle();
    assert_eq!(text(&os, editor), "Thanks.\nKind regards");
    assert_eq!(os.injected()[1], Injected::DeleteBack(4));
    worker.stop();
}

#[test]
fn enter_as_the_delimiter_is_pressed_again_as_a_key() {
    let (os, editor, worker) = setup();
    os.type_text("brb\n");
    worker.settle();
    assert_eq!(text(&os, editor), "be right back\n");
    assert_eq!(
        os.injected()[3],
        Injected::PressKey {
            key: KeyName::Enter,
            shift: false,
            times: 1
        }
    );
    worker.stop();
}

#[test]
fn a_key_token_presses_a_real_key() {
    let (os, editor, worker) = setup();
    os.type_text("addr ");
    worker.settle();
    assert_eq!(text(&os, editor), "1 Main Street\nSpringfield ");
    worker.stop();
}

#[test]
fn a_cursor_snippet_leaves_the_caret_inside_the_text() {
    let (os, editor, worker) = setup();
    os.type_text("greet ");
    worker.settle();
    assert_eq!(text(&os, editor), "Hi , how are you?");
    assert_eq!(os.caret(editor), Some(3));
    os.type_text("Ana");
    assert_eq!(text(&os, editor), "Hi Ana, how are you?");
    worker.stop();
}

#[test]
fn the_clipboard_token_reads_the_platform_clipboard() {
    let (os, editor, worker) = setup();
    os.copy(ClipboardContent::text("copied"));
    os.type_text("clip ");
    worker.settle();
    assert_eq!(text(&os, editor), "[copied] ");
    assert_eq!(os.clipboard(), ClipboardContent::text("copied"));
    worker.stop();
}

#[test]
fn a_palette_pick_types_at_the_caret_without_deleting() {
    let (os, editor, worker) = setup();
    os.type_text("Note: ");
    worker.request(id(2));
    worker.settle();
    assert_eq!(text(&os, editor), "Note: on my way");
    assert_eq!(
        os.injected()[1],
        Injected::TypeText("on my way".to_owned()),
        "no deletion for an explicit request"
    );
    let report = worker.stop();
    assert_eq!(report.used, [id(2)]);
}

// ---------------------------------------------------------------------------------------------
// Typing while Iamb is injecting (architecture §4.6)
// ---------------------------------------------------------------------------------------------

#[test]
fn keys_typed_before_the_start_marker_are_deleted_and_typed_again() {
    let (os, editor, worker) = setup();
    // The typist is already on the next word when the injector starts.
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("om"));
    os.type_text("brb ");
    worker.settle();

    assert_eq!(text(&os, editor), "be right back om");
    assert_eq!(
        os.injected(),
        [
            Injected::StartMarker(ExpansionId::from_raw(1)),
            Injected::DeleteBack(6),
            Injected::TypeText("be right back".to_owned()),
            Injected::TypeText(" ".to_owned()),
            Injected::Retype(2),
            Injected::EndMarker(ExpansionId::from_raw(1)),
        ]
    );

    // The engine's buffer agrees with the field: finishing the word expands it.
    os.type_text("w ");
    worker.settle();
    assert_eq!(text(&os, editor), "be right back on my way ");

    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [
            InjectionOutcome::Finished { retyped: 2 },
            InjectionOutcome::Finished { retyped: 0 },
        ]
    );
    assert_eq!(report.used, [id(1), id(2)]);
    assert!(report.diagnostics.is_empty(), "compensated, not reported");
}

#[test]
fn backspaces_that_only_undo_later_keys_are_compensated_too() {
    let (os, editor, worker) = setup();
    let mut keys = MockKey::text("oxm");
    keys.extend([
        MockKey::Key(KeyName::Backspace),
        MockKey::Key(KeyName::Backspace),
        MockKey::Char('m'),
    ]);
    os.interleave(InjectionPoint::BeforeStartMarker, keys);
    os.type_text("brb ");
    worker.settle();
    assert_eq!(text(&os, editor), "be right back om");
    assert_eq!(os.injected()[1], Injected::DeleteBack(6));

    os.type_text("w ");
    worker.settle();
    assert_eq!(text(&os, editor), "be right back on my way ");
    worker.stop();
}

#[test]
fn compensated_keys_are_typed_again_at_the_caret_of_a_cursor_snippet() {
    let (os, editor, worker) = setup();
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("An"));
    os.type_text("greet ");
    worker.settle();
    assert_eq!(text(&os, editor), "Hi An, how are you?");
    os.type_text("a");
    assert_eq!(text(&os, editor), "Hi Ana, how are you?");
    let report = worker.stop();
    assert_eq!(report.outcomes, [InjectionOutcome::Finished { retyped: 2 }]);
}

#[test]
fn typing_ahead_in_many_expansions_never_corrupts_the_text() {
    let (os, editor, worker) = setup();
    // A fixed pseudo-random sequence (a linear congruential generator), so a failure can be
    // replayed.
    let mut state: u32 = 0x1A3B_5C7D;
    let mut next = |bound: u32| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 16) % bound
    };
    let mut expected = String::new();
    let mut retyped_total = 0;
    for _ in 0..300 {
        // Up to three letters typed ahead, then a space so the next abbreviation starts a word.
        let mut ahead = String::new();
        for _ in 0..next(4) {
            ahead.push(char::from(b'x' + u8::try_from(next(3)).expect("below 3")));
        }
        if !ahead.is_empty() {
            ahead.push(' ');
        }
        retyped_total += ahead.chars().count();
        os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text(&ahead));
        os.type_text("brb ");
        worker.settle();
        expected.push_str("be right back ");
        expected.push_str(&ahead);
        assert_eq!(text(&os, editor), expected);
    }
    let report = worker.stop();
    assert_eq!(report.used.len(), 300);
    assert!(report.diagnostics.is_empty());
    let retyped: usize = report
        .outcomes
        .iter()
        .map(|outcome| match outcome {
            InjectionOutcome::Finished { retyped } => *retyped,
            _ => 0,
        })
        .sum();
    assert_eq!(retyped, retyped_total);
    assert!(retyped_total > 300, "the run did type ahead");
}

#[test]
fn a_boundary_before_the_start_marker_cancels_the_expansion() {
    let (os, editor, worker) = setup();
    os.interleave(
        InjectionPoint::BeforeStartMarker,
        [MockKey::Char('a'), MockKey::Key(KeyName::Left)],
    );
    os.type_text("brb ");
    worker.settle();

    // The abbreviation stays: a visible, bounded failure (ADR-0019).
    assert_eq!(text(&os, editor), "brb a");
    assert!(matches!(
        os.injected().last(),
        Some(Injected::Cancelled(_, CancelReason::Boundary))
    ));

    // The next abbreviation expands normally.
    os.press(KeyName::Right);
    os.type_text(" omw ");
    worker.settle();
    assert_eq!(text(&os, editor), "brb a on my way ");

    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [
            InjectionOutcome::Cancelled(CancelReason::Boundary),
            InjectionOutcome::Finished { retyped: 0 },
        ]
    );
    assert_eq!(report.diagnostics, [Reason::InjectionCancelled]);
    assert_eq!(report.used, [id(2)], "a cancelled expansion is not usage");
}

#[test]
fn a_backspace_into_the_abbreviation_cancels_the_expansion() {
    let (os, editor, worker) = setup();
    os.interleave(
        InjectionPoint::BeforeStartMarker,
        [MockKey::Key(KeyName::Backspace)],
    );
    os.type_text("brb ");
    worker.settle();
    assert_eq!(text(&os, editor), "brb");
    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [InjectionOutcome::Cancelled(CancelReason::OverDeleted)]
    );
    assert_eq!(report.diagnostics, [Reason::InjectionCancelled]);
}

#[test]
fn keys_between_the_markers_are_reported_and_not_repaired() {
    let (os, editor, worker) = setup();
    os.interleave(InjectionPoint::BeforeStep(1), MockKey::text("x"));
    os.type_text("brb ");
    worker.settle();

    // The key landed between the deletion and the text. It cannot be repaired; the engine
    // clears its buffer and records why.
    assert_eq!(text(&os, editor), "xbe right back ");
    let report_so_far = os.injected();
    assert_eq!(report_so_far.len(), BRB_STEPS);

    // Expansion works again afterwards.
    os.type_text("omw ");
    worker.settle();
    assert_eq!(text(&os, editor), "xbe right back on my way ");

    let report = worker.stop();
    assert_eq!(report.diagnostics, [Reason::TypedDuringInjection]);
    assert_eq!(report.outcomes.len(), 2);
    assert_eq!(report.used, [id(1), id(2)]);
    assert_eq!(report.final_state, Some(InjectionState::Idle));
}

#[test]
fn a_key_right_after_the_start_marker_is_not_counted() {
    let (os, editor, worker) = setup();
    os.interleave(InjectionPoint::AfterStartMarker, MockKey::text("x"));
    os.type_text("brb ");
    worker.settle();
    // The deletion took the key instead of the first letter of the abbreviation.
    assert_eq!(text(&os, editor), "bbe right back ");
    let report = worker.stop();
    assert_eq!(report.outcomes, [InjectionOutcome::Finished { retyped: 0 }]);
    assert_eq!(report.diagnostics, [Reason::TypedDuringInjection]);
}

#[test]
fn an_atomic_batch_keeps_real_keys_out_of_the_output() {
    let (os, editor, worker) = setup();
    os.set_atomic_batches(true);
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("o"));
    os.interleave(InjectionPoint::BeforeStep(1), MockKey::text("m"));
    os.interleave(InjectionPoint::BeforeEndMarker, MockKey::text("w "));
    os.type_text("brb ");
    worker.settle();

    // 'o' was compensated; 'm', 'w' and the space queued behind the batch and were typed
    // after the end marker, where they completed the next abbreviation.
    assert_eq!(text(&os, editor), "be right back on my way ");
    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [
            InjectionOutcome::Finished { retyped: 1 },
            InjectionOutcome::Finished { retyped: 0 },
        ]
    );
    assert!(report.diagnostics.is_empty());
}

#[test]
fn a_focus_change_before_the_injection_cancels_it() {
    let (os, editor, worker) = setup();
    let other = os.open_window(MockWindow::new("org.example.chat", 200));

    // Hold the injector back, trigger, and move the focus before it runs.
    worker.gate.set_closed(true);
    os.type_text("brb ");
    worker.sync();
    os.focus(other).expect("window is open");
    worker.sync();
    worker.gate.set_closed(false);
    worker.settle();

    assert_eq!(text(&os, editor), "brb ", "the abbreviation stays");
    assert_eq!(
        text(&os, other),
        "",
        "nothing is typed into the other window"
    );
    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [InjectionOutcome::Cancelled(CancelReason::Disturbed)]
    );
    assert_eq!(report.diagnostics, [Reason::InjectionCancelled]);
    assert!(report.used.is_empty());
}

#[test]
fn many_keys_before_the_start_marker_cancel_at_the_capacity() {
    let (os, editor, worker) = setup();
    let many = "k".repeat(65);
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text(&many));
    os.type_text("brb ");
    worker.settle();
    assert_eq!(text(&os, editor), format!("brb {many}"));
    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [InjectionOutcome::Cancelled(CancelReason::TooManyTyped)]
    );
}

#[test]
fn lost_events_during_a_flight_cancel_it() {
    let (os, editor, worker) = setup();
    worker.gate.set_closed(true);
    os.type_text("brb ");
    os.lose_events();
    worker.sync();
    worker.gate.set_closed(false);
    worker.settle();
    assert_eq!(text(&os, editor), "brb ");
    let report = worker.stop();
    assert_eq!(
        report.outcomes,
        [InjectionOutcome::Cancelled(CancelReason::Overflow)]
    );
    // The engine already forgot the flight on Overflow and ignores the late cancellation
    // silently: no diagnostic, no change.
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(report.final_state, Some(InjectionState::Idle));
}

// ---------------------------------------------------------------------------------------------
// Focus, secure fields, hotkeys, permissions
// ---------------------------------------------------------------------------------------------

#[test]
fn nothing_expands_in_a_password_field_an_own_window_or_an_unknown_one() {
    let os = MockPlatform::new();
    let password = os.open_window(MockWindow::new("org.example.bank", 1).secure());
    let palette = os.open_window(MockWindow::new("iamb-desktop", 2).own());
    let admin = os.open_window(MockWindow::new("admin-tool", 3).elevated());
    let slow = os.open_window(
        MockWindow::new("org.example.slow", 4).inspect_delay(SECURE_STATE_TIMEOUT * 2),
    );
    let editor = os.open_window(MockWindow::new("org.example.editor", 5));
    let worker = Worker::start(&os, library());

    for window in [password, palette, admin, slow] {
        os.focus(window).expect("window is open");
        worker.sync();
        os.type_text("brb ");
        worker.settle();
        assert_eq!(text(&os, window), "brb ");
    }
    os.focus(editor).expect("window is open");
    worker.sync();
    os.type_text("brb ");
    worker.settle();
    assert_eq!(text(&os, editor), "be right back ");

    let report = worker.stop();
    assert_eq!(
        report.verdicts,
        [
            SecureState::Secure,
            SecureState::OwnWindow,
            SecureState::Unknown,
            SecureState::Unknown,
            SecureState::Normal,
        ]
    );
    assert_eq!(
        report.diagnostics,
        [
            Reason::Suspended(SuspendReason::SecureField),
            Reason::Suspended(SuspendReason::UnknownFocus),
            Reason::Suspended(SuspendReason::UnknownFocus),
        ]
    );
    assert_eq!(report.outcomes.len(), 1);
}

#[test]
fn moving_to_a_password_field_of_the_same_window_suspends() {
    let (os, editor, worker) = setup();
    os.type_text("br");
    os.set_security(editor, FieldSecurity::Secure);
    worker.sync();
    os.type_text("b ");
    worker.settle();
    assert_eq!(text(&os, editor), "brb ");

    os.set_security(
        editor,
        FieldSecurity::Unknown(UnknownReason::NotInspectable),
    );
    worker.sync();
    os.type_text("brb ");
    os.set_security(editor, FieldSecurity::Normal);
    worker.sync();
    os.type_text("brb ");
    worker.settle();
    assert_eq!(text(&os, editor), "brb brb be right back ");
    let report = worker.stop();
    assert_eq!(report.outcomes.len(), 1);
}

#[test]
fn a_hotkey_reaches_the_service_and_not_the_engine_or_the_field() {
    let (os, editor, worker) = setup();
    let palette = Chord::parse("Ctrl+Shift+Space").expect("chord");
    os.register(HotkeyId(7), palette).expect("free chord");
    os.type_text("br");
    assert!(os.press_chord(palette));
    os.type_text("b ");
    worker.settle();
    // The hotkey is not a boundary: the abbreviation typed around it still expands.
    assert_eq!(text(&os, editor), "be right back ");
    let report = worker.stop();
    assert_eq!(report.hotkeys, [HotkeyId(7)]);
}

#[test]
fn an_injection_the_os_refuses_is_reported_as_cancelled() {
    let (os, editor, worker) = setup();
    os.set_clipboard_busy(true);
    os.type_text("brb ");
    worker.settle();
    // Plain text needs no clipboard: it still works.
    assert_eq!(text(&os, editor), "be right back ");
    os.set_clipboard_busy(false);

    os.set_permission(PermissionKind::Accessibility, Grant::Denied);
    os.type_text("omw ");
    worker.settle();
    assert_eq!(text(&os, editor), "be right back omw ");
    let report = worker.stop();
    assert_eq!(
        report.outcomes[1],
        InjectionOutcome::Failed(PlatformError::PermissionDenied(
            PermissionKind::Accessibility
        ))
    );
    assert_eq!(report.diagnostics, [Reason::InjectionCancelled]);
    assert_eq!(report.used, [id(1)]);
}

#[test]
fn a_dead_hook_stops_expansion_and_the_self_test_notices() {
    let (os, editor, worker) = setup();
    os.kill_hook();
    os.type_text("brb ");
    worker.settle();
    assert_eq!(text(&os, editor), "brb ", "the listener is blind");
    assert_eq!(
        os.self_test(std::time::Duration::from_secs(1)),
        Err(PlatformError::Timeout)
    );
    // The typed-characters hand-off and the engine were never involved.
    assert_eq!(worker.handoff.status(), HandoffStatus::Idle);
    assert!(os.injected().is_empty());
    worker.stop();
}
