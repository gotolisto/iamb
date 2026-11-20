//! The fill-in form's flow in the UI host (architecture §4.5; ADR-0013 and its addendum;
//! `docs/ux/flows/4-fill-in.md`, `docs/ux/spec.md` §4.4).
//!
//! The [`FormController`] owns one thread (`iamb-form`) that takes each `FormRequested` the
//! bridge hears and carries the form to its end. Nothing here touches Tauri: the panel is a
//! [`PanelHost`] (`app::form_panel` is the real one), the service a [`FormService`] (the
//! [`Bridge`]), and the focus hand-back an [`iamb_platform::FocusInspector`] (the operating
//! system's, or the mock's in tests). So the whole flow runs in tests against a real worker
//! without a window (`tests/form.rs`).
//!
//! # A form, step by step
//!
//! 1. `FormRequested` reaches the bridge's reader thread, which stamps its arrival and passes
//!    it here (never blocking the reader). A request that waited longer than [`ACK_LIMIT`]
//!    before this thread took it is answered with `FormCancel`: the service gave up on it
//!    already (flow 4, "the UI host does not acknowledge within 2 s").
//! 2. `FormAck`, at once, well inside the service's 2 s.
//! 3. `GetSnippet` for the header (the label and the abbreviation; the request carries only
//!    the snippet's id). A failure leaves the header empty; the form still works.
//! 4. The panel's front end hears of the form ([`FormEvent::Open`]) and fetches it
//!    ([`FormController::current`], the `form_current` command), and renders it while the
//!    panel is still hidden and reports its height ([`FormController::laid_out`]).
//! 5. `PanelFocus { form, shown: true }`, and the answer is awaited: only then may the panel
//!    take the keyboard (the 2.8 review's finding 16). A refusal cancels the form.
//! 6. The panel is shown, in the upper third of the display, and takes the keyboard. The
//!    lease is renewed every [`LEASE_RENEWAL`] while the panel holds the keyboard.
//! 7. Insert ([`FormController::submit`]): the panel hides, the focus goes back to the token's
//!    window and is confirmed (`restore_focus`), then `PanelFocus { shown: false }`, then
//!    `FormSubmit`. When the focus cannot be given back, nothing is sent: the panel comes
//!    back and the front end shows "Iamb could not get back to …".
//! 8. Cancel ([`FormController::cancel`]): the panel hides, the focus goes back (best effort),
//!    `PanelFocus { shown: false }`, `FormCancel`.
//!
//! The service ends the panel's hold by itself on `FormSubmit` and `FormCancel`; the
//! `shown: false` before them makes the order explicit and costs one message.
//!
//! # When the panel loses the keyboard
//!
//! A click into the app behind the panel (or any other window) takes the keyboard from it:
//! `PanelFocus { shown: false }` is sent at once, so the keys typed there count as typed into
//! that app again. The panel stays open (spec §4.4: clicking outside does not close it). While
//! it does not hold the keyboard the UI host watches which application is in front: when
//! another application than the target comes to the front the panel steps aside (hidden, still
//! open); when the target application comes back, so does the panel, and it takes the keyboard
//! again (after `PanelFocus { shown: true }`). When the user clicks back into the panel, the
//! window system gives it the keyboard before the UI host hears of it; `PanelFocus { shown:
//! true }` follows as soon as it does.
//!
//! # Privacy (architecture §8, spec §9)
//!
//! Form values are user-entered text. They exist here only while a form is open: the last
//! values of the request (to answer [`FormController::current`]) and, for the length of one
//! call, the values the front end submits. Both are held in [`Secret`], which prints as
//! `<redacted>`, and the remembered values are overwritten when the form closes. Nothing here
//! logs a value, a label or a field name: the log has session numbers, outcomes and timings.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use iamb_core::model::Id;
use iamb_core::template::overwrite;
use iamb_ipc::{EventBody, PanelKind, RequestBody, ResponseBody, Secret, WireField, WireValues};
use iamb_platform::{FocusInspector, FocusToken};
use serde::Serialize;

use crate::bridge::{Bridge, CallError};
use crate::contract::{CommandError, FieldView};
use crate::log::Logger;
use crate::timing::wall_ms;

/// The Tauri event that carries a [`FormEvent`], sent to the form panel only.
pub const FORM_EVENT: &str = "iamb:form";

/// A `FormRequested` that waited this long before the UI host could take it is cancelled: the
/// service abandons an unacknowledged form after 2 s (architecture §4.5).
pub const ACK_LIMIT: Duration = Duration::from_secs(2);

/// How often `PanelFocus { shown: true }` is sent again while the panel holds the keyboard
/// (the lease is 30 s, `iamb_ipc::PANEL_FOCUS_LEASE`).
pub const LEASE_RENEWAL: Duration = Duration::from_secs(10);

