//! The search palette's flow in the UI host (architecture §4.5; ADR-0013 and its addendum;
//! `docs/ux/flows/5-search.md`, `docs/ux/spec.md` §4.5).
//!
//! The [`PaletteController`] owns one thread (`iamb-palette`) that carries the palette from the
//! moment it is asked for to the moment it goes away. Nothing here touches Tauri: the panel is a
//! [`PaletteHost`] (`app::palette_panel` is the real one), the service a
//! [`FormService`](crate::form::FormService) (the [`Bridge`](crate::bridge::Bridge)), and the
//! focus hand-back an [`iamb_platform::FocusInspector`]. So the whole flow runs in tests against
//! a real worker without a window (`tests/palette.rs`). It is the fill-in form's flow
//! (`crate::form`, whose module docs explain the keyboard and the lease at length) with a query
//! in place of the fields.
//!
//! # Who asks for it, and who registers the shortcut
//!
//! The service registers the global shortcut (the `palette_hotkey` setting, `PutSetting`;
//! architecture §6.3), listens for it, captures a focus token and sends `PaletteRequested`
//! to the UI host. The UI host never registers a shortcut of its own: a second registration of
//! the same chord would be refused by the system and would not work while the service is down.
//! The tray's "Search snippets…" and ⌘K / Ctrl+K inside the library window ask for the palette
//! directly ([`PaletteController::open_from_tray`], [`PaletteController::open_in_library`]).
//!
//! # A palette, step by step
//!
//! 1. `PaletteRequested` reaches the bridge's reader thread, which stamps its arrival and passes
//!    it here (never blocking the reader). While a fill-in form is open it does nothing (spec
//!    §4.5); while the palette is open it closes it ("pressing it again closes the palette").
//! 2. The panel's front end hears of it ([`PaletteEvent::Open`]), clears its query and fetches
//!    what to show ([`PaletteController::current`], the `palette_current` command).
//! 3. `PanelFocus { palette, shown: true }`, and the answer is awaited: only then may the panel
//!    take the keyboard (the 2.8 review's finding 16). A service that does not answer leaves
//!    the palette closed.
//! 4. The panel is shown, in the upper third of the display, with the keyboard. The lease is
//!    renewed every [`LEASE_RENEWAL`](crate::form::LEASE_RENEWAL) while it holds the keyboard.
//! 5. A pick ([`PaletteController::pick`]): the panel hides, the focus goes back to the token's
//!    window and is confirmed (`restore_focus`), then `PanelFocus { shown: false }`, then
//!    `ExpandSnippet { id, focus }` (a snippet with blanks leads the service to send
//!    `FormRequested`, and the form panel takes over: the palette has hidden and released the
//!    keyboard before the service hears the pick, so the two never hold it together). When
//!    the focus cannot be given back nothing is sent: the panel comes back and the front end
//!    says so. "Fill in … again" does the same with `ReopenLastForm { focus }`.
//! 6. Esc ([`Pick::Close`]): the panel hides, the focus goes back, `PanelFocus { shown: false }`.
//!    Nothing else is sent.
//! 7. Opened from Iamb's own window (the global shortcut while the library had the focus, or
//!    ⌘K inside it), a pick selects the snippet in the library instead of typing it, and
//!    "New snippet" and the review or the repair of a snippet open the library too
//!    ([`LibraryRequest`]).
//!
//! # When the panel loses the keyboard
//!
//! As the form's does (`crate::form`): a click into the app behind it sends `PanelFocus
//! { shown: false }` at once and the panel stays; while another application than the token's is
//! in front it steps aside, and it comes back with its application. A palette whose window is
//! gone (closed, or its application quit) closes as Esc closes it, and so does a palette whose
//! window the system closes (Alt+F4; [`PaletteController::close_requested`]).
//!
//! # The tray menu takes the foreground (the Phase 3 UI review's finding 4)
//!
//! On Windows the tray crate brings its own hidden window to the foreground before it shows
//! the menu, so when "Search snippets…" or "Fill in the last form again…" is chosen, the
//! window in front is Iamb's own, not the one the user was typing in. The tray therefore tells
//! the controller when the pointer reaches the icon ([`PaletteController::tray_pointer`], on
//! the icon's pointer events, which come before the menu): the window in front then, if it is
//! another process's, is remembered for [`TRAY_CAPTURE_TTL`]. The menu's items use the window
//! in front when they run if that is another process's (macOS, whose menu bar does not change
//! the front application), and the remembered one otherwise
//! ([`PaletteController::tray_target`]).
//!
//! # Privacy (architecture §8, spec §9)
//!
//! The query is the user's text. It never reaches this module: it goes from the front end's
//! state to the one `Search` frame (`palette_data`), and the controller only hears of the
//! picks. A "New snippet labelled …" pick carries the query as the draft's label, which this
//! module passes on once and does not keep. Nothing here logs a label, a query or a snippet.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use iamb_core::model::Id;
use iamb_ipc::{EventBody, FocusRef, PanelKind, RequestBody};
use iamb_platform::{FocusInspector, FocusToken};
use serde::{Deserialize, Serialize};

