//! The worker: everything of architecture §3.1, on plain threads and channels (ADR-0009).
//!
//! | Thread | Code | Wakes on |
//! | --- | --- | --- |
//! | listener | the platform's own OS loop; our [`EventSink`] only `try_send`s | keys |
//! | engine | [`engine`] | a message; the 500 ms heartbeat; a form or request deadline |
//! | injector | [`threads::injector`] | an expansion to inject |
//! | focus | [`threads::focus`] | a focus change (the OS crate's watcher forwards it) |
//! | store | [`store`] | a write, a usage record |
//! | watchdog | [`threads::watchdog`] | every 30 s, for the listener self-test (§3.2) |
//! | accept | [`endpoint`], [`ipc`] | a client connects; while the endpoint cannot be served, every [`ENDPOINT_RETRY`] |
//! | reader + event pump, per client (at most 8) | [`ipc`], [`handlers`] | a request; an event to push |
//!
//! While idle only the heartbeat and the 30-second self-test fire; every other thread blocks
//! on a channel or the OS. [`Worker::wake_counts`] exposes how often each thread woke, which
//! `tests/idle.rs` checks.
//!
//! # Duties and where they are met
//!
//! - **Order of focus events and explicit requests** (§4.5): the engine thread forwards a palette
//!   pick or form submit only once it has delivered the `FocusChanged` for the request's window
//!   ([`engine`] "parked request").
//! - **Typed-characters hand-off** (§4.6): the engine thread is the hand-off's writer: it
//!   `observe`s every listener event before the engine sees it, `disturb`s before focus, verdict
//!   and pause events, `arm`s on every `Expanded`, and `finish`es on every injector outcome.
//! - **Every expansion answered** (§4.6): the injector thread answers each expansion with its
//!   outcome; the engine thread turns every outcome but `Finished` into `InjectionCancelled`.
//!   The injector handles one expansion at a time and the engine refuses explicit requests while
//!   one is in flight (`InjectionBusy`).
//! - **Fail closed** (§4.1): the focus thread waits at most 100 ms for the secure-field answer;
//!   anything but a confirmed normal field keeps the engine suspended.
//! - **Usage** is recorded by the store thread when the expansion's end marker arrives (or its
//!   injector reports `Finished`, whichever is first), never for a cancelled one.
//! - **Memory lock** (§4.1, §8 rule 1): the engine's keystroke buffer, the hand-off's storage
//!   and the key queue's character slots are locked at start ([`lock_typed_text`]).
//! - **Keys in flight** (§8 rule 1): the listener's sink puts each event in the [`keys`] queue,
//!   which overwrites a character as soon as the engine thread takes it; the engine channel
//!   carries only a notice.
//! - **Where a key went** (§4.1): a key the listener names another window for than the last
//!   focus change the engine heard of is not handed to the engine ([`engine`]).

mod endpoint;
mod engine;
mod handlers;
mod injection;
mod ipc;
mod keys;
mod store;
mod threads;

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::{Engine, ExpansionId, FormSessionId, SecureState, Snapshot};
use iamb_core::model::Id;
use iamb_core::sensitive::Sensitive;
use iamb_core::template::FormValues;
use iamb_ipc::{EventBody, InjectionStatus, ServiceStatus, WireDiagnostic};
use iamb_platform::{
    DEFAULT_EVENT_CAPACITY, EventSink, FocusChange, FocusSink, FocusToken, InjectionOutcome,
    InjectionRequest, LocalEndpoint, MemoryRegion, PeerPolicy, Platform, TypedHandoff,
};
use iamb_store::{Store, StoreError};

use crate::library::SharedView;
use crate::log::Logger;
use crate::paths::DataDirs;

pub use engine::{RESYNC_AFTER, RESYNC_AFTER_KEYS};
pub use ipc::MAX_CLIENTS;

/// How long the worker waits for `run/store.lock` at start (§6.3).
pub const STORE_LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often the engine thread sends a heartbeat (§3.2).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

/// How often the worker tries again to serve the service endpoint while it cannot (the 2.8
/// review's finding 12).
pub const ENDPOINT_RETRY: Duration = Duration::from_secs(5);