/// How long the panel waits for the front end's height before it is shown anyway.
pub const LAYOUT_WAIT: Duration = Duration::from_millis(80);

/// How often the front application is looked at while the panel does not hold the keyboard.
pub const FOLLOW_EVERY: Duration = Duration::from_millis(200);

/// How long a change of the panel's keyboard must last before it counts. Showing a window can
/// make the window system report a loss and a gain within a few milliseconds (seen under X11);
/// acting on each would send `PanelFocus` twice for nothing.
pub const KEYBOARD_SETTLE: Duration = Duration::from_millis(60);

/// The error code of a submit whose focus could not be given back.
pub const FOCUS_LOST: &str = "focus_lost";

/// The error code of a submit or cancel for a form that is not open.
pub const NO_FORM: &str = "no_form";

/// The error of a submit whose focus could not be given back ([`FOCUS_LOST`]).
#[must_use]
pub fn focus_lost() -> CommandError {
    CommandError::new(
        FOCUS_LOST,
        "the window to return to is gone or would not take the focus",
    )
}

/// What the form panel shows: the answer of `form_current` (see [`FormEvent::Open`]).
///
/// `last_values` are user-entered text (the values last submitted for this snippet, held by
/// the service's worker); they are serialised only into the one event that carries them to
/// the panel, and `Debug` prints them as `<redacted>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormOpen {
    /// The form session; every answer names it.
    pub session: u64,
    /// The snippet's id.
    pub snippet: String,
    /// The snippet's label (empty when it has none, or could not be read).
    pub label: String,
    /// The snippet's abbreviation (empty when it could not be read).
    pub abbreviation: String,
    /// The name of the application the text goes into, when the platform can name it (none
    /// can yet: `docs/ux/spec.md` §11.2 entry 13).
    pub app: Option<String>,
    /// The fields, in the order they first appear in the snippet.
    pub fields: Vec<FieldView>,
    /// The values last submitted for this snippet, to pre-fill; `None` when there are none.
    pub last_values: Option<Secret<WireValues>>,
}

/// The header of a form: the snippet's label and abbreviation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// The label.
    pub label: String,
    /// The abbreviation.
    pub abbreviation: String,
}

/// Builds what the panel shows from a request's parts and the snippet's header.
#[must_use]
pub fn form_open(
    session: u64,
    snippet: Id,
    spec: &[WireField],
    last_values: Option<&Secret<WireValues>>,
    header: &Header,
    app: Option<String>,
) -> FormOpen {
    FormOpen {
        session,
        snippet: snippet.to_string(),
        label: header.label.clone(),
        abbreviation: header.abbreviation.clone(),
        app,
        fields: spec.iter().map(FieldView::from).collect(),
        last_values: last_values
            .filter(|values| !values.expose().is_empty())
            .cloned(),
    }
}

/// The header in a `Snippet` response; `None` for any other.
#[must_use]
pub fn header_from(response: &ResponseBody) -> Option<Header> {
    match response {
        ResponseBody::Snippet { snippet, .. } => Some(Header {
            label: snippet.label.clone(),
            abbreviation: snippet.abbreviation.as_str().to_owned(),
        }),
        _ => None,
    }
}

/// What the Rust side tells the form panel, on [`FORM_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FormEvent {
    /// A form to show: the front end fetches it with the `form_current` command, which
    /// answers the `form` window only, so that the values never travel in an event another
    /// window could listen to. The panel is still hidden; it is shown once the front end
    /// reported its height, or after [`LAYOUT_WAIT`].
    Open {
        /// The session.
        session: u64,
    },
    /// The panel is visible and has the keyboard (the front end reports its next paint).
    Shown {
        /// The session.
        session: u64,
    },
    /// The form is over (submitted, cancelled, or abandoned because the service went away):
    /// the front end forgets its values.
    Closed {
        /// The session.
        session: u64,
    },
}

/// The service, as the form sees it: the bridge, or a stand-in for the measurement.
pub trait FormService: Send + Sync + 'static {
    /// Sends one request and waits for its answer.
    ///
    /// # Errors
    ///
    /// The call's errors.
    fn call(&self, body: RequestBody) -> Result<ResponseBody, CallError>;
}

impl FormService for Bridge {
    fn call(&self, body: RequestBody) -> Result<ResponseBody, CallError> {
        Self::call(self, body)
    }
}