use crate::bridge::CallError;
use crate::contract::CommandError;
use crate::form::{
    FOCUS_LOST, FOLLOW_EVERY, FormService, FormTimer, FormTiming, KEYBOARD_SETTLE, LEASE_RENEWAL,
    focus_lost,
};
use crate::log::Logger;
use crate::timing::wall_ms;

/// The Tauri event that carries a [`PaletteEvent`], sent to the palette panel only.
pub const PALETTE_EVENT: &str = "iamb:palette";

/// The error code of a pick or a close for a palette that is not open.
pub const NO_PALETTE: &str = "no_palette";

/// How long the window in front when the pointer reached the tray icon stays the tray menu's
/// target (module docs).
pub const TRAY_CAPTURE_TTL: Duration = Duration::from_mins(1);

/// What the Rust side tells the palette panel, on [`PALETTE_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaletteEvent {
    /// The palette was asked for: the front end forgets its query, fetches [`PaletteOpen`] with
    /// `palette_current` and draws the empty state, while the panel is still hidden.
    Open {
        /// The session.
        session: u64,
    },
    /// The panel is visible and has the keyboard (the front end reports its next paint).
    Shown {
        /// The session.
        session: u64,
    },
    /// The palette is over (picked, closed, replaced, or abandoned because the service went
    /// away): the front end forgets its query.
    Closed {
        /// The session.
        session: u64,
    },
}

/// What the panel shows when it opens (the answer of `palette_current`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteOpen {
    /// The palette session; every answer names it.
    pub session: u64,
    /// Opened from Iamb's own window: a pick selects the snippet in the library.
    pub own_window: bool,
    /// The name of the application the text goes into, when the platform can name it (none can
    /// yet: spec §11.2 entry 13).
    pub app: Option<String>,
    /// The label of the snippet whose form was submitted in the last 30 minutes ("Fill in
    /// “Late delivery” again"), if any.
    pub last_form: Option<String>,
}

/// What the library window is asked to do after a pick in the palette.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LibraryRequest {
    /// A new snippet whose label is `label` ("New snippet labelled “warranty”"): the one
    /// place a query is used for anything else, and only after the user chose to.
    New {
        /// The draft's label.
        label: String,
    },
    /// Select the snippet and show it in the editor ("edit", "review", "open", or a pick made
    /// from the library's own window).
    Show {
        /// The snippet's id.
        id: String,
    },
    /// Show one of the window's views (the menu's "Settings…" and "Why didn't it expand?").
    View {
        /// Which.
        view: LibraryView,
    },
}

/// A view of the library window a [`LibraryRequest::View`] opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryView {
    /// Settings.
    Settings,
    /// "Why didn't it expand?".
    Diagnostics,
}

impl std::fmt::Debug for LibraryRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The label is what the user typed.
        match self {
            Self::New { .. } => f.write_str("LibraryRequest::New { label: <redacted> }"),
            Self::Show { id } => write!(f, "LibraryRequest::Show {{ id: {id} }}"),
            Self::View { view } => write!(f, "LibraryRequest::View {{ view: {view:?} }}"),
        }
    }
}

/// What the user did with the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Type the snippet where the cursor was (`ExpandSnippet`).
    Insert(Id),
    /// Open the last form again, pre-filled (`ReopenLastForm`).
    FillAgain,
    /// Hand something to the library window.
    Library(LibraryRequest),
    /// Esc: close, change nothing.
    Close,
}

