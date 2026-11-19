//! The engine thread: owns the engine and the keystroke buffer, feeds the typed-characters
//! hand-off, sends the heartbeat, and keeps the service's side of §4.5 and §4.6.
//!
//! # Parked requests (the order duty of §4.5)
//!
//! An explicit request names the window it will type into: a palette pick carries the focus
//! token the palette was opened for, a form submit the token captured when its form was
//! requested. The focus change for that window reaches this thread from the focus thread and
//! the request from a reader thread, so either can come first. A request is forwarded to the
//! engine only once the `FocusChanged` for its window has been delivered; until then it is
//! *parked*. Delivering that `FocusChanged` forwards it, right behind the change (the engine
//! then holds it for the verdict). A newer request replaces a parked one, a cancelled form
//! drops its parked submit, and a request whose window never gets the focus is dropped after
//! [`PARK_TIMEOUT`] with the diagnostic `request_focus_timeout`. A request without a token runs
//! against whatever has the focus.
//!
//! # Keys that went to another window (the 2.8 review's finding 5)
//!
//! A key reaches this thread straight from the hook; a focus change reaches it later, through
//! the OS watcher and the focus thread. Until the `FocusChanged` is here, the engine still has
//! the verdict of the window before, so a key typed into a newly focused password field would
//! be stored and matched as if typed in the previous field. Where the listener says where a
//! key went ([`KeyTarget`]), a key whose window is not the one of the last `FocusChanged`
//! delivered here is not handed to the engine at all (nor to the hand-off, which is disturbed
//! instead, so an expansion waiting for its start marker is cancelled). Nothing is stored for
//! it, and the engine's buffer still describes the text before the caret of the window it
//! belongs to; the `FocusChanged` that follows clears it as usual. A key without a target (the
//! macOS listener, an event that is not a key) is handled as before. A move to another field
//! of the same window is not seen this way: the target names a window, not a field.
//!
//! # A focus watcher that stopped (the 2.8 review's finding 15)
//!
//! A watcher that ended by itself reports nothing more, so the last verdict would stay in force
//! wherever the focus goes. With every heartbeat this thread asks the platform whether its
//! watcher still runs ([`FocusInspector::is_watching`](iamb_platform::FocusInspector)), once
//! the worker started one. When it does not, the engine is told `SecureState(Unknown)` (it is
//! suspended, and a verdict that still arrives for the old focus is taken as unknown) and the
//! watcher is started again; its first report, the focus as it is, brings the next verdict.
//! While starting fails, it is tried again at the next heartbeat.
//!
//! # Why an injection stopped
//!
//! Every outcome but `Finished` becomes `InjectionCancelled`, whose diagnostic is
//! `injection_cancelled`. Two reasons of the injector get a code of their own instead, because
//! the user can act on them: `injection_focus_moved` (the focus changed after the start marker,
//! the injector stopped there) and `injection_clipboard_not_restorable` (the clipboard held
//! content its snapshot could not save whole, and the expansion needed a paste). Like every
//! diagnostic they name the snippet and the app, nothing typed.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::SecureState;
use iamb_core::engine::{
    Action, Engine, Environment, Event, ExpansionId, FormSessionId, InjectionState,
};
use iamb_core::model::Id;
use iamb_core::sensitive::Sensitive;
use iamb_core::template::{FieldSpec, FormValues};
use iamb_ipc::{EventBody, FocusRef, InjectionStatus, Secret, WireDiagnostic, WireField};
use iamb_platform::{
    CancelReason, FocusChange, FocusToken, InjectionOutcome, InjectionRequest, KeyTarget,
    ListenerEvent, Routed, TypedHandoff, split_actions,
};

use super::keys::KeyQueue;
use super::store::StoreMsg;
use super::{EngineMsg, EngineStatus, Explicit, PALETTE_HOTKEY, Shared, WakeCounters};

/// How long a parked request waits for the focus change of its window: the UI host's
/// `restore_focus` limit (500 ms) and some margin.
pub const PARK_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a connected UI host has to acknowledge a `FormRequested` (§4.5 step 2).
pub const FORM_ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// How long remembered form values live without use (§4.5).
pub const FORM_VALUES_TTL: Duration = Duration::from_mins(30);