/// How often the listener's self-test runs (§3.2).
pub const SELF_TEST_INTERVAL: Duration = Duration::from_secs(30);

/// How long the self-test waits for its own marked event (§3.2).
pub const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(1);

/// How many diagnostics the ring keeps (§8 rule 4).
pub const DIAGNOSTICS_KEPT: usize = 200;

/// The hotkey id of the search palette.
pub const PALETTE_HOTKEY: iamb_platform::HotkeyId = iamb_platform::HotkeyId(1);

/// How the worker is started.
pub struct WorkerConfig {
    /// The data root.
    pub dirs: DataDirs,
    /// The operating system (the OS crate, or the mock).
    pub platform: Arc<dyn Platform>,
    /// Where the service endpoint is served: the native endpoint, or the mock's in tests.
    pub endpoint: Arc<dyn LocalEndpoint>,
    /// What clients must be.
    pub policy: PeerPolicy,
    /// Called from the engine thread every [`heartbeat_interval`](Self::heartbeat_interval).
    pub heartbeat: Box<dyn FnMut() + Send>,
    /// Normally [`HEARTBEAT_INTERVAL`].
    pub heartbeat_interval: Duration,
    /// Normally [`SELF_TEST_INTERVAL`].
    pub self_test_interval: Duration,
    /// Normally [`STORE_LOCK_WAIT`].
    pub store_lock_wait: Duration,
    /// Normally [`ENDPOINT_RETRY`].
    pub endpoint_retry: Duration,
    /// The clock templates see.
    pub clock: Arc<dyn Fn() -> LocalDateTime + Send + Sync>,
    /// Where log lines go.
    pub logger: Arc<Logger>,
}

impl WorkerConfig {
    /// The defaults for `dirs`, serving on `endpoint`: no heartbeat, the build's peer policy,
    /// the real clock, no log.
    #[must_use]
    pub fn new(
        dirs: DataDirs,
        platform: Arc<dyn Platform>,
        endpoint: Arc<dyn LocalEndpoint>,
    ) -> Self {
        Self {
            dirs,
            platform,
            endpoint,
            policy: PeerPolicy::for_build(),
            heartbeat: Box::new(|| {}),
            heartbeat_interval: HEARTBEAT_INTERVAL,
            self_test_interval: SELF_TEST_INTERVAL,
            store_lock_wait: STORE_LOCK_WAIT,
            endpoint_retry: ENDPOINT_RETRY,
            clock: Arc::new(crate::clock::local_now),
            logger: Arc::new(Logger::disabled()),
        }
    }
}

impl fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerConfig")
            .field("dirs", &self.dirs)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// Why the worker could not start.
#[derive(Debug)]
pub enum StartError {
    /// The store could not be opened (`Locked` when another process holds `run/store.lock`
    /// for longer than the wait).
    Store(StoreError),
    /// A thread could not be started.
    Thread(std::io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "cannot open the store: {}", store::Logged(e)),
            Self::Thread(e) => write!(f, "cannot start a thread: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// Why the worker ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// Asked to stop (the supervisor went away, or a test). Exit status 0.
    Stop,
    /// The listener failed its self-test twice: exit so the supervisor restarts the worker.
    ListenerDead,
}

/// How often each thread woke up (blocking waits that returned), for the idle test.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeCounts {
    /// Engine thread (messages and timer expiries, heartbeats included).
    pub engine: u64,
    /// Heartbeats sent.
    pub heartbeats: u64,
    /// Injector thread.
    pub injector: u64,
    /// Focus thread.
    pub focus: u64,
    /// Store thread.
    pub store: u64,
    /// Watchdog thread.
    pub watchdog: u64,
    /// Accept thread.
    pub accept: u64,
    /// All reader and event-pump threads together.
    pub clients: u64,
}

#[derive(Debug, Default)]
pub(crate) struct WakeCounters {
    engine: AtomicU64,
    heartbeats: AtomicU64,
    injector: AtomicU64,
    focus: AtomicU64,
    store: AtomicU64,
    watchdog: AtomicU64,
    accept: AtomicU64,
    clients: AtomicU64,
}

