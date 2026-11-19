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
//! belongs to; the `FocusChanged` that follows clears it as usual. A key without a target (a
//! listener that could not tell where it went, an event that is not a key) is handled as
//! before. A move to another field of the same window is not seen this way: the target names
//! a window, not a field.
//!
//! # When the report does not come (the follow-up review's N5)
//!
//! If the watcher missed the change, those keys would be dropped until the next focus event,
//! without a word. So after [`RESYNC_AFTER_KEYS`] such keys, or [`RESYNC_AFTER`] of them, the
//! diagnostic `keys_for_unreported_window` is recorded (a code, nothing typed, no application)
//! and the focus is read from the inspector as it is now. When it names another window than
//! the last report, that reading goes to the focus thread as if the watcher had reported it,
//! and brings its verdict. When it names the same window, the listener's stamp and the
//! inspector disagree about where keys go (a key window owned by a helper process of the
//! frontmost application, say). Once that has lasted [`RESYNC_AFTER`], keys with that stamp
//! are handled as keys without a target, as before the stamp existed, until the next focus
//! change: a stamp the inspector cannot confirm must not stop expansion in a window for good.
//! It is logged as a warning, with numbers only.
//!
//! # Where an expansion's keys belong (the follow-up review's N1)
//!
//! Every [`InjectionRequest`] names the window of the last `FocusChanged` delivered here
//! ([`InjectionRequest::target`]), the window every key let through was compared with, so the
//! trigger's. Before any `FocusChanged`, it is the trigger key's own target, if it has one.
//! The injector compares it with where its keys would go right before its start marker and
//! does not start when that is another window (`injection_focus_moved`, nothing sent).
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
//! # Iamb's own panels (the 2.8 review's finding 16)
//!
//! On macOS the form and the palette are non-activating panels: the application behind them
//! stays the frontmost one, so the focus thread keeps its verdict (`normal`) and keys typed
//! into a panel would be buffered and matched as if typed into that application. The UI host
//! says when a panel has the keyboard (`PanelFocus`). While one has it:
//!
//! - every key (`Char`, `Backspace`, `Boundary`) is dropped before the hand-off and the engine
//!   see it, and before the key-target check (a key typed into the panel names Iamb's own
//!   process, which is no focus report's): nothing is stored, matched or captured. Injection
//!   markers, composition, overflow and hotkeys go on as before;
//! - the engine is held in `own_window`: the panel's start delivers `SecureState(OwnWindow)`
//!   (after disturbing the hand-off, which cancels an expansion still waiting for its start
//!   marker), and every verdict the focus thread sends meanwhile is noted and delivered as
//!   `own_window`. Explicit requests are refused (`refused_own_window`), as in any window of
//!   Iamb's.
//!
//! When the panel gives the keyboard back, the last verdict the focus thread sent for the
//! current focus is delivered again (none, when the verdict for the latest focus change is
//! still to come: it is delivered when it comes). This happens on the engine thread, in queue
//! order, so a `FormSubmit` or `ExpandSnippet` the UI host sends right after `shown: false`
//! finds the field's verdict in force. A panel's hold ends with `shown: false`; with
//! `FormSubmit` or `FormCancel` for a form, `ExpandSnippet` or `ReopenLastForm` for the palette
//! (the UI host hides the panel before it sends them); with the UI host's disconnection; or when
//! its lease runs out ([`PanelFocus` holds for a lease](iamb_ipc::PANEL_FOCUS_LEASE), renewed
//! by every `shown: true`), with the diagnostic `panel_focus_expired`. On Windows the panels take
//! the foreground, so the focus thread reports `own_window` itself; the message changes
//! nothing there.
//!
//! # Screen lock and sign-out (the 2.8 review's finding 13)
//!
//! Before each message, a forget requested by the session watcher
//! ([`Shared::forget_pending`]) is carried out: the remembered form values are overwritten
//! (`Engine::forget_form_values`, whose values overwrite themselves when dropped) and the
//! hand-off is disturbed, which overwrites what it captured.
//!
//! # Why an injection stopped
//!
//! Every outcome but `Finished` becomes `InjectionCancelled`, whose diagnostic is
//! `injection_cancelled`. Two reasons of the injector get a code of their own instead, because
//! the user can act on them: `injection_focus_moved` (the keys would have gone to another
//! window than the trigger's before the start marker, and nothing was sent, or the focus
//! changed after it and the injector stopped there) and `injection_clipboard_not_restorable` (the clipboard held
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
use iamb_ipc::{
    EventBody, FocusRef, InjectionStatus, PanelKind, Secret, WireDiagnostic, WireField,
};
use iamb_platform::{
    CancelReason, FocusChange, FocusToken, InjectionOutcome, InjectionRequest, KeyTarget,
    ListenerEvent, Routed, TypedHandoff, split_actions,
};