/// The palette's window and front end, as the flow sees them.
pub trait PaletteHost: Send + Sync + 'static {
    /// Sends `event` to the panel's front end, and to no other window.
    fn send(&self, event: &PaletteEvent);
    /// Places the panel, shows it and gives it the keyboard.
    ///
    /// # Errors
    ///
    /// A sentence for the log when the window is gone or refuses.
    fn show(&self) -> Result<(), String>;
    /// Hides the panel.
    fn hide(&self);
    /// Shows the panel once where it cannot be seen and without the keyboard, then hides it
    /// (the first paint's cost, `windows::prepaint_panel`). Default: nothing.
    fn prepaint(&self) {}
    /// Opens the library window and has it do `request`.
    fn to_library(&self, request: LibraryRequest);
}

/// How the controller is built.
#[derive(Clone)]
pub struct PaletteConfig {
    /// The service.
    pub service: Arc<dyn FormService>,
    /// The panel.
    pub host: Arc<dyn PaletteHost>,
    /// Gives the focus back and says which window is in front; `None` where the UI host has no
    /// platform layer (Linux, development only): nothing is handed back.
    pub focus: Option<Arc<dyn FocusInspector>>,
    /// The UI host's log.
    pub logger: Arc<Logger>,
    /// The UI host's own process (its windows are not "another application").
    pub own_pid: u32,
    /// Whether a fill-in form is open (the shortcut then does nothing).
    pub form_open: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The label of the form submitted in the last 30 minutes, if any.
    pub last_form: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// Normally [`LEASE_RENEWAL`].
    pub lease_renewal: Duration,
    /// Normally [`FOLLOW_EVERY`].
    pub follow_every: Duration,
    /// Normally [`KEYBOARD_SETTLE`].
    pub keyboard_settle: Duration,
    /// Whether the end of the connection to the service closes an open palette; normally true
    /// (false for the measurement, which has no service).
    pub follows_connection: bool,
    /// Told of each pick just before it is sent (`crate::notice`); nothing by default.
    pub on_request: crate::form::OnRequest,
}

impl PaletteConfig {
    /// The defaults.
    #[must_use]
    pub fn new(
        service: Arc<dyn FormService>,
        host: Arc<dyn PaletteHost>,
        focus: Option<Arc<dyn FocusInspector>>,
        logger: Arc<Logger>,
    ) -> Self {
        Self {
            service,
            host,
            focus,
            logger,
            own_pid: std::process::id(),
            form_open: Arc::new(|| false),
            last_form: Arc::new(|| None),
            lease_renewal: LEASE_RENEWAL,
            follow_every: FOLLOW_EVERY,
            keyboard_settle: KEYBOARD_SETTLE,
            follows_connection: true,
            on_request: Arc::new(|_, _| {}),
        }
    }
}

type Reply = Sender<Result<(), CommandError>>;

enum Msg {
    /// The palette was asked for; `token` is the window the text goes into.
    Requested {
        token: Option<FocusToken>,
        /// The request came from Iamb's own library window (⌘K), whatever the token says.
        local: bool,
        at: Instant,
        wall_ms: f64,
    },
    /// The tray asked: the application in front now is the one the text goes into.
    FromTray {
        at: Instant,
        wall_ms: f64,
    },
    Pick {
        session: u64,
        pick: Pick,
        reply: Reply,
    },
    Keyboard(bool),
    ConnectionLost,
    Prepaint,
    CloseRequested,
}

/// The window in front when the pointer last reached the tray icon, and when (module docs).
type TrayCapture = Arc<Mutex<Option<(Instant, FocusToken)>>>;

/// The open palette.
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
    followed: Instant,
    /// A change of the panel's keyboard the window reported, not settled yet.
    keyboard: Option<(bool, Instant)>,
}

/// The palette's flow (module docs). Cheap to share; [`stop`](Self::stop) ends it.
pub struct PaletteController {
    tx: Mutex<Option<Sender<Msg>>>,
    current: Arc<Mutex<Option<PaletteOpen>>>,
    timer: Arc<FormTimer>,
    prepainted: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    focus: Option<Arc<dyn FocusInspector>>,
    own_pid: u32,
    tray: TrayCapture,
}