/// The form panel's window and front end, as the form sees them.
pub trait PanelHost: Send + Sync + 'static {
    /// Sends `event` to the panel's front end, and to no other window.
    fn send(&self, event: &FormEvent);
    /// Places the panel (the content is `height` CSS pixels high, when known), shows it and
    /// gives it the keyboard.
    ///
    /// # Errors
    ///
    /// A sentence for the log when the window is gone or refuses.
    fn show(&self, height: Option<f64>) -> Result<(), String>;
    /// Hides the panel.
    fn hide(&self);
    /// Shows the panel once where it cannot be seen and without the keyboard, then hides it,
    /// so that the first form does not pay for the web view's first paint. Default: nothing.
    fn prepaint(&self) {}
}

/// How the controller is built.
#[derive(Clone)]
pub struct FormConfig {
    /// The service.
    pub service: Arc<dyn FormService>,
    /// The panel.
    pub host: Arc<dyn PanelHost>,
    /// Gives the focus back and says which window is in front; `None` where the UI host has
    /// no platform layer (Linux, development only): nothing is handed back.
    pub focus: Option<Arc<dyn FocusInspector>>,
    /// The UI host's log.
    pub logger: Arc<Logger>,
    /// The UI host's own process (its windows are not "another application").
    pub own_pid: u32,
    /// Normally [`ACK_LIMIT`].
    pub ack_limit: Duration,
    /// Normally [`LEASE_RENEWAL`].
    pub lease_renewal: Duration,
    /// Normally [`LAYOUT_WAIT`].
    pub layout_wait: Duration,
    /// Normally [`FOLLOW_EVERY`].
    pub follow_every: Duration,
    /// Normally [`KEYBOARD_SETTLE`].
    pub keyboard_settle: Duration,
    /// Whether the end of the connection to the service closes an open form; normally true
    /// (false for the measurement, which has no service).
    pub follows_connection: bool,
}

impl FormConfig {
    /// The defaults.
    #[must_use]
    pub fn new(
        service: Arc<dyn FormService>,
        host: Arc<dyn PanelHost>,
        focus: Option<Arc<dyn FocusInspector>>,
        logger: Arc<Logger>,
    ) -> Self {
        Self {
            service,
            host,
            focus,
            logger,
            own_pid: std::process::id(),
            ack_limit: ACK_LIMIT,
            lease_renewal: LEASE_RENEWAL,
            layout_wait: LAYOUT_WAIT,
            follow_every: FOLLOW_EVERY,
            keyboard_settle: KEYBOARD_SETTLE,
            follows_connection: true,
        }
    }
}

/// One form opened, measured (the KPI "visible in under 150 ms", `docs/test-strategy.md` §8.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormTiming {
    /// From the request's arrival at the UI host to the panel's first paint, by the wall clock
    /// (the front end's paint time minus the arrival).
    pub paint_ms: f64,
    /// From the arrival until the panel was shown (the window call returned), monotonic.
    pub shown_ms: f64,
    /// From the arrival until the paint report reached Rust, monotonic (an upper bound).
    pub report_ms: f64,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    session: u64,
    at: Instant,
    wall_ms: f64,
    shown: Option<Instant>,
}

/// The pending form's timing and the last result.
#[derive(Debug, Default)]
pub struct FormTimer {
    state: Mutex<(Option<Pending>, Option<FormTiming>)>,
    done: Condvar,
}