impl WakeCounters {
    fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> WakeCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        WakeCounts {
            engine: get(&self.engine),
            heartbeats: get(&self.heartbeats),
            injector: get(&self.injector),
            focus: get(&self.focus),
            store: get(&self.store),
            watchdog: get(&self.watchdog),
            accept: get(&self.accept),
            clients: get(&self.clients),
        }
    }
}

/// An explicit request (§4.5): it types into the focused field, so it waits for its focus.
pub(crate) enum Explicit {
    /// A palette pick or `ExpandSnippet`.
    Expand(Id),
    /// A submitted form.
    Submit {
        session: FormSessionId,
        values: Sensitive<FormValues>,
    },
    /// `ReopenLastForm`.
    Reopen,
}

/// What the engine thread is told. One bounded queue carries keys and everything else, so
/// the engine sees them in one order.
pub(crate) enum EngineMsg {
    /// From the listener's sink: one listener event waits in the [`keys::KeyQueue`]. The
    /// notice carries nothing, so no typed character stays behind in this channel.
    Key,
    /// From the focus thread, before it inspects the field.
    FocusChanged(FocusChange),
    /// From the focus thread: the verdict for the change before it.
    Verdict(SecureState),
    /// From the injector thread.
    Injected {
        expansion: ExpansionId,
        outcome: InjectionOutcome,
    },
    /// From a reader.
    Request {
        request: Explicit,
        focus: Option<FocusToken>,
    },
    /// From a reader.
    FormCancel(FormSessionId),
    /// From a reader.
    FormAck(FormSessionId),
    /// From a reader: the last UI host disconnected. Every form session it was sent can no
    /// longer be submitted or cancelled by anyone, so the engine cancels them all.
    UiHostGone,
    /// From a reader.
    SetPaused(bool),
    /// From the store thread.
    Snapshot(Snapshot),
    /// Screen lock, sign-out (from task 3.x), or a test.
    ForgetFormValues,
    /// From a reader: the engine's part of the status.
    Status(Sender<EngineStatus>),
    /// Answered at once: everything sent before it has been handled.
    Sync(Sender<()>),
    /// Answered when no injection is in flight.
    Settle(Sender<()>),
    /// Stop the engine thread.
    Stop,
    /// Test fault: the engine thread stops working (debug builds only).
    #[cfg(debug_assertions)]
    Hang,
}

/// The engine's part of [`ServiceStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EngineStatus {
    pub paused: bool,
    pub suspended: bool,
    pub injection: InjectionStatus,
}

/// Messages to the focus thread.
pub(crate) enum FocusMsg {
    /// The OS crate's watcher saw a change.
    Change(FocusChange),
    /// Exclusions or the test field changed: judge the current focus again.
    Rejudge,
    /// Passed on to the engine thread behind every change before it (for [`Worker::sync`]).
    Forward(EngineMsg),
    Stop,
}

/// The ring of recent diagnostics.
#[derive(Debug, Default)]
pub(crate) struct Diagnostics(Mutex<VecDeque<WireDiagnostic>>);

impl Diagnostics {
    pub(crate) fn push(&self, entry: WireDiagnostic) {
        let mut ring = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if ring.len() == DIAGNOSTICS_KEPT {
            ring.pop_front();
        }
        ring.push_back(entry);
    }

    pub(crate) fn all(&self) -> Vec<WireDiagnostic> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

/// The worker's exit request, waited on by [`Worker::wait_exit`].
#[derive(Debug, Default)]
pub(crate) struct ExitSignal {
    reason: Mutex<Option<ExitReason>>,
    changed: Condvar,
}

impl ExitSignal {
    pub(crate) fn request(&self, reason: ExitReason) {
        let mut r = self.reason.lock().unwrap_or_else(PoisonError::into_inner);
        if r.is_none() {
            *r = Some(reason);
        }
        drop(r);
        self.changed.notify_all();
    }