impl std::fmt::Debug for PaletteController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaletteController")
            .field("open", &lock(&self.current).as_ref().map(|o| o.session))
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PaletteController {
    /// Starts the palette's thread.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn start(cfg: PaletteConfig) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let current = Arc::new(Mutex::new(None));
        let timer = Arc::new(FormTimer::default());
        let prepainted = Arc::new(AtomicBool::new(false));
        let focus = cfg.focus.clone();
        let own_pid = cfg.own_pid;
        let tray: TrayCapture = Arc::new(Mutex::new(None));
        let mut flow = Flow {
            tray: Arc::clone(&tray),
            cfg,
            rx,
            backlog: VecDeque::new(),
            open: None,
            sessions: AtomicU64::new(0),
            current: Arc::clone(&current),
            timer: Arc::clone(&timer),
            prepainted: Arc::clone(&prepainted),
        };
        let thread = thread::Builder::new()
            .name("iamb-palette".to_owned())
            .spawn(move || flow.run())?;
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            current,
            timer,
            prepainted,
            thread: Mutex::new(Some(thread)),
            focus,
            own_pid,
            tray,
        })
    }

    fn send(&self, msg: Msg) -> bool {
        lock(&self.tx)
            .as_ref()
            .is_some_and(|tx| tx.send(msg).is_ok())
    }

    /// A service event, from the bridge's reader thread: a `PaletteRequested` is taken, stamped
    /// now; everything else is ignored. Never blocks.
    pub fn service_event(&self, event: &EventBody) {
        self.service_event_at(event, Instant::now(), wall_ms());
    }

    /// [`service_event`](Self::service_event) with the arrival given (the measurement).
    pub fn service_event_at(&self, event: &EventBody, at: Instant, wall_ms: f64) {
        if let EventBody::PaletteRequested { focus } = event {
            self.send(Msg::Requested {
                token: Some(FocusToken::from(*focus)),
                local: false,
                at,
                wall_ms,
            });
        }
    }

    /// Opens the palette for the window `focus` (the measurement and tests; the service's
    /// `PaletteRequested` does the same).
    pub fn open_for(&self, focus: Option<FocusRef>, at: Instant, wall_ms: f64) {
        self.send(Msg::Requested {
            token: focus.map(FocusToken::from),
            local: false,
            at,
            wall_ms,
        });
    }

    /// The pointer is over the tray icon, before its menu takes the foreground: the window in
    /// front now, if it is another process's, is the menu's target (module docs). `entered` is
    /// the pointer's arrival on the icon, which always counts; a later move over it counts only
    /// when nothing fresh was noted, since a press on the icon may already have brought the
    /// taskbar to the front. Asks the window system one cheap question.
    pub fn tray_pointer(&self, entered: bool) {
        let Some(focus) = &self.focus else {
            return;
        };
        let mut tray = lock(&self.tray);
        if !entered && tray.is_some_and(|(at, _)| at.elapsed() < TRAY_CAPTURE_TTL) {
            return;
        }
        if let Ok(front) = focus.focus_token()
            && front.pid() != self.own_pid
        {
            *tray = Some((Instant::now(), front));
        }
    }

    /// The window the tray menu's items are for (module docs): the one in front now when it is
    /// another process's, else the one in front when the pointer last reached the icon, within
    /// [`TRAY_CAPTURE_TTL`]; `None` when neither (no platform layer, or only Iamb's windows).
    #[must_use]
    pub fn tray_target(&self) -> Option<FocusToken> {
        tray_target(self.focus.as_deref(), self.own_pid, &self.tray)
    }

    /// The system asked to close the panel's window (Alt+F4): the open palette closes as Esc
    /// closes it. Never blocks.
    pub fn close_requested(&self) {
        self.send(Msg::CloseRequested);
    }

    /// The tray's "Search snippets…": opens the palette for the window the menu is for
    /// ([`tray_target`](Self::tray_target)). Never blocks.
    pub fn open_from_tray(&self) {
        self.send(Msg::FromTray {
            at: Instant::now(),
            wall_ms: wall_ms(),
        });
    }

    /// ⌘K / Ctrl+K inside the library window: opens the palette over it; a pick selects the
    /// snippet in the library. Never blocks.
    pub fn open_in_library(&self) {
        self.send(Msg::Requested {
            token: None,
            local: true,
            at: Instant::now(),
            wall_ms: wall_ms(),
        });
    }

    /// The panel's page has loaded (the `palette_ready` command): the panel paints once while
    /// hidden ([`PaletteHost::prepaint`]), the first time only, on the palette's thread, so that
    /// no request can show the panel meanwhile (it waits its turn). Never blocks.
    pub fn prepaint(&self) {
        self.send(Msg::Prepaint);
    }

    /// Whether the panel has painted once while hidden.
    #[must_use]
    pub fn prepainted(&self) -> bool {
        self.prepainted.load(Ordering::SeqCst)
    }

    /// The connection to the service ended: an open palette is over.
    pub fn connection_lost(&self) {
        self.send(Msg::ConnectionLost);
    }

    /// The panel's window gained (`true`) or lost the keyboard. Never blocks.
    pub fn keyboard(&self, has: bool) {
        self.send(Msg::Keyboard(has));
    }

    /// The front end painted `session`'s panel at `painted_at_ms` after it was shown.
    pub fn painted(&self, session: u64, painted_at_ms: f64) -> Option<FormTiming> {
        self.timer.painted(session, painted_at_ms)
    }

    /// The timings of the palettes opened.
    #[must_use]
    pub fn timer(&self) -> &FormTimer {
        &self.timer
    }

    /// The palette that is open, if any (for a panel whose page loaded after the request).
    #[must_use]
    pub fn current(&self) -> Option<PaletteOpen> {
        lock(&self.current).clone()
    }

    /// Carries out `pick` for `session`; blocks until the service answered.
    ///
    /// # Errors
    ///
    /// [`FOCUS_LOST`] when the focus could not be given back (the panel is shown again and
    /// nothing was sent); [`NO_PALETTE`] when `session` is not open; the call's errors.
    pub fn pick(&self, session: u64, pick: Pick) -> Result<(), CommandError> {
        let (reply, answer) = mpsc::channel();
        if !self.send(Msg::Pick {
            session,
            pick,
            reply,
        }) {
            return Err(CommandError::new("internal", "the palette has stopped"));
        }
        answer
            .recv()
            .map_err(|_| CommandError::new("internal", "the palette has stopped"))?
    }

    /// Stops the thread (an open palette is left to the service, which ends its hold when the
    /// UI host's connection ends).
    pub fn stop(&self) {
        lock(&self.tx).take();
        if let Some(handle) = lock(&self.thread).take() {
            let _ = handle.join();
        }
        lock(&self.current).take();
    }
}