struct Env<'a> {
    shared: &'a Shared,
    now: LocalDateTime,
}

impl Environment for Env<'_> {
    fn now(&self) -> LocalDateTime {
        self.now
    }

    fn clipboard_text(&self) -> Option<String> {
        self.shared
            .platform
            .read_text()
            .ok()
            .flatten()
            .map(Sensitive::into_inner)
    }
}

struct Parked {
    request: Explicit,
    token: FocusToken,
    deadline: Instant,
}

/// What an expansion in flight will count as usage.
struct InFlight {
    snippet: Id,
    chars_saved: usize,
}

pub(crate) struct EngineThread {
    shared: Arc<Shared>,
    engine: Engine,
    handoff: Arc<TypedHandoff>,
    keys: Arc<KeyQueue>,
    injector_tx: Sender<InjectionRequest>,
    heartbeat: Box<dyn FnMut() + Send>,
    heartbeat_every: Duration,
    next_heartbeat: Instant,
    /// Expansions sent to the injector whose outcome has not come back.
    injecting: usize,
    usage: HashMap<ExpansionId, InFlight>,
    settle_waiters: Vec<Sender<()>>,
    /// The last focus change delivered to the engine.
    focus: Option<FocusChange>,
    parked: Option<Parked>,
    /// The focus token of each requested form, for its submit.
    forms: HashMap<FormSessionId, Option<FocusToken>>,
    ack_deadline: Option<(FormSessionId, Instant)>,
    values_touched: Option<Instant>,
    paused: bool,
    /// The code the next `injection_cancelled` diagnostic is given instead (module docs).
    cancel_code: Option<&'static str>,
    /// The focus watcher stopped by itself and is not running again yet (module docs).
    watcher_down: bool,
}

impl EngineThread {
    pub(crate) fn new(
        shared: Arc<Shared>,
        engine: Engine,
        handoff: Arc<TypedHandoff>,
        keys: Arc<KeyQueue>,
        injector_tx: Sender<InjectionRequest>,
        heartbeat: Box<dyn FnMut() + Send>,
        heartbeat_every: Duration,
    ) -> Self {
        Self {
            shared,
            engine,
            handoff,
            keys,
            injector_tx,
            heartbeat,
            heartbeat_every,
            next_heartbeat: Instant::now(),
            injecting: 0,
            usage: HashMap::new(),
            settle_waiters: Vec::new(),
            focus: None,
            parked: None,
            forms: HashMap::new(),
            ack_deadline: None,
            values_touched: None,
            paused: false,
            cancel_code: None,
            watcher_down: false,
        }
    }