use super::keys::KeyQueue;
use super::store::StoreMsg;
use super::{
    EngineMsg, EngineStatus, Explicit, FocusMsg, PALETTE_HOTKEY, Shared, WakeCounters, coded,
};

/// How long a parked request waits for the focus change of its window: the UI host's
/// `restore_focus` limit (500 ms) and some margin.
pub const PARK_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a connected UI host has to acknowledge a `FormRequested` (§4.5 step 2).
pub const FORM_ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// How long remembered form values live without use (§4.5).
pub const FORM_VALUES_TTL: Duration = Duration::from_mins(30);

/// After this many keys for a window the focus thread has not reported, the focus is read
/// again (module docs).
pub const RESYNC_AFTER_KEYS: u32 = 3;

/// After this long of keys for a window the focus thread has not reported, the focus is read
/// again; a stamp the inspector still contradicts then is no longer trusted (module docs).
pub const RESYNC_AFTER: Duration = Duration::from_millis(500);

/// Keys that went to a window the focus thread has not reported (module docs).
struct Elsewhere {
    target: KeyTarget,
    since: Instant,
    keys: u32,
    /// When the focus was last read again for these keys.
    synced: Option<Instant>,
}

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

/// One of Iamb's panels has the keyboard (module docs).
struct Panel {
    kind: PanelKind,
    until: Instant,
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
    /// Keys are going to a window the focus thread has not reported (module docs).
    elsewhere: Option<Elsewhere>,
    /// A stamp the inspector contradicted: keys with it are handled as keys without a target
    /// until the next focus change (module docs).
    unconfirmed: Option<KeyTarget>,
    /// The target of the key being handled, for the request of an expansion it triggers.
    trigger: Option<KeyTarget>,
    /// A panel of Iamb's has the keyboard (module docs).
    panel: Option<Panel>,
    panel_lease: Duration,
    /// The last verdict for the current focus, as the focus thread sent it; `None` until the
    /// verdict for the latest focus change is in.
    verdict: Option<SecureState>,
}