impl FormTimer {
    fn lock(&self) -> MutexGuard<'_, (Option<Pending>, Option<FormTiming>)> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(&self, session: u64, at: Instant, wall_ms: f64) {
        *self.lock() = (
            Some(Pending {
                session,
                at,
                wall_ms,
                shown: None,
            }),
            None,
        );
    }

    fn shown(&self, session: u64) {
        if let Some(p) = self.lock().0.as_mut().filter(|p| p.session == session) {
            p.shown = Some(Instant::now());
        }
    }

    /// The panel of `session` painted at `painted_at_ms` (epoch milliseconds). Returns the
    /// timing when that form was being timed; a paint of another or a later one is ignored.
    pub fn painted(&self, session: u64, painted_at_ms: f64) -> Option<FormTiming> {
        let arrived = Instant::now();
        let mut state = self.lock();
        let pending = state
            .0
            .filter(|p| p.session == session && p.shown.is_some())?;
        state.0 = None;
        let shown = pending.shown.unwrap_or(arrived);
        let timing = FormTiming {
            paint_ms: painted_at_ms - pending.wall_ms,
            shown_ms: shown.duration_since(pending.at).as_secs_f64() * 1000.0,
            report_ms: arrived.duration_since(pending.at).as_secs_f64() * 1000.0,
        };
        state.1 = Some(timing);
        self.done.notify_all();
        Some(timing)
    }

    /// Waits up to `timeout` for the next timing.
    pub fn wait(&self, timeout: Duration) -> Option<FormTiming> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if let Some(t) = state.1.take() {
                return Some(t);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

type Reply = Sender<Result<(), CommandError>>;

/// A `FormRequested`, taken apart.
struct Request {
    session: u64,
    snippet: Id,
    spec: Vec<WireField>,
    focus: Option<FocusToken>,
    last_values: Option<Secret<WireValues>>,
    at: Instant,
    wall_ms: f64,
}

enum Msg {
    Requested(Box<Request>),
    LaidOut {
        session: u64,
        height: f64,
    },
    Submit {
        session: u64,
        values: Secret<WireValues>,
        reply: Reply,
    },
    Cancel {
        session: u64,
        reply: Reply,
    },
    Keyboard(bool),
    ConnectionLost,
    Prepaint,
}

/// The open form.
struct Open {
    session: u64,
    token: Option<FocusToken>,
    /// `PanelFocus { shown: true }` is in force (as far as the UI host knows).
    held: bool,
    /// When it was last sent.
    renewed: Instant,
    /// The panel is on screen.
    visible: bool,
    /// The panel stepped aside because another application is in front.
    away: bool,
    height: Option<f64>,
    followed: Instant,
    /// A change of the panel's keyboard the window reported, not settled yet.
    keyboard: Option<(bool, Instant)>,
}

/// The form's flow (module docs). Cheap to share; [`stop`](Self::stop) ends it.
pub struct FormController {
    tx: Mutex<Option<Sender<Msg>>>,
    current: Arc<Mutex<Option<FormOpen>>>,
    timer: Arc<FormTimer>,
    service: Arc<dyn FormService>,
    prepainted: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for FormController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormController")
            .field("open", &lock(&self.current).as_ref().map(|o| o.session))
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Overwrites the values of `form` before they are dropped (§4.5: forgotten values are
/// overwritten, not only freed).
fn forget(form: Option<FormOpen>) {
    forget_values(form.and_then(|f| f.last_values));
}

/// Overwrites `values` before they are dropped.
fn forget_values(values: Option<Secret<WireValues>>) {
    if let Some(values) = values {
        for mut value in values.into_inner().into_values() {
            overwrite(&mut value);
        }
    }
}

impl FormController {
    /// Starts the form's thread.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn start(cfg: FormConfig) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let current = Arc::new(Mutex::new(None));
        let timer = Arc::new(FormTimer::default());
        let service = Arc::clone(&cfg.service);
        let prepainted = Arc::new(AtomicBool::new(false));
        let mut flow = Flow {
            cfg,
            rx,
            backlog: VecDeque::new(),
            open: None,
            current: Arc::clone(&current),
            timer: Arc::clone(&timer),
            prepainted: Arc::clone(&prepainted),
        };
        let thread = thread::Builder::new()
            .name("iamb-form".to_owned())
            .spawn(move || flow.run())?;
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            current,
            timer,
            service,
            prepainted,
            thread: Mutex::new(Some(thread)),
        })
    }

    fn send(&self, msg: Msg) -> bool {
        lock(&self.tx)
            .as_ref()
            .is_some_and(|tx| tx.send(msg).is_ok())
    }

    /// A service event, from the bridge's reader thread: a `FormRequested` is taken, stamped
    /// now; everything else is ignored. Never blocks.
    pub fn service_event(&self, event: &EventBody) {
        self.service_event_at(event, Instant::now(), wall_ms());
    }

    /// [`service_event`](Self::service_event) with the arrival given (the measurement, and
    /// tests of a late request).
    pub fn service_event_at(&self, event: &EventBody, at: Instant, wall_ms: f64) {
        if let EventBody::FormRequested {
            session,
            snippet,
            spec,
            focus,
            last_values,
        } = event
        {
            self.send(Msg::Requested(Box::new(Request {
                session: *session,
                snippet: *snippet,
                spec: spec.clone(),
                focus: focus.map(FocusToken::from),
                last_values: last_values.clone(),
                at,
                wall_ms,
            })));
        }
    }

    /// The panel's page has loaded (the `form_ready` command): the panel paints once while
    /// hidden ([`PanelHost::prepaint`]), the first time only, on the form's thread, so that no
    /// request can show the panel meanwhile (it waits its turn). Never blocks.
    pub fn prepaint(&self) {
        self.send(Msg::Prepaint);
    }

    /// Whether the panel has painted once while hidden.
    #[must_use]
    pub fn prepainted(&self) -> bool {
        self.prepainted.load(Ordering::SeqCst)
    }

    /// The connection to the service ended: an open form is over.
    pub fn connection_lost(&self) {
        self.send(Msg::ConnectionLost);
    }

    /// The panel's window gained (`true`) or lost the keyboard. Never blocks.
    pub fn keyboard(&self, has: bool) {
        self.send(Msg::Keyboard(has));
    }

    /// The front end rendered `session`'s form; its content is `height` CSS pixels high.
    pub fn laid_out(&self, session: u64, height: f64) {
        self.send(Msg::LaidOut { session, height });
    }

    /// The front end painted `session`'s panel at `painted_at_ms` after it was shown.
    pub fn painted(&self, session: u64, painted_at_ms: f64) -> Option<FormTiming> {
        self.timer.painted(session, painted_at_ms)
    }

    /// The timings of the forms opened.
    #[must_use]
    pub fn timer(&self) -> &FormTimer {
        &self.timer
    }

    /// The form that is open, if any (for a panel whose page loaded after the request).
    #[must_use]
    pub fn current(&self) -> Option<FormOpen> {
        lock(&self.current).clone()
    }

    fn ask(&self, make: impl FnOnce(Reply) -> Msg) -> Result<(), CommandError> {
        let (reply, answer) = mpsc::channel();
        if !self.send(make(reply)) {
            return Err(CommandError::new("internal", "the form has stopped"));
        }
        answer
            .recv()
            .map_err(|_| CommandError::new("internal", "the form has stopped"))?
    }

    /// Insert: hides the panel, gives the focus back, then sends the values. Blocks until the
    /// service answered.
    ///
    /// # Errors
    ///
    /// [`FOCUS_LOST`] when the focus could not be given back (the panel is shown again and
    /// nothing was sent); [`NO_FORM`] when `session` is not open; the call's errors.
    pub fn submit(&self, session: u64, values: WireValues) -> Result<(), CommandError> {
        let values = Secret::new(values);
        self.ask(|reply| Msg::Submit {
            session,
            values,
            reply,
        })
    }

    /// Cancel: hides the panel, gives the focus back, and cancels the session.
    ///
    /// # Errors
    ///
    /// [`NO_FORM`] when `session` is not open; the call's errors.
    pub fn cancel(&self, session: u64) -> Result<(), CommandError> {
        self.ask(|reply| Msg::Cancel { session, reply })
    }

    /// Asks the service to show the last submitted form again (`ReopenLastForm`); the form
    /// arrives as a new `FormRequested`.
    ///
    /// # Errors
    ///
    /// The call's errors.
    pub fn reopen_last(&self) -> Result<(), CommandError> {
        match self.service.call(RequestBody::ReopenLastForm {}) {
            Ok(_) => Ok(()),
            Err(e) => Err(crate::bridge::command(e)),
        }
    }

    /// Stops the thread (an open form is left to the service, which cancels it when the UI
    /// host's connection ends).
    pub fn stop(&self) {
        lock(&self.tx).take();
        if let Some(handle) = lock(&self.thread).take() {
            let _ = handle.join();
        }
        forget(lock(&self.current).take());
    }
}