    /// The thread's loop.
    pub(crate) fn run(mut self, rx: &Receiver<EngineMsg>) {
        loop {
            let now = Instant::now();
            if now >= self.next_heartbeat {
                (self.heartbeat)();
                WakeCounters::bump(&self.shared.wakes.heartbeats);
                self.next_heartbeat = now + self.heartbeat_every;
                self.check_watcher();
            }
            self.expire(now);
            let wake = self.next_deadline();
            let msg = super::recv_counted(
                rx,
                wake.saturating_duration_since(Instant::now()),
                &self.shared.wakes.engine,
            );
            match msg {
                Ok(EngineMsg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(msg) => self.on_message(msg),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        self.handoff.clear();
        self.keys.clear();
    }

    fn next_deadline(&self) -> Instant {
        let mut next = self.next_heartbeat;
        let mut consider = |t: Option<Instant>| {
            if let Some(t) = t {
                next = next.min(t);
            }
        };
        consider(self.parked.as_ref().map(|p| p.deadline));
        consider(self.ack_deadline.map(|(_, t)| t));
        consider(self.values_touched.map(|t| t + FORM_VALUES_TTL));
        next
    }

    fn expire(&mut self, now: Instant) {
        if self.parked.as_ref().is_some_and(|p| now >= p.deadline)
            && let Some(parked) = self.parked.take()
        {
            self.dropped(&parked.request, "request_focus_timeout");
        }
        if let Some((session, deadline)) = self.ack_deadline
            && now >= deadline
        {
            self.ack_deadline = None;
            self.forms.remove(&session);
            self.handle(Event::FormCancelled { session });
            self.shared
                .diagnostic(WireDiagnostic::code("form_not_acknowledged"));
        }
        if self
            .values_touched
            .is_some_and(|t| now >= t + FORM_VALUES_TTL)
        {
            self.values_touched = None;
            self.engine.forget_form_values();
        }
    }

    fn on_message(&mut self, msg: EngineMsg) {
        match msg {
            EngineMsg::Key => {
                if let Some((event, target)) = self.keys.pop() {
                    self.on_key(event, target);
                }
            }
            EngineMsg::FocusChanged(change) => self.on_focus(change),
            EngineMsg::Verdict(mut state) => {
                if self.watcher_down {
                    // A verdict for a focus the stopped watcher last saw (an exclusion change
                    // judges it again): the focus may be anywhere by now.
                    state = SecureState::Unknown;
                }
                // As it is wired, this never finds a capture: each verdict follows its own
                // `FocusChanged`, which disturbed already, and the engine is suspended between
                // the two, so nothing can be armed. It stays so the hand-off's writer duty
                // (disturb before every event that clears the buffer) does not rest on that.
                self.handoff.disturb();
                self.handle(Event::SecureState(state));
            }
            EngineMsg::Injected { expansion, outcome } => {
                self.handoff.finish(expansion);
                if outcome.needs_cancel_event() {
                    self.usage.remove(&expansion);
                    self.cancel_code = match outcome {
                        InjectionOutcome::Cancelled(CancelReason::FocusMoved) => {
                            Some("injection_focus_moved")
                        }
                        InjectionOutcome::Cancelled(CancelReason::ClipboardNotRestorable) => {
                            Some("injection_clipboard_not_restorable")
                        }
                        _ => None,
                    };
                    self.handle(Event::InjectionCancelled { expansion });
                    self.cancel_code = None;
                } else {
                    self.record_usage(expansion);
                }
                self.injecting = self.injecting.saturating_sub(1);
                if self.injecting == 0 {
                    for w in self.settle_waiters.drain(..) {
                        let _ = w.send(());
                    }
                }
            }
            EngineMsg::Request { request, focus } => self.on_request(request, focus),
            EngineMsg::FormCancel(session) => {
                self.forms.remove(&session);
                if self.ack_deadline.is_some_and(|(s, _)| s == session) {
                    self.ack_deadline = None;
                }
                if matches!(&self.parked, Some(Parked { request: Explicit::Submit { session: s, .. }, .. }) if *s == session)
                {
                    self.parked = None;
                }
                self.handle(Event::FormCancelled { session });
            }
            EngineMsg::FormAck(session) => {
                if self.ack_deadline.is_some_and(|(s, _)| s == session) {
                    self.ack_deadline = None;
                }
            }
            EngineMsg::SetPaused(on) => {
                self.handoff.disturb();
                self.handle(Event::Paused(on));
                if self.paused != on {
                    self.paused = on;
                    let status = self.shared.status(self.status());
                    self.shared
                        .clients
                        .broadcast(&EventBody::StatusChanged { status });
                }
            }
            EngineMsg::Snapshot(snapshot) => self.handle(Event::SnapshotChanged(snapshot)),
            EngineMsg::ForgetFormValues => {
                self.values_touched = None;
                self.engine.forget_form_values();
            }
            EngineMsg::Status(reply) => {
                let _ = reply.send(self.status());
            }
            EngineMsg::Sync(reply) => {
                let _ = reply.send(());
            }
            EngineMsg::Settle(reply) => {
                if self.injecting == 0 {
                    let _ = reply.send(());
                } else {
                    self.settle_waiters.push(reply);
                }
            }
            EngineMsg::Stop => {}
            #[cfg(debug_assertions)]
            EngineMsg::Hang => {
                self.shared
                    .logger
                    .warn(format_args!("test fault: the engine thread hangs"));
                loop {
                    std::thread::park();
                }
            }
        }
    }

    fn status(&self) -> EngineStatus {
        EngineStatus {
            paused: self.paused,
            suspended: self.engine.is_suspended(),
            injection: match self.engine.injection_state() {
                InjectionState::Idle => InjectionStatus::Idle,
                InjectionState::Pending(_) => InjectionStatus::Pending,
                InjectionState::Injecting(_) => InjectionStatus::Injecting,
            },
        }
    }

    fn on_key(&mut self, event: ListenerEvent, target: Option<KeyTarget>) {
        if let (Some(target), Some(focus)) = (target, &self.focus)
            && !target.is(&focus.token)
        {
            // The key went to a window the focus thread has not reported yet: neither the
            // engine nor the hand-off may see it (module docs).
            self.handoff.disturb();
            return;
        }
        // The hand-off sees every listener event, before the engine does.
        self.handoff.observe(&event);
        if let ListenerEvent::InjectionFinished { expansion, .. } = &event {
            self.record_usage(*expansion);
        }
        match event.route() {
            Routed::Engine(event) => self.handle(event),
            Routed::Hotkey(id) if id == PALETTE_HOTKEY => {
                match self.shared.platform.focus_token() {
                    Ok(token) => self.shared.clients.broadcast(&EventBody::PaletteRequested {
                        focus: FocusRef::from(token),
                    }),
                    Err(e) => self
                        .shared
                        .logger
                        .warn(format_args!("palette hotkey without a focused window: {e}")),
                }
            }
            Routed::Hotkey(_) => {}
        }
    }

    /// With the heartbeat: is the focus watcher the worker started still running? See the
    /// module docs.
    fn check_watcher(&mut self) {
        let platform = Arc::clone(&self.shared.platform);
        if !self.shared.watching.load(Ordering::Relaxed) || platform.is_watching() {
            return;
        }
        if !self.watcher_down {
            self.watcher_down = true;
            self.shared.logger.warn(format_args!(
                "the focus watcher stopped; the focus counts as unknown until it runs again"
            ));
            self.handoff.disturb();
            self.handle(Event::SecureState(SecureState::Unknown));
        }
        let _ = platform.unwatch();
        match platform.watch(super::focus_sink(self.shared.focus_tx.clone())) {
            Ok(()) => {
                self.watcher_down = false;
                self.shared
                    .logger
                    .info(format_args!("the focus watcher runs again"));
            }
            Err(e) => self
                .shared
                .logger
                .warn(format_args!("cannot start the focus watcher again: {e}")),
        }
    }

    fn on_focus(&mut self, change: FocusChange) {
        // Anything else that clears the engine's buffer disturbs a capture.
        self.handoff.disturb();
        self.handle(Event::FocusChanged(change.app.app.clone()));
        let token = change.token;
        self.focus = Some(change);
        if self.parked.as_ref().is_some_and(|p| p.token == token)
            && let Some(parked) = self.parked.take()
        {
            self.forward(parked.request);
        }
    }

    fn on_request(&mut self, request: Explicit, focus: Option<FocusToken>) {
        let focus = match &request {
            Explicit::Submit { session, .. } => self.forms.remove(session).flatten(),
            _ => focus,
        };
        if let Some(old) = self.parked.take() {
            // A newer request replaces a waiting one (§4.5).
            self.dropped(&old.request, "request_cancelled");
        }
        match focus {
            Some(token) if self.focus.as_ref().map(|c| c.token) != Some(token) => {
                self.parked = Some(Parked {
                    request,
                    token,
                    deadline: Instant::now() + PARK_TIMEOUT,
                });
            }
            _ => self.forward(request),
        }
    }

    fn forward(&mut self, request: Explicit) {
        let event = match request {
            Explicit::Expand(snippet) => Event::ExpandRequest { snippet },
            Explicit::Submit { session, values } => {
                self.values_touched = Some(Instant::now());
                Event::FormSubmitted { session, values }
            }
            Explicit::Reopen => {
                self.values_touched = Some(Instant::now());
                Event::ReopenLastForm
            }
        };
        self.handle(event);
    }

    fn dropped(&self, request: &Explicit, code: &str) {
        let mut d = WireDiagnostic::code(code);
        if let Explicit::Expand(snippet) = request {
            d.snippet = Some(*snippet);
        }
        self.shared.diagnostic(d);
    }

    /// Hands the usage of a finished expansion to the store thread, once: on its end marker
    /// or on the injector's `Finished`, whichever comes first.
    fn record_usage(&mut self, expansion: ExpansionId) {
        if let Some(done) = self.usage.remove(&expansion) {
            self.shared.store.send(StoreMsg::RecordUsage {
                snippet: done.snippet,
                chars_saved: done.chars_saved,
            });
        }
    }

    /// Runs one event through the engine and carries out its actions.
    fn handle(&mut self, event: Event) {
        let env = Env {
            shared: &self.shared,
            now: (self.shared.clock)(),
        };
        let actions = self.engine.handle(event, &env);
        if actions.is_empty() {
            return;
        }
        let (batches, other) = split_actions(actions);
        for action in other {
            match action {
                Action::Diagnostic(d) => {
                    let mut wire = WireDiagnostic::from(&d);
                    if let Some(code) = self.cancel_code
                        && wire.code == "injection_cancelled"
                    {
                        code.clone_into(&mut wire.code);
                    }
                    self.shared.diagnostic(wire);
                }
                Action::ShowForm {
                    session,
                    snippet,
                    spec,
                    last_values,
                } => self.show_form(session, snippet, &spec, last_values),
                _ => {}
            }
        }
        for batch in batches {
            // Arm before the injector can run: it must find the hand-off capturing.
            self.handoff.arm(batch.expansion);
            self.usage.insert(
                batch.expansion,
                InFlight {
                    snippet: batch.snippet,
                    chars_saved: batch.chars_saved,
                },
            );
            let mut request = InjectionRequest::for_batch(&batch);
            if let Some(change) = &self.focus {
                request = request.in_app(change.app.app.clone());
            }
            self.injecting += 1;
            if self.injector_tx.send(request).is_err() {
                // The injector is gone (shutdown): answer the expansion ourselves.
                self.injecting -= 1;
                self.handoff.finish(batch.expansion);
                self.usage.remove(&batch.expansion);
                self.handle(Event::InjectionCancelled {
                    expansion: batch.expansion,
                });
            }
        }
    }

    fn show_form(
        &mut self,
        session: FormSessionId,
        snippet: Id,
        spec: &[FieldSpec],
        last_values: Option<Sensitive<FormValues>>,
    ) {
        if !self.shared.clients.ui_connected() {
            // §4.5 step 2: abandon. Hook point: ask the supervisor to start the UI host, once
            // the supervisor runs one (it does not yet).
            self.handle(Event::FormCancelled { session });
            let mut d = WireDiagnostic::code("form_no_ui_host");
            d.snippet = Some(snippet);
            self.shared.diagnostic(d);
            return;
        }
        let token = self.focus.as_ref().map(|c| c.token);
        self.forms.insert(session, token);
        self.ack_deadline = Some((session, Instant::now() + FORM_ACK_TIMEOUT));
        if let Some(pid) = self.shared.clients.ui_pid() {
            // §4.5 step 3 (Windows): let the UI host take the foreground. Does nothing on macOS.
            let _ = self.shared.platform.allow_foreground(pid);
        }
        let last_values = last_values.map(|values| {
            let values = values.expose();
            Secret::new(
                spec.iter()
                    .filter_map(|f| values.get(&f.name).map(|v| (f.name.clone(), v.to_owned())))
                    .collect(),
            )
        });
        self.shared.clients.broadcast(&EventBody::FormRequested {
            session: session.raw(),
            snippet,
            spec: spec.iter().map(WireField::from).collect(),
            focus: token.map(FocusRef::from),
            last_values,
        });
    }
}