    fn wait(&self, timeout: Option<Duration>) -> Option<ExitReason> {
        let guard = self.reason.lock().unwrap_or_else(PoisonError::into_inner);
        match timeout {
            None => *self
                .changed
                .wait_while(guard, |r| r.is_none())
                .unwrap_or_else(PoisonError::into_inner),
            Some(t) => {
                *self
                    .changed
                    .wait_timeout_while(guard, t, |r| r.is_none())
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
        }
    }
}

/// What every worker thread shares.
pub(crate) struct Shared {
    pub logger: Arc<Logger>,
    pub view: SharedView,
    pub platform: Arc<dyn Platform>,
    pub engine_tx: SyncSender<EngineMsg>,
    pub focus_tx: Sender<FocusMsg>,
    pub store: store::StoreHandle,
    pub clients: ipc::Clients,
    pub diagnostics: Diagnostics,
    pub test_field: Mutex<Option<FocusToken>>,
    pub wakes: WakeCounters,
    pub listener_ok: AtomicBool,
    /// The focus watcher was started: the engine thread keeps it running.
    pub watching: AtomicBool,
    pub memory_locked: AtomicBool,
    /// What clients must be: also what roles their executables may take (finding 10).
    pub policy: PeerPolicy,
    pub exit: ExitSignal,
    pub clock: Arc<dyn Fn() -> LocalDateTime + Send + Sync>,
}

impl Shared {
    /// Sends to the engine thread, waiting if its queue is full.
    pub(crate) fn to_engine(&self, msg: EngineMsg) -> bool {
        self.engine_tx.send(msg).is_ok()
    }

    /// The engine's status, asked over its queue.
    pub(crate) fn engine_status(&self) -> Option<EngineStatus> {
        let (tx, rx) = channel();
        if !self.to_engine(EngineMsg::Status(tx)) {
            return None;
        }
        rx.recv_timeout(Duration::from_secs(2)).ok()
    }

    /// The whole status, from the engine's part.
    pub(crate) fn status(&self, engine: EngineStatus) -> ServiceStatus {
        let view = self.view.get();
        ServiceStatus {
            version: crate::VERSION.to_owned(),
            pid: std::process::id(),
            paused: engine.paused,
            suspended: engine.suspended,
            injection: engine.injection,
            snippets: u64::try_from(view.snapshot.len()).unwrap_or(u64::MAX),
            revision: view.revision,
            listener: self.listener_ok.load(Ordering::Relaxed),
            memory_locked: self.memory_locked.load(Ordering::Relaxed),
            clients: u32::try_from(self.clients.count()).unwrap_or(u32::MAX),
            ui_connected: self.clients.ui_connected(),
        }
    }