impl EngineThread {
    #[allow(
        clippy::too_many_arguments,
        reason = "the thread's parts and settings, each passed once at start"
    )]
    pub(crate) fn new(
        shared: Arc<Shared>,
        engine: Engine,
        handoff: Arc<TypedHandoff>,
        keys: Arc<KeyQueue>,
        injector_tx: Sender<InjectionRequest>,
        heartbeat: Box<dyn FnMut() + Send>,
        heartbeat_every: Duration,
        panel_lease: Duration,
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
            elsewhere: None,
            unconfirmed: None,
            trigger: None,
            panel: None,
            panel_lease,
            verdict: None,
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
            self.forget_if_asked();
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
        consider(self.panel.as_ref().map(|p| p.until));
        next
    }

    /// Carries out a forget the session watcher asked for (module docs).
    fn forget_if_asked(&mut self) {
        if self.shared.forget_pending.swap(false, Ordering::SeqCst) {
            self.forget();
        }
    }

    /// Overwrites the remembered form values and what the hand-off captured.
    fn forget(&mut self) {
        self.values_touched = None;
        self.engine.forget_form_values();
        self.handoff.disturb();
    }

    /// A verdict from the focus thread (or the watcher check): noted, and delivered as
    /// `own_window` while a panel has the keyboard (module docs).
    fn deliver_verdict(&mut self, state: SecureState) {
        self.verdict = Some(state);
        let state = if self.panel.is_some() {
            SecureState::OwnWindow
        } else {
            state
        };
        self.handoff.disturb();
        self.handle(Event::SecureState(state));
    }

    /// `PanelFocus` (module docs).
    fn panel_focus(&mut self, kind: PanelKind, shown: bool) {
        if shown {
            let until = Instant::now() + self.panel_lease;
            if let Some(panel) = &mut self.panel {
                panel.kind = kind;
                panel.until = until;
                return;
            }
            self.panel = Some(Panel { kind, until });
            self.shared
                .logger
                .info(format_args!("the {} panel has the keyboard", kind.as_str()));
            self.handoff.disturb();
            self.handle(Event::SecureState(SecureState::OwnWindow));
        } else if self.panel.as_ref().is_some_and(|p| p.kind == kind) {
            self.end_panel();
        }
    }

    /// The panel no longer has the keyboard: the field's own verdict is in force again.
    fn end_panel(&mut self) {
        let Some(panel) = self.panel.take() else {
            return;
        };
        self.shared.logger.info(format_args!(
            "the {} panel gave the keyboard back",
            panel.kind.as_str()
        ));
        if let Some(state) = self.verdict {
            self.handoff.disturb();
            self.handle(Event::SecureState(state));
        }
    }

    /// A request that the UI host sends only once `kind`'s panel is hidden ends its hold.
    fn panel_hidden(&mut self, kind: PanelKind) {
        if self.panel.as_ref().is_some_and(|p| p.kind == kind) {
            self.end_panel();
        }
    }

    /// The last UI host disconnected: every form session it was sent is cancelled. Without
    /// this a form acknowledged by a UI host that then died stays open, and the engine refuses
    /// every typed match with `form_open` until the worker restarts.
    fn ui_host_gone(&mut self) {
        self.end_panel();
        self.ack_deadline = None;
        if matches!(
            &self.parked,
            Some(Parked {
                request: Explicit::Submit { .. },
                ..
            })
        ) {
            self.parked = None;
        }
        let sessions: Vec<FormSessionId> = self.forms.drain().map(|(s, _)| s).collect();
        for session in sessions {
            self.handle(Event::FormCancelled { session });
            self.shared.diagnostic(coded("form_ui_host_gone"));
        }
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
            self.shared.diagnostic(coded("form_not_acknowledged"));
        }
        if self.panel.as_ref().is_some_and(|p| now >= p.until) {
            // The UI host stopped renewing its panel's hold: it may hang. Keys count as typed
            // into the focused application again.
            self.shared.diagnostic(coded("panel_focus_expired"));
            self.end_panel();
        }
        if self
            .values_touched
            .is_some_and(|t| now >= t + FORM_VALUES_TTL)
        {
            self.values_touched = None;
            self.engine.forget_form_values();
        }
    }

    #[allow(clippy::too_many_lines, reason = "one arm per message")]
    fn on_message(&mut self, msg: EngineMsg) {
        self.forget_if_asked();
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
                // The disturb in `deliver_verdict`, as it is wired, never finds a capture:
                // each verdict follows its own `FocusChanged`, which disturbed already, and the
                // engine is suspended between the two, so nothing can be armed. It stays so
                // the hand-off's writer duty (disturb before every event that clears the
                // buffer) does not rest on that.
                self.deliver_verdict(state);
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
            EngineMsg::Request { request, focus } => {
                match &request {
                    Explicit::Submit { .. } => self.panel_hidden(PanelKind::Form),
                    Explicit::Expand(_) | Explicit::Reopen => self.panel_hidden(PanelKind::Palette),
                }
                self.on_request(request, focus);
            }
            EngineMsg::FormCancel(session) => {
                self.panel_hidden(PanelKind::Form);
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
            EngineMsg::UiHostGone => self.ui_host_gone(),
            EngineMsg::PanelFocus {
                panel,
                shown,
                reply,
            } => {
                self.panel_focus(panel, shown);
                let _ = reply.send(());
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
            // The forget itself ran at the top of this function, when the session watcher or
            // `Worker::forget_form_values` asked for it; the message only woke the thread.
            EngineMsg::ForgetFormValues => {}
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
        if self.panel.is_some()
            && matches!(
                event,
                ListenerEvent::Char { .. }
                    | ListenerEvent::Backspace { .. }
                    | ListenerEvent::Boundary { .. }
            )
        {
            // Typed into one of Iamb's panels: neither the hand-off nor the engine sees it
            // (module docs). Dropping the event drops its character.
            return;
        }
        // A stamp the inspector contradicted counts as none (module docs).
        let target = target.filter(|t| self.unconfirmed != Some(*t));
        if let (Some(target), Some(focus)) = (target, &self.focus)
            && !target.is(&focus.token)
        {
            // The key went to a window the focus thread has not reported yet: neither the
            // engine nor the hand-off may see it (module docs).
            self.handoff.disturb();
            self.went_elsewhere(target);
            return;
        }
        self.trigger = target;
        self.route(event);
        self.trigger = None;
    }

    fn route(&mut self, event: ListenerEvent) {
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

    /// A key went to `target`, a window the focus thread has not reported: counts it, and
    /// reads the focus again when it is time (module docs).
    fn went_elsewhere(&mut self, target: KeyTarget) {
        let now = Instant::now();
        let episode = match &mut self.elsewhere {
            Some(e) if e.target == target => e,
            slot => slot.insert(Elsewhere {
                target,
                since: now,
                keys: 0,
                synced: None,
            }),
        };
        episode.keys = episode.keys.saturating_add(1);
        let due = match episode.synced {
            None => {
                episode.keys >= RESYNC_AFTER_KEYS
                    || now.duration_since(episode.since) >= RESYNC_AFTER
            }
            Some(at) => now.duration_since(at) >= RESYNC_AFTER,
        };
        if !due {
            return;
        }
        let first = episode.synced.is_none();
        episode.synced = Some(now);
        let lasted = now.duration_since(episode.since) >= RESYNC_AFTER;
        if first {
            self.shared.diagnostic(coded("keys_for_unreported_window"));
        }
        self.resync(target, lasted);
    }

    /// Reads where the focus is now. Another window than the last report goes to the focus
    /// thread as a change; the same window, once the keys have gone elsewhere for
    /// [`RESYNC_AFTER`], makes `target` a stamp that is no longer trusted (module docs).
    ///
    /// The application and the window come from one consistent reading
    /// ([`FocusInspector::focus_now`](iamb_platform::FocusInspector::focus_now)): the change
    /// is judged by the application's name and its keys by the window, so a focus change
    /// between two separate reads would pair one application's name with another's window
    /// (the close-out review's C3).
    fn resync(&mut self, target: KeyTarget, lasted: bool) {
        let Ok(change) = self.shared.platform.focus_now() else {
            return;
        };
        let token = change.token;
        if self.focus.as_ref().map(|c| c.token) != Some(token) {
            self.shared.logger.info(format_args!(
                "keys went to a window the focus watcher did not report; the focus was read again"
            ));
            let _ = self.shared.focus_tx.send(FocusMsg::Change(change));
        } else if lasted {
            self.shared.logger.warn(format_args!(
                "the listener names process {} for keys while the focus is in process {}; \
                 keys are handled without a target until the focus changes",
                target.pid(),
                token.pid()
            ));
            self.unconfirmed = Some(target);
            self.elsewhere = None;
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
            self.deliver_verdict(SecureState::Unknown);
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
        self.verdict = None;
        self.handle(Event::FocusChanged(change.app.app.clone()));
        let token = change.token;
        self.focus = Some(change);
        self.elsewhere = None;
        self.unconfirmed = None;
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
        let mut d = coded(code);
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
                    let mut wire = WireDiagnostic::from_engine(&d, crate::clock::timestamp());
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
            // Where its keys belong (module docs): the window every key let through was
            // compared with, or, before any focus report, the trigger key's own target.
            if let Some(target) = self
                .focus
                .as_ref()
                .map(|c| KeyTarget::window(c.token))
                .or(self.trigger)
            {
                request = request.for_target(target);
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
            let mut d = coded("form_no_ui_host");
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