impl Drop for PaletteController {
    fn drop(&mut self) {
        self.stop();
    }
}

/// See [`PaletteController::tray_target`].
fn tray_target(
    focus: Option<&dyn FocusInspector>,
    own_pid: u32,
    tray: &Mutex<Option<(Instant, FocusToken)>>,
) -> Option<FocusToken> {
    let front = focus?.focus_token().ok();
    if let Some(front) = front.filter(|t| t.pid() != own_pid) {
        return Some(front);
    }
    lock(tray)
        .filter(|(at, _)| at.elapsed() < TRAY_CAPTURE_TTL)
        .map(|(_, token)| token)
        .or(front)
}

/// The thread's state.
struct Flow {
    cfg: PaletteConfig,
    tray: TrayCapture,
    rx: Receiver<Msg>,
    backlog: VecDeque<Msg>,
    open: Option<Open>,
    sessions: AtomicU64,
    current: Arc<Mutex<Option<PaletteOpen>>>,
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
            Msg::Requested {
                token,
                local,
                at,
                wall_ms,
            } => self.requested(token, local, at, wall_ms),
            Msg::FromTray { at, wall_ms } => {
                let token = tray_target(self.cfg.focus.as_deref(), self.cfg.own_pid, &self.tray);
                self.requested(token, false, at, wall_ms);
            }
            Msg::CloseRequested => {
                if let Some(open) = self.open.take() {
                    self.log().info(format_args!(
                        "palette {}: the system closed the panel",
                        open.session
                    ));
                    self.dismiss(&open);
                }
            }
            Msg::Pick {
                session,
                pick,
                reply,
            } => {
                let _ = reply.send(self.pick(session, pick));
            }
            Msg::Keyboard(has) => {
                if let Some(open) = &mut self.open {
                    open.keyboard = Some((has, Instant::now()));
                }
            }
            Msg::Prepaint => {
                // Once (a page that loads again has painted before), and never over an open
                // palette (which has painted).
                if self.open.is_none() && !self.prepainted.swap(true, Ordering::SeqCst) {
                    let started = Instant::now();
                    self.cfg.host.prepaint();
                    self.log().info(format_args!(
                        "palette panel painted once while hidden ({:.0} ms)",
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                }
            }
            Msg::ConnectionLost if !self.cfg.follows_connection => {}
            Msg::ConnectionLost => {
                if let Some(open) = self.open.take() {
                    self.log().info(format_args!(
                        "palette {}: the connection to the service ended; the palette is closed",
                        open.session
                    ));
                    if open.visible {
                        self.cfg.host.hide();
                        // Best effort: the palette is over either way.
                        let _ = self.give_back(&open);
                    }
                    self.finish(open.session);
                }
            }
        }
    }