    /// Records a diagnostic in the ring, tells the UI, and logs its code.
    pub(crate) fn diagnostic(&self, mut entry: WireDiagnostic) {
        entry.at = Some(crate::clock::timestamp());
        self.logger.info(format_args!("diagnostic {}", entry.code));
        self.diagnostics.push(entry.clone());
        self.clients
            .broadcast(&EventBody::DiagnosticAdded { entry });
    }
}

/// The address range of the engine's keystroke buffer (architecture §4.1).
#[must_use]
pub fn engine_buffer_region(engine: &Engine) -> MemoryRegion {
    let (addr, len) = engine.buffer_region();
    MemoryRegion::from_raw(addr, len)
}

/// Locks the three places typed characters may live: the key queue, the engine's keystroke
/// buffer and the typed-characters hand-off (§8 rule 1). Returns true when all are locked. A
/// refusal is logged and the worker keeps running.
fn lock_typed_text(
    platform: &dyn Platform,
    keys: &keys::KeyQueue,
    engine: &Engine,
    handoff: &TypedHandoff,
    logger: &Logger,
) -> bool {
    let mut ok = true;
    for (what, region) in [
        ("key queue", keys.region()),
        ("keystroke buffer", engine_buffer_region(engine)),
        ("typed-characters hand-off", handoff.region()),
    ] {
        if let Err(e) = platform.lock(region) {
            ok = false;
            logger.warn(format_args!("cannot lock the {what} in memory: {e}"));
        }
    }
    ok
}

/// Builds the listener's sink over the key queue and the engine channel: never blocks; a full
/// queue is announced with `Overflow` (ADR-0010).
fn listener_sink(keys: &Arc<keys::KeyQueue>, tx: SyncSender<EngineMsg>) -> Box<dyn EventSink> {
    Box::new(keys::KeySink::new(Arc::clone(keys), tx))
}

/// The focus watcher's sink: every change goes to the focus thread.
fn focus_sink(tx: Sender<FocusMsg>) -> Box<dyn FocusSink> {
    Box::new(move |change: FocusChange| {
        let _ = tx.send(FocusMsg::Change(change));
    })
}

/// A running worker. See the [module docs](self).
pub struct Worker {
    shared: Arc<Shared>,
    endpoint: Arc<endpoint::Served>,
    handoff: Arc<TypedHandoff>,
    keys: Arc<keys::KeyQueue>,
    threads: Vec<(&'static str, JoinHandle<()>)>,
    watchdog_stop: Sender<()>,
    store_stop: Sender<store::StoreMsg>,
}

impl fmt::Debug for Worker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker").finish_non_exhaustive()
    }
}

fn spawn(
    name: &'static str,
    threads: &mut Vec<(&'static str, JoinHandle<()>)>,
    f: impl FnOnce() + Send + 'static,
) -> Result<(), StartError> {
    let handle = thread::Builder::new()
        .name(format!("iamb-{name}"))
        .spawn(f)
        .map_err(StartError::Thread)?;
    threads.push((name, handle));
    Ok(())
}

impl Worker {
    /// Opens the store (waiting for its lock), builds the snapshot, serves the endpoint and
    /// starts every thread. A listener or focus watcher that cannot start (a missing macOS
    /// permission) is logged; the worker still runs, so `GetPermissions` can say why. An
    /// endpoint that cannot be served is logged too, and tried again until it can
    /// ([`endpoint`]): expansion does not need it.
    ///
    /// # Errors
    ///
    /// [`StartError`].
    #[allow(
        clippy::too_many_lines,
        reason = "the start-up sequence, one step per thread"
    )]
    pub fn start(config: WorkerConfig) -> Result<Self, StartError> {
        let WorkerConfig {
            dirs,
            platform,
            endpoint,
            policy,
            heartbeat,
            heartbeat_interval,
            self_test_interval,
            store_lock_wait,
            endpoint_retry,
            clock,
            logger,
        } = config;
        logger.info(format_args!(
            "worker {} starting, pid {}",
            crate::VERSION,
            std::process::id()
        ));

        let mut store_db =
            Store::open_with_timeout(dirs.root(), store_lock_wait).map_err(StartError::Store)?;
        let (profile, first_view, migrated) =
            store::initial_view(&mut store_db).map_err(StartError::Store)?;
        if migrated.rewritten > 0 {
            logger.info(format_args!(
                "{} app exclusions rewritten in the form the focus inspector reports",
                migrated.rewritten
            ));
        }
        if migrated.unusable > 0 {
            logger.warn(format_args!(
                "{} app exclusions name no application of this system and never apply",
                migrated.unusable
            ));
        }
        // A bind that fails does not stop the worker (finding 12): the accept thread tries
        // again ([`endpoint`]).
        let first_bind = endpoint.bind(&dirs.service_endpoint(), policy);
        let served_at_start = first_bind.is_ok();

        let (engine_tx, engine_rx) = sync_channel::<EngineMsg>(DEFAULT_EVENT_CAPACITY);
        let (focus_tx, focus_rx) = channel::<FocusMsg>();
        let (store_tx, store_rx) = channel::<store::StoreMsg>();
        let (injector_tx, injector_rx) = channel::<InjectionRequest>();
        let (watchdog_stop, watchdog_rx) = channel::<()>();

        let snapshot = first_view.snapshot.clone();
        // The injector gets the user's per-app settings before anything can be injected.
        let injection = injection::InjectionSettings::start(
            Arc::clone(&platform) as Arc<dyn iamb_platform::Injector>,
            Arc::clone(&logger),
            &first_view.settings,
        );
        let shared = Arc::new(Shared {
            logger: Arc::clone(&logger),
            view: SharedView::new(first_view),
            platform: Arc::clone(&platform),
            engine_tx: engine_tx.clone(),
            focus_tx: focus_tx.clone(),
            store: store::StoreHandle::new(store_tx.clone()),
            clients: ipc::Clients::default(),
            diagnostics: Diagnostics::default(),
            test_field: Mutex::new(None),
            wakes: WakeCounters::default(),
            listener_ok: AtomicBool::new(false),
            watching: AtomicBool::new(false),
            memory_locked: AtomicBool::new(false),
            policy,
            exit: ExitSignal::default(),
            clock,
        });

        let handoff = Arc::new(TypedHandoff::new());
        let keys = Arc::new(keys::KeyQueue::new());
        let engine_core = Engine::new(snapshot);
        shared.memory_locked.store(
            lock_typed_text(platform.as_ref(), &keys, &engine_core, &handoff, &logger),
            Ordering::Relaxed,
        );

        let mut threads = Vec::new();
        {
            let shared = Arc::clone(&shared);
            spawn("store", &mut threads, move || {
                store::run(&shared, store_db, profile, injection, &store_rx);
            })?;
        }
        {
            let state = engine::EngineThread::new(
                Arc::clone(&shared),
                engine_core,
                Arc::clone(&handoff),
                Arc::clone(&keys),
                injector_tx,
                heartbeat,
                heartbeat_interval,
            );
            spawn("engine", &mut threads, move || state.run(&engine_rx))?;
        }
        {
            let shared = Arc::clone(&shared);
            let handoff = Arc::clone(&handoff);
            spawn("injector", &mut threads, move || {
                threads::injector(&shared, &handoff, &injector_rx);
            })?;
        }
        {
            let shared = Arc::clone(&shared);
            spawn("focus", &mut threads, move || {
                threads::focus(&shared, &focus_rx);
            })?;
        }

        // The listener and the focus watcher feed the threads above.
        match platform.start(listener_sink(&keys, engine_tx.clone())) {
            Ok(()) => shared.listener_ok.store(true, Ordering::Relaxed),
            Err(e) => logger.warn(format_args!("the key listener did not start: {e}")),
        }
        match platform.watch(focus_sink(focus_tx)) {
            Ok(()) => shared.watching.store(true, Ordering::Relaxed),
            Err(e) => logger.warn(format_args!("the focus watcher did not start: {e}")),
        }
        register_palette_hotkey(&shared);

        {
            let shared = Arc::clone(&shared);
            let tx = engine_tx;
            let keys = Arc::clone(&keys);
            spawn("watchdog", &mut threads, move || {
                threads::watchdog(&shared, &watchdog_rx, self_test_interval, &move || {
                    listener_sink(&keys, tx.clone())
                });
            })?;
        }
        let served = Arc::new(endpoint::Served::default());
        {
            let shared = Arc::clone(&shared);
            let served = Arc::clone(&served);
            let binder = endpoint::Binder {
                endpoint,
                name: dirs.service_endpoint(),
                policy,
                retry: endpoint_retry,
            };
            spawn("accept", &mut threads, move || {
                endpoint::run(&shared, &served, &binder, first_bind);
            })?;
        }
        logger.info(format_args!(
            "worker ready: {} snippets, listener {}, memory locked {}, endpoint {}",
            shared.view.get().snapshot.len(),
            shared.listener_ok.load(Ordering::Relaxed),
            shared.memory_locked.load(Ordering::Relaxed),
            served_at_start
        ));
        Ok(Self {
            shared,
            endpoint: served,
            handoff,
            keys,
            threads,
            watchdog_stop,
            store_stop: store_tx,
        })
    }