impl Drop for FormController {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The thread's state.
struct Flow {
    cfg: FormConfig,
    rx: Receiver<Msg>,
    /// Messages that arrived while a request waited for its height.
    backlog: VecDeque<Msg>,
    open: Option<Open>,
    current: Arc<Mutex<Option<FormOpen>>>,
    timer: Arc<FormTimer>,
    prepainted: Arc<AtomicBool>,
}

impl Flow {
    fn log(&self) -> &Logger {
        &self.cfg.logger
    }

    fn run(&mut self) {
        loop {
            let msg = if let Some(msg) = self.backlog.pop_front() {
                msg
            } else {
                match self.next_wake() {
                    Some(wait) => match self.rx.recv_timeout(wait) {
                        Ok(msg) => msg,
                        Err(RecvTimeoutError::Timeout) => {
                            self.tick();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                    None => match self.rx.recv() {
                        Ok(msg) => msg,
                        Err(_) => break,
                    },
                }
            };
            self.on(msg);
            self.tick();
        }
    }

    /// How long until the next renewal or look at the front application.
    fn next_wake(&self) -> Option<Duration> {
        let open = self.open.as_ref()?;
        let mut wait: Option<Duration> = None;
        if open.held {
            wait = Some(
                (open.renewed + self.cfg.lease_renewal).saturating_duration_since(Instant::now()),
            );
        }
        if self.following(open) {
            let follow =
                (open.followed + self.cfg.follow_every).saturating_duration_since(Instant::now());
            wait = Some(wait.map_or(follow, |w| w.min(follow)));
        }
        if let Some((_, at)) = open.keyboard {
            let settle = (at + self.cfg.keyboard_settle).saturating_duration_since(Instant::now());
            wait = Some(wait.map_or(settle, |w| w.min(settle)));
        }
        wait
    }

    fn following(&self, open: &Open) -> bool {
        self.cfg.focus.is_some() && open.token.is_some() && (open.away || !open.held)
    }

    fn on(&mut self, msg: Msg) {
        match msg {
            Msg::Requested(request) => self.requested(*request),
            // A height that came after the panel was shown: the panel keeps its size while the
            // user types (design system §4.14).
            Msg::LaidOut { .. } => {}
            Msg::Submit {
                session,
                values,
                reply,
            } => {
                let _ = reply.send(self.submit(session, values));
            }
            Msg::Cancel { session, reply } => {
                let _ = reply.send(self.cancel(session));
            }
            Msg::Keyboard(has) => {
                if let Some(open) = &mut self.open {
                    open.keyboard = Some((has, Instant::now()));
                }
            }
            Msg::Prepaint => {
                // Once (a page that loads again has painted before), and never over an open
                // form (which has painted).
                if self.open.is_none() && !self.prepainted.swap(true, Ordering::SeqCst) {
                    let started = Instant::now();
                    self.cfg.host.prepaint();
                    self.log().info(format_args!(
                        "form panel painted once while hidden ({:.0} ms)",
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                }
            }
            Msg::ConnectionLost if !self.cfg.follows_connection => {}
            Msg::ConnectionLost => {
                if let Some(open) = self.open.take() {
                    self.log().info(format_args!(
                        "form {}: the connection to the service ended; the form is closed",
                        open.session
                    ));
                    if open.visible {
                        self.cfg.host.hide();
                        // Best effort: the form is over either way.
                        let _ = self.give_back(&open);
                    }
                    self.close(open.session);
                }
            }
        }
    }

    fn call(&self, body: RequestBody) -> Result<ResponseBody, CallError> {
        self.cfg.service.call(body)
    }

    fn panel_focus(&self, shown: bool) -> Result<(), CallError> {
        self.call(RequestBody::PanelFocus {
            panel: PanelKind::Form,
            shown,
        })
        .map(|_| ())
    }

    /// Ends the form on the Rust side: the front end forgets, the values are overwritten.
    fn close(&mut self, session: u64) {
        self.cfg.host.send(&FormEvent::Closed { session });
        forget(lock(&self.current).take());
    }

    fn requested(&mut self, mut request: Request) {
        let session = request.session;
        if let Some(old) = self.open.take() {
            // The service holds one form at a time, so a new request means the old one is
            // gone (a service that restarted). Its panel makes way.
            self.log().info(format_args!(
                "form {}: replaced by form {session}",
                old.session
            ));
            self.close(old.session);
        }
        let waited = request.at.elapsed();
        if waited >= self.cfg.ack_limit {
            self.log().info(format_args!(
                "form {session}: the request waited {:.0} ms; cancelled",
                waited.as_secs_f64() * 1000.0
            ));
            let _ = self.call(RequestBody::FormCancel { session });
            return;
        }
        if let Err(e) = self.call(RequestBody::FormAck { session }) {
            self.log()
                .warn(format_args!("form {session}: FormAck failed: {}", e.code()));
            return;
        }
        self.timer.start(session, request.at, request.wall_ms);
        let header = self
            .call(crate::contract::get_snippet(request.snippet))
            .ok()
            .as_ref()
            .and_then(header_from)
            .unwrap_or_default();
        let form = form_open(
            session,
            request.snippet,
            &request.spec,
            request.last_values.as_ref(),
            &header,
            None,
        );
        forget(lock(&self.current).replace(form));
        self.cfg.host.send(&FormEvent::Open { session });
        forget_values(request.last_values.take());
        // The front end renders while the panel is hidden; wait for its height, briefly.
        let height = self.wait_layout(session);
        if let Err(e) = self.panel_focus(true) {
            self.log().warn(format_args!(
                "form {session}: PanelFocus was not answered ({}); the form is cancelled",
                e.code()
            ));
            let _ = self.call(RequestBody::FormCancel { session });
            self.close(session);
            return;
        }
        let mut open = Open {
            session,
            token: request.focus,
            held: true,
            renewed: Instant::now(),
            visible: false,
            away: false,
            height,
            followed: Instant::now(),
            keyboard: None,
        };
        if let Err(e) = self.cfg.host.show(height) {
            self.log().warn(format_args!(
                "form {session}: the panel cannot be shown: {e}"
            ));
            let _ = self.panel_focus(false);
            let _ = self.call(RequestBody::FormCancel { session });
            self.close(session);
            return;
        }
        open.visible = true;
        // A panel that was shown has painted: no prepaint is needed any more.
        self.prepainted.store(true, Ordering::SeqCst);
        self.timer.shown(session);
        self.cfg.host.send(&FormEvent::Shown { session });
        self.log().info(format_args!(
            "form {session}: shown {:.1} ms after the request arrived",
            request.at.elapsed().as_secs_f64() * 1000.0
        ));
        self.open = Some(open);
    }

    /// Waits up to the layout wait for the front end's height of `session`; other messages
    /// are kept for later, in order.
    fn wait_layout(&mut self, session: u64) -> Option<f64> {
        let deadline = Instant::now() + self.cfg.layout_wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match self.rx.recv_timeout(left) {
                Ok(Msg::LaidOut { session: s, height }) if s == session => {
                    return Some(height);
                }
                Ok(Msg::LaidOut { .. }) => {}
                Ok(other) => self.backlog.push_back(other),
                Err(_) => return None,
            }
        }
    }

    /// Gives the focus back to the form's window; `Ok` when there is no window to give it to
    /// or no platform layer to do it with.
    fn give_back(&self, open: &Open) -> Result<(), iamb_platform::PlatformError> {
        match (&self.cfg.focus, open.token) {
            (Some(focus), Some(token)) => focus.restore_focus(&token),
            _ => Ok(()),
        }
    }

    fn not_open(session: u64) -> CommandError {
        CommandError::new(NO_FORM, format!("form {session} is not open"))
    }

    fn submit(&mut self, session: u64, values: Secret<WireValues>) -> Result<(), CommandError> {
        let Some(mut open) = self.open.take_if(|o| o.session == session) else {
            forget_values(Some(values));
            return Err(Self::not_open(session));
        };
        self.cfg.host.hide();
        open.visible = false;
        if let Err(e) = self.give_back(&open) {
            // Flow 4: nothing is sent; the panel comes back with every answer.
            self.log().info(format_args!(
                "form {session}: the focus could not be given back ({e}); nothing was sent"
            ));
            forget_values(Some(values));
            self.take_keyboard(&mut open);
            self.open = Some(open);
            return Err(focus_lost());
        }
        if open.held && self.panel_focus(false).is_err() {
            // `FormSubmit` ends the hold too.
            self.log().info(format_args!(
                "form {session}: PanelFocus(false) was not answered"
            ));
        }
        let answer = self.call(RequestBody::FormSubmit { session, values });
        self.close(session);
        match answer {
            Ok(_) => {
                self.log().info(format_args!("form {session}: submitted"));
                Ok(())
            }
            Err(e) => {
                self.log().warn(format_args!(
                    "form {session}: FormSubmit failed: {}",
                    e.code()
                ));
                Err(crate::bridge::command(e))
            }
        }
    }

    fn cancel(&mut self, session: u64) -> Result<(), CommandError> {
        let Some(open) = self.open.take_if(|o| o.session == session) else {
            return Err(Self::not_open(session));
        };
        if open.visible {
            self.cfg.host.hide();
        }
        if let Err(e) = self.give_back(&open) {
            // Cancelling changes nothing, wherever the focus is.
            self.log().info(format_args!(
                "form {session}: the focus could not be given back ({e})"
            ));
        }
        if open.held {
            let _ = self.panel_focus(false);
        }
        let answer = self.call(RequestBody::FormCancel { session });
        self.close(session);
        self.log().info(format_args!("form {session}: cancelled"));
        answer.map(|_| ()).map_err(crate::bridge::command)
    }

    /// Shows the panel again with the keyboard, after `PanelFocus { shown: true }`.
    fn take_keyboard(&self, open: &mut Open) {
        if !open.held || open.renewed.elapsed() >= self.cfg.lease_renewal {
            match self.panel_focus(true) {
                Ok(()) => {
                    open.held = true;
                    open.renewed = Instant::now();
                }
                Err(e) => {
                    self.log().warn(format_args!(
                        "form {}: PanelFocus was not answered ({}); the panel stays without the \
                         keyboard",
                        open.session,
                        e.code()
                    ));
                    return;
                }
            }
        }
        match self.cfg.host.show(open.height) {
            Ok(()) => {
                open.visible = true;
                open.away = false;
            }
            Err(e) => self.log().warn(format_args!(
                "form {}: the panel cannot be shown: {e}",
                open.session
            )),
        }
    }

    fn keyboard(&mut self, has: bool) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        if open.visible {
            if has && !open.held {
                // The user clicked back into the panel: the window system gave it the keyboard
                // first (module docs).
                match self.panel_focus(true) {
                    Ok(()) => {
                        open.held = true;
                        open.renewed = Instant::now();
                        self.log().info(format_args!(
                            "form {}: the panel has the keyboard again",
                            open.session
                        ));
                    }
                    Err(e) => self.log().warn(format_args!(
                        "form {}: PanelFocus was not answered ({})",
                        open.session,
                        e.code()
                    )),
                }
            } else if !has && open.held {
                match self.panel_focus(false) {
                    Ok(()) => {
                        open.held = false;
                        open.followed = Instant::now();
                        self.log().info(format_args!(
                            "form {}: the panel lost the keyboard; it stays open",
                            open.session
                        ));
                    }
                    Err(e) => self.log().warn(format_args!(
                        "form {}: PanelFocus(false) was not answered ({})",
                        open.session,
                        e.code()
                    )),
                }
            }
        }
        self.open = Some(open);
    }

    /// Renews the lease, and follows the front application while the panel does not hold the
    /// keyboard (module docs).
    fn tick(&mut self) {
        if let Some(open) = &mut self.open
            && let Some((has, at)) = open.keyboard
            && at.elapsed() >= self.cfg.keyboard_settle
        {
            open.keyboard = None;
            self.keyboard(has);
        }
        let Some(mut open) = self.open.take() else {
            return;
        };
        if open.held && open.renewed.elapsed() >= self.cfg.lease_renewal {
            match self.panel_focus(true) {
                Ok(()) => open.renewed = Instant::now(),
                Err(e) => {
                    self.log().warn(format_args!(
                        "form {}: the PanelFocus lease was not renewed ({})",
                        open.session,
                        e.code()
                    ));
                    // Tried again at the next wake, not in a tight loop.
                    open.renewed = Instant::now();
                }
            }
        }
        if self.following(&open) && open.followed.elapsed() >= self.cfg.follow_every {
            open.followed = Instant::now();
            self.follow(&mut open);
        }
        self.open = Some(open);
    }

    fn follow(&self, open: &mut Open) {
        let (Some(focus), Some(token)) = (&self.cfg.focus, open.token) else {
            return;
        };
        let Ok(front) = focus.focus_token() else {
            return;
        };
        if front.pid() == self.cfg.own_pid {
            return;
        }
        if front.pid() == token.pid() {
            if open.away {
                self.log().info(format_args!(
                    "form {}: its application is back in front; so is the panel",
                    open.session
                ));
                self.take_keyboard(open);
            }
        } else if !open.away && open.visible {
            self.log().info(format_args!(
                "form {}: another application is in front; the panel steps aside",
                open.session
            ));
            self.cfg.host.hide();
            open.visible = false;
            open.away = true;
        }
    }
}

/// A service that answers every request at once, for the measurement of the panel alone
/// (`--measure-form`): `Done`, and no snippet for the header.
#[derive(Debug, Default)]
pub struct InstantService;

impl FormService for InstantService {
    fn call(&self, body: RequestBody) -> Result<ResponseBody, CallError> {
        match body {
            RequestBody::GetSnippet { .. } => Err(CallError::Remote(iamb_ipc::ErrorBody::new(
                iamb_ipc::ErrorCode::NotFound,
                "no service in a measurement",
            ))),
            _ => Ok(ResponseBody::Done {}),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn a_form_open_carries_values_only_when_there_are_some() {
        let spec = vec![WireField {
            name: "Name".to_owned(),
            kind: "text".to_owned(),
            options: Vec::new(),
            default: String::new(),
        }];
        let header = Header {
            label: "Greeting".to_owned(),
            abbreviation: "hi".to_owned(),
        };
        let empty = Secret::new(BTreeMap::new());
        let open = form_open(
            1,
            Id::from_bytes([3; 16]),
            &spec,
            Some(&empty),
            &header,
            None,
        );
        assert_eq!(open.last_values, None);
        let values = Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())]));
        let open = form_open(
            1,
            Id::from_bytes([3; 16]),
            &spec,
            Some(&values),
            &header,
            None,
        );
        assert!(!format!("{open:?}").contains("Ada"), "Debug is redacted");
        let json = serde_json::to_value(&open).expect("json");
        assert_eq!(json["lastValues"]["Name"], "Ada");
        assert_eq!(json["label"], "Greeting");
        assert_eq!(
            serde_json::to_value(FormEvent::Open { session: 1 }).expect("json"),
            serde_json::json!({"type": "open", "session": 1})
        );
    }

    #[test]
    fn a_paint_is_timed_only_after_the_panel_was_shown() {
        let t = FormTimer::default();
        assert_eq!(t.painted(1, wall_ms()), None);
        t.start(1, Instant::now(), wall_ms());
        assert_eq!(t.painted(1, wall_ms()), None, "not shown yet");
        t.shown(2);
        assert_eq!(t.painted(1, wall_ms()), None, "another session");
        t.shown(1);
        let got = t.painted(1, wall_ms() + 5.0).expect("timed");
        assert!(got.paint_ms >= 5.0 && got.paint_ms < 1000.0, "{got:?}");
        assert!(got.shown_ms <= got.report_ms);
        assert_eq!(t.wait(Duration::ZERO), Some(got));
        assert_eq!(t.painted(1, wall_ms()), None, "one paint per form");
    }
}