    fn call(&self, body: RequestBody) -> Result<iamb_ipc::ResponseBody, CallError> {
        self.cfg.service.call(body)
    }

    fn panel_focus(&self, shown: bool) -> Result<(), CallError> {
        self.call(RequestBody::PanelFocus {
            panel: PanelKind::Palette,
            shown,
        })
        .map(|_| ())
    }

    /// Ends the palette on the Rust side: the front end forgets its query.
    fn finish(&mut self, session: u64) {
        lock(&self.current).take();
        self.cfg.host.send(&PaletteEvent::Closed { session });
    }

    fn requested(&mut self, token: Option<FocusToken>, local: bool, at: Instant, wall_ms: f64) {
        if (self.cfg.form_open)() {
            // A fill-in form is open: the shortcut does nothing (spec §4.5).
            self.log()
                .info(format_args!("palette: not opened, a form is open"));
            return;
        }
        if let Some(open) = self.open.take() {
            // The shortcut again closes the palette.
            self.log().info(format_args!(
                "palette {}: closed by the shortcut",
                open.session
            ));
            self.dismiss(&open);
            return;
        }
        let session = self.sessions.fetch_add(1, Ordering::SeqCst) + 1;
        let own_window = local || token.is_some_and(|t| t.pid() == self.cfg.own_pid);
        self.timer.start(session, at, wall_ms);
        *lock(&self.current) = Some(PaletteOpen {
            session,
            own_window,
            app: None,
            last_form: (self.cfg.last_form)(),
        });
        self.cfg.host.send(&PaletteEvent::Open { session });
        if let Err(e) = self.panel_focus(true) {
            self.log().warn(format_args!(
                "palette {session}: PanelFocus was not answered ({}); the palette stays closed",
                e.code()
            ));
            self.finish(session);
            return;
        }
        let mut open = Open {
            session,
            token,
            held: true,
            renewed: Instant::now(),
            visible: false,
            away: false,
            followed: Instant::now(),
            keyboard: None,
        };
        if let Err(e) = self.cfg.host.show() {
            self.log().warn(format_args!(
                "palette {session}: the panel cannot be shown: {e}"
            ));
            let _ = self.panel_focus(false);
            self.finish(session);
            return;
        }
        open.visible = true;
        // A panel that was shown has painted: no prepaint is needed any more.
        self.prepainted.store(true, Ordering::SeqCst);
        self.timer.shown(session);
        self.cfg.host.send(&PaletteEvent::Shown { session });
        self.log().info(format_args!(
            "palette {session}: shown {:.1} ms after the request arrived",
            at.elapsed().as_secs_f64() * 1000.0
        ));
        self.open = Some(open);
    }

    /// Gives the focus back to the palette's window; `Ok` when there is no window to give it to
    /// (the library's own, or none) or no platform layer to do it with.
    fn give_back(&self, open: &Open) -> Result<(), iamb_platform::PlatformError> {
        match (&self.cfg.focus, open.token) {
            (Some(focus), Some(token)) if token.pid() != self.cfg.own_pid => {
                focus.restore_focus(&token)
            }
            _ => Ok(()),
        }
    }