    /// Sends `msg` through the focus thread, so it reaches the engine behind every focus
    /// change reported so far, and waits for the answer.
    fn ask(&self, msg: impl FnOnce(Sender<()>) -> EngineMsg) {
        let (tx, rx) = channel();
        if self
            .shared
            .focus_tx
            .send(FocusMsg::Forward(msg(tx)))
            .is_ok()
        {
            let _ = rx.recv();
        }
    }

    /// Waits until the engine thread has handled everything sent to it so far, focus changes
    /// included (for tests and the compatibility harness).
    pub fn sync(&self) {
        self.ask(EngineMsg::Sync);
    }

    /// Waits until everything sent so far is handled and no injection is in flight.
    pub fn settle(&self) {
        self.ask(EngineMsg::Settle);
    }

    /// How often each thread woke since start.
    #[must_use]
    pub fn wake_counts(&self) -> WakeCounts {
        self.shared.wakes.snapshot()
    }

    /// The typed-characters hand-off (to check it is idle and locked).
    #[must_use]
    pub fn handoff(&self) -> &TypedHandoff {
        &self.handoff
    }

    /// True while the service endpoint is served: false while the worker runs without it and
    /// tries the bind again (the 2.8 review's finding 12).
    #[must_use]
    pub fn endpoint_served(&self) -> bool {
        self.endpoint.is_served()
    }