    /// Closes `open` as Esc does: hide, give the focus back (best effort), release the keyboard.
    fn dismiss(&mut self, open: &Open) {
        if open.visible {
            self.cfg.host.hide();
        }
        if let Err(e) = self.give_back(open) {
            // Closing changes nothing, wherever the focus is.
            self.log().info(format_args!(
                "palette {}: the focus could not be given back ({e})",
                open.session
            ));
        }
        if open.held {
            let _ = self.panel_focus(false);
        }
        self.finish(open.session);
    }

    fn not_open(session: u64) -> CommandError {
        CommandError::new(NO_PALETTE, format!("palette {session} is not open"))
    }

    fn pick(&mut self, session: u64, pick: Pick) -> Result<(), CommandError> {
        let Some(mut open) = self.open.take_if(|o| o.session == session) else {
            return Err(Self::not_open(session));
        };
        let request = match pick {
            Pick::Close => {
                self.dismiss(&open);
                self.log().info(format_args!("palette {session}: closed"));
                return Ok(());
            }
            Pick::Library(request) => {
                // The library window takes the focus itself; nothing is typed anywhere.
                if open.visible {
                    self.cfg.host.hide();
                }
                if open.held {
                    let _ = self.panel_focus(false);
                }
                self.finish(session);
                self.cfg.host.to_library(request);
                self.log()
                    .info(format_args!("palette {session}: handed to the library"));
                return Ok(());
            }
            Pick::Insert(id) => RequestBody::ExpandSnippet {
                id,
                focus: open.token.map(FocusRef::from),
            },
            Pick::FillAgain => RequestBody::ReopenLastForm {
                focus: open.token.map(FocusRef::from),
            },
        };
        self.cfg.host.hide();
        open.visible = false;
        if let Err(e) = self.give_back(&open) {
            // Flow 5: nothing is sent; the panel comes back.
            self.log().info(format_args!(
                "palette {session}: the focus could not be given back ({e}); nothing was sent"
            ));
            self.take_keyboard(&mut open);
            self.open = Some(open);
            return Err(focus_lost());
        }
        if open.held && self.panel_focus(false).is_err() {
            // `ExpandSnippet` and `ReopenLastForm` end the hold too.
            self.log().info(format_args!(
                "palette {session}: PanelFocus(false) was not answered"
            ));
        }
        match &request {
            RequestBody::ExpandSnippet { id, .. } => {
                (self.cfg.on_request)(crate::notice::Asked::Insert, Some(*id));
            }
            _ => (self.cfg.on_request)(crate::notice::Asked::Reopen, None),
        }
        let answer = self.call(request);
        self.finish(session);
        match answer {
            Ok(_) => {
                self.log().info(format_args!("palette {session}: picked"));
                Ok(())
            }
            Err(e) => {
                self.log().warn(format_args!(
                    "palette {session}: the pick failed: {}",
                    e.code()
                ));
                Err(crate::bridge::command(e))
            }
        }
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
                        "palette {}: PanelFocus was not answered ({}); the panel stays without \
                         the keyboard",
                        open.session,
                        e.code()
                    ));
                    return;
                }
            }
        }
        match self.cfg.host.show() {
            Ok(()) => {
                open.visible = true;
                open.away = false;
            }
            Err(e) => self.log().warn(format_args!(
                "palette {}: the panel cannot be shown: {e}",
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
                // first (`crate::form`).
                match self.panel_focus(true) {
                    Ok(()) => {
                        open.held = true;
                        open.renewed = Instant::now();
                    }
                    Err(e) => self.log().warn(format_args!(
                        "palette {}: PanelFocus was not answered ({})",
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
                            "palette {}: the panel lost the keyboard; it stays open",
                            open.session
                        ));
                    }
                    Err(e) => self.log().warn(format_args!(
                        "palette {}: PanelFocus(false) was not answered ({})",
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
                        "palette {}: the PanelFocus lease was not renewed ({})",
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
            if !self.follow(&mut open) {
                return;
            }
        }
        self.open = Some(open);
    }

    /// Follows the front application; `false` when the palette's window is gone and the
    /// palette closed.
    fn follow(&mut self, open: &mut Open) -> bool {
        let (Some(focus), Some(token)) = (self.cfg.focus.clone(), open.token) else {
            return true;
        };
        if token.pid() != self.cfg.own_pid && !focus.window_exists(&token) {
            self.log().info(format_args!(
                "palette {}: the window it is for is gone; closed",
                open.session
            ));
            self.dismiss(open);
            return false;
        }
        let Ok(front) = focus.focus_token() else {
            return true;
        };
        if front.pid() == self.cfg.own_pid {
            return true;
        }
        if front.pid() == token.pid() {
            if open.away {
                self.log().info(format_args!(
                    "palette {}: its application is back in front; so is the panel",
                    open.session
                ));
                self.take_keyboard(open);
            }
        } else if !open.away && open.visible {
            self.log().info(format_args!(
                "palette {}: another application is in front; the panel steps aside",
                open.session
            ));
            self.cfg.host.hide();
            open.visible = false;
            open.away = true;
        }
        true
    }
}

/// The error code of a pick whose focus could not be given back (the form's, shared).
pub const PALETTE_FOCUS_LOST: &str = FOCUS_LOST;

/// The shortcut the palette has until the user chooses another (spec §12, decision 2).
pub const DEFAULT_SHORTCUT: &str = "Ctrl+Shift+Space";

/// The file in the run directory that says the default shortcut was offered to the service.
const SEEDED: &str = "palette-shortcut-seeded";

/// Registers [`DEFAULT_SHORTCUT`] with the service the first time this UI host meets a service
/// that has no `palette_hotkey` setting, so that the palette opens from the global shortcut out
/// of the box. The service registers only what the setting says (architecture §6.3); it has no
/// default of its own.
///
/// It is offered once: a marker file in `run_dir` remembers it, so that a user who clears the
/// shortcut in Settings does not get it back. A system that refuses the chord (another
/// application holds it) leaves no marker, and the next start tries again; Settings → Shortcuts
/// shows the choice meanwhile. Returns whether the shortcut is now registered by this call.
pub fn seed_default_shortcut(
    bridge: &crate::bridge::Bridge,
    run_dir: &std::path::Path,
    log: &Logger,
) -> bool {
    let marker = run_dir.join(SEEDED);
    if marker.exists() {
        return false;
    }
    let remember = || {
        if let Err(e) = std::fs::write(&marker, b"") {
            log.warn(format_args!("cannot write {SEEDED}: {e}"));
        }
    };
    match crate::settings::settings(bridge) {
        Ok(settings) if settings.contains_key("palette_hotkey") => {
            remember();
            false
        }
        Ok(_) => {
            match crate::settings::put_setting(bridge, "palette_hotkey", Some(DEFAULT_SHORTCUT)) {
                Ok(()) => {
                    remember();
                    log.info(format_args!("the default palette shortcut is registered"));
                    true
                }
                Err(e) => {
                    log.info(format_args!(
                    "the default palette shortcut was not registered ({}); Settings has the choice",
                    e.code
                ));
                    false
                }
            }
        }
        Err(e) => {
            log.info(format_args!(
                "the palette shortcut was not looked at ({})",
                e.code
            ));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_have_the_front_ends_shape() {
        assert_eq!(
            serde_json::to_value(PaletteEvent::Open { session: 3 }).expect("json"),
            serde_json::json!({"type": "open", "session": 3})
        );
        assert_eq!(
            serde_json::to_value(PaletteEvent::Closed { session: 3 }).expect("json"),
            serde_json::json!({"type": "closed", "session": 3})
        );
    }

    #[test]
    fn the_open_palette_says_whose_window_it_is() {
        let open = PaletteOpen {
            session: 1,
            own_window: true,
            app: None,
            last_form: Some("Late delivery".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(open).expect("json"),
            serde_json::json!({
                "session": 1, "ownWindow": true, "app": null, "lastForm": "Late delivery"
            })
        );
    }

    #[test]
    fn a_library_request_never_prints_a_label() {
        let new = LibraryRequest::New {
            label: "warranty".to_owned(),
        };
        assert!(!format!("{new:?}").contains("warranty"));
        assert_eq!(
            serde_json::to_value(&new).expect("json"),
            serde_json::json!({"kind": "new", "label": "warranty"})
        );
        let show: LibraryRequest =
            serde_json::from_value(serde_json::json!({"kind": "show", "id": "abc"})).expect("json");
        assert_eq!(show, LibraryRequest::Show { id: "abc".into() });
    }
}