    /// The address range of the key queue's character slots (to check it is locked).
    #[must_use]
    pub fn key_queue_region(&self) -> MemoryRegion {
        self.keys.region()
    }

    /// Asks the worker to end with `reason` (what [`wait_exit`](Self::wait_exit) returns).
    pub fn request_exit(&self, reason: ExitReason) {
        self.shared.exit.request(reason);
    }

    /// A handle that asks the worker to end, for another thread.
    #[must_use]
    pub fn exit_handle(&self) -> ExitHandle {
        ExitHandle(Arc::clone(&self.shared))
    }

    /// Forgets remembered form values (screen lock, sign-out).
    pub fn forget_form_values(&self) {
        self.shared.to_engine(EngineMsg::ForgetFormValues);
    }

    /// Blocks until something asks the worker to end ([`request_exit`](Self::request_exit), a
    /// dead listener), or `timeout` passes (`None`).
    #[must_use]
    pub fn wait_exit(&self, timeout: Option<Duration>) -> Option<ExitReason> {
        self.shared.exit.wait(timeout)
    }

    /// Test fault: the engine thread stops (and with it the heartbeat). Debug builds only.
    #[cfg(debug_assertions)]
    pub fn hang_engine(&self) {
        self.shared.to_engine(EngineMsg::Hang);
    }

    /// Stops every thread, releases the endpoint and the store lock, and zeroes the hand-off and
    /// the key queue.
    pub fn stop(self) {
        let logger = Arc::clone(&self.shared.logger);
        let platform = &self.shared.platform;
        let _ = platform.stop();
        // The engine thread must not start the watcher again (it keeps a stopped one running).
        self.shared.watching.store(false, Ordering::Relaxed);
        let _ = platform.unwatch();
        let _ = platform.unregister(PALETTE_HOTKEY);
        self.endpoint.stop();
        self.shared.clients.shutdown_all();
        let _ = self.watchdog_stop.send(());
        let _ = self.shared.focus_tx.send(FocusMsg::Stop);
        let _ = self.shared.engine_tx.send(EngineMsg::Stop);
        let _ = self.store_stop.send(store::StoreMsg::Stop);
        for (name, handle) in self.threads {
            if handle.join().is_err() {
                logger.error(format_args!("the {name} thread panicked"));
            }
        }
        // In case the engine thread restarted the watcher just before it read the flag above.
        let _ = platform.unwatch();
        self.handoff.clear();
        self.keys.clear();
        logger.info(format_args!("worker stopped"));
    }
}

/// Asks a worker to end, from any thread ([`Worker::exit_handle`]).
#[derive(Clone)]
pub struct ExitHandle(Arc<Shared>);

impl ExitHandle {
    /// Asks the worker to end with `reason`.
    pub fn request(&self, reason: ExitReason) {
        self.0.exit.request(reason);
    }
}

impl fmt::Debug for ExitHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExitHandle").finish_non_exhaustive()
    }
}

/// Registers the palette hotkey from the `palette_hotkey` setting, if there is one.
fn register_palette_hotkey(shared: &Shared) {
    let view = shared.view.get();
    let Some(text) = view.settings.get("palette_hotkey") else {
        return;
    };
    match iamb_platform::Chord::parse(text) {
        Ok(chord) => {
            if let Err(e) = shared.platform.register(PALETTE_HOTKEY, chord) {
                shared
                    .logger
                    .warn(format_args!("cannot register the palette hotkey: {e}"));
            }
        }
        Err(_) => shared.logger.warn(format_args!(
            "the palette_hotkey setting is not a valid chord"
        )),
    }
}

/// Receives with a timeout, counting the wake-up.
pub(crate) fn recv_counted<T>(
    rx: &Receiver<T>,
    timeout: Duration,
    counter: &AtomicU64,
) -> Result<T, RecvTimeoutError> {
    let r = rx.recv_timeout(timeout);
    WakeCounters::bump(counter);
    r
}
