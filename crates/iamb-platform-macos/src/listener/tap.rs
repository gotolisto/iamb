//! The event tap and the thread it lives on.
//!
//! [`start`] creates the listener thread, which installs a listen-only event tap at the head
//! of the login session's event stream for key presses, key releases, modifier changes and
//! mouse button presses, and runs its own run loop. A listen-only tap cannot block, delay or
//! swallow an event (ADR-0010): the system does not wait for its callback.
//!
//! # The callback
//!
//! [`tap_callback`] reads a handful of integer fields from the event ([`raw_event`]) and hands
//! them to [`ListenerCore::handle`], which translates, takes a sequence number, delivers to
//! the sink (which never blocks) and returns. The callback returns the event it was given,
//! unchanged. A panic is caught at the boundary and counted.
//!
//! # The thread
//!
//! Besides the tap, the run loop serves one more source: [`Shared::send`] queues a
//! [`Request`] from any thread and signals it. That is how `stop`, `self_test`, a hotkey
//! press (delivered on the main thread, see `hotkeys.rs`) and a new keyboard layout reach the
//! listener thread, so that everything that goes to the sink is delivered by one thread, in
//! one order.
//!
//! Every [`WATCHDOG_INTERVAL`] the thread also looks at its tap, enables it if the system
//! disabled it without notice, and asks for a fresh keyboard layout if anything was typed.
//!
//! # Lifetimes
//!
//! The tap callback and the request source receive a pointer to the thread's
//! [`ThreadState`]. It is owned by the thread function, which removes both sources and
//! invalidates the tap's port before it drops the state.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use iamb_platform::{EventSink, HotkeyId, Marker, PlatformError, Result, SeqSource};
use objc2::MainThreadMarker;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopRunResult, CFRunLoopSource,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventMask, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::core::{CoreParts, ListenerCore, RawEvent, RawKind};
use super::inputs::InputSources;
use super::keycodes::vk;
use super::layout::Wake;
use super::main_thread::{self, RunLoopSignal, common_modes, default_mode};
use super::recovery::{
    DisableReason, RecoveryStats, TapCheck, TapHealth, TapSwitch, WATCHDOG_INTERVAL,
};
use super::selftest::SelfTests;
use super::stats::{ListenerStats, StatsSnapshot};
use super::sys;

/// How long `start` waits for the listener thread to install its tap.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `stop` waits for the listener thread to leave its run loop.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a failed self-test waits for the main thread to say whether secure keyboard
/// entry is on.
pub(crate) const SECURE_INPUT_QUESTION: Duration = Duration::from_millis(200);

/// The event kinds the tap asks for.
const EVENT_MASK: CGEventMask = (1 << CGEventType::KeyDown.0)
    | (1 << CGEventType::KeyUp.0)
    | (1 << CGEventType::FlagsChanged.0)
    | (1 << CGEventType::LeftMouseDown.0)
    | (1 << CGEventType::RightMouseDown.0)
    | (1 << CGEventType::OtherMouseDown.0);

/// The key of the self-test's no-op event: the release of F20, a key few keyboards have, from
/// a private event source. A key release types nothing and triggers no shortcut.
const SELF_TEST_KEY: u16 = vk::F20;

/// Something another thread asks of the listener thread.
pub(crate) enum Request {
    /// A registered hotkey was pressed.
    Hotkey(HotkeyId),
    /// The input sources published a new snapshot.
    InputsChanged,
    /// Look at the tap, enable it if needed, and say what was found.
    CheckTap(SyncSender<TapCheck>),
    /// Disable the tap, as the system would without telling anyone, and say when it is done.
    #[cfg(test)]
    DisableTap(SyncSender<()>),
}

/// What the listener thread shares with the rest of the process.
pub(crate) struct Shared {
    requests: Mutex<VecDeque<Request>>,
    signal: Mutex<Option<RunLoopSignal>>,
    stop: AtomicBool,
    exited: Mutex<bool>,
    exit: Condvar,
    /// The tap's health as the listener thread last published it.
    health: Mutex<(TapHealth, RecoveryStats)>,
    self_tests: Arc<SelfTests>,
    stats: Arc<ListenerStats>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // What these mutexes guard is replaced or popped whole; a panic cannot leave it torn.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn new() -> Self {
        Self {
            requests: Mutex::new(VecDeque::new()),
            signal: Mutex::new(None),
            stop: AtomicBool::new(false),
            exited: Mutex::new(false),
            exit: Condvar::new(),
            health: Mutex::new((TapHealth::Working, RecoveryStats::default())),
            self_tests: Arc::new(SelfTests::new()),
            stats: Arc::new(ListenerStats::default()),
        }
    }

    /// Queues `request` for the listener thread and wakes it. Never blocks; callable from any
    /// thread. A request for a thread that has gone is dropped.
    pub(crate) fn send(&self, request: Request) {
        lock(&self.requests).push_back(request);
        self.wake();
    }

    fn wake(&self) {
        if let Some(signal) = lock(&self.signal).as_ref() {
            signal.raise();
        }
    }

    fn take_request(&self) -> Option<Request> {
        lock(&self.requests).pop_front()
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(signal) = lock(&self.signal).as_ref() {
            signal.raise();
            signal.stop_run_loop();
        }
    }

    fn has_exited(&self) -> bool {
        *lock(&self.exited)
    }

    fn mark_exited(&self) {
        *lock(&self.signal) = None;
        lock(&self.requests).clear();
        *lock(&self.exited) = true;
        self.exit.notify_all();
    }

    fn wait_for_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut exited = lock(&self.exited);
        while !*exited {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            exited = self
                .exit
                .wait_timeout(exited, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Wake for Shared {
    fn inputs_changed(&self) {
        self.send(Request::InputsChanged);
    }
}

/// The real tap's switch.
struct PortSwitch {
    port: CFRetained<CFMachPort>,
}

impl TapSwitch for PortSwitch {
    fn enable(&mut self) {
        CGEvent::tap_enable(&self.port, true);
    }

    fn is_enabled(&self) -> bool {
        CGEvent::tap_is_enabled(&self.port)
    }
}

/// What the callbacks on the listener thread work with.
struct ThreadState {
    shared: Arc<Shared>,
    core: Option<ListenerCore>,
    /// The tap's port, for the test request that disables it.
    #[cfg(test)]
    port: Option<CFRetained<CFMachPort>>,
}

impl ThreadState {
    /// Serves the queued requests. Runs on the listener thread, from the request source.
    fn drain(&mut self) {
        while let Some(request) = self.shared.take_request() {
            let Some(core) = self.core.as_mut() else {
                continue;
            };
            match request {
                Request::Hotkey(id) => core.hotkey(id),
                Request::InputsChanged => core.inputs_changed(),
                Request::CheckTap(reply) => {
                    let _ = reply.try_send(core.check_tap(Instant::now()));
                }
                #[cfg(test)]
                Request::DisableTap(done) => {
                    if let Some(port) = &self.port {
                        CGEvent::tap_enable(port, false);
                    }
                    let _ = done.try_send(());
                }
            }
        }
        self.publish_health();
        if self.shared.stop.load(Ordering::Acquire)
            && let Some(run_loop) = CFRunLoop::current()
        {
            run_loop.stop();
        }
    }

    /// The watchdog's periodic look.
    fn tick(&mut self) {
        if let Some(core) = self.core.as_mut() {
            core.tick(Instant::now());
        }
        self.publish_health();
    }

    /// Copies the tap's health to where other threads can read it without asking this one.
    fn publish_health(&self) {
        if let Some(core) = self.core.as_ref() {
            *lock(&self.shared.health) = core.tap_health();
        }
    }
}

/// Runs `work` on the thread state unless it is already in use, and keeps a panic from
/// unwinding into the system frames below a callback.
fn guarded(cell: &RefCell<ThreadState>, work: impl FnOnce(&mut ThreadState)) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if let Ok(mut state) = cell.try_borrow_mut() {
            work(&mut state);
        }
    }));
    if outcome.is_err()
        && let Ok(state) = cell.try_borrow()
    {
        state.shared.stats.count_panic();
    }
}

/// Reads from an event what the listener needs. `None` for an event without content that is
/// not a notice.
pub(crate) fn raw_event(kind: CGEventType, event: Option<&CGEvent>) -> Option<RawEvent> {
    let kind = match kind {
        // The notices carry no event worth reading.
        CGEventType::TapDisabledByTimeout => {
            return Some(RawEvent::of(RawKind::TapDisabled(DisableReason::Timeout)));
        }
        CGEventType::TapDisabledByUserInput => {
            return Some(RawEvent::of(RawKind::TapDisabled(DisableReason::UserInput)));
        }
        CGEventType::KeyDown => RawKind::KeyDown,
        CGEventType::KeyUp => RawKind::KeyUp,
        CGEventType::FlagsChanged => RawKind::FlagsChanged,
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            RawKind::MouseDown
        }
        _ => RawKind::Other,
    };
    let event = event?;
    let field = |field| CGEvent::integer_value_field(Some(event), field);
    let mut raw = RawEvent {
        user_data: field(CGEventField::EventSourceUserData),
        ..RawEvent::of(kind)
    };
    if kind == RawKind::KeyDown {
        raw.key_code = u16::try_from(field(CGEventField::KeyboardEventKeycode)).unwrap_or(u16::MAX);
        raw.repeat = field(CGEventField::KeyboardEventAutorepeat) != 0;
        raw.keyboard_type =
            u32::try_from(field(CGEventField::KeyboardEventKeyboardType)).unwrap_or(0);
        raw.flags = CGEvent::flags(Some(event)).0;
    }
    Some(raw)
}

/// The tap callback as the system calls it: the event may be null for a notice.
pub(crate) type RawTapCallBack = unsafe extern "C-unwind" fn(
    CGEventTapProxy,
    CGEventType,
    *mut CGEvent,
    *mut c_void,
) -> *mut CGEvent;

/// The tap callback as the bindings declare it.
type BoundTapCallBack = unsafe extern "C-unwind" fn(
    CGEventTapProxy,
    CGEventType,
    NonNull<CGEvent>,
    *mut c_void,
) -> *mut CGEvent;

/// Creates an event tap on the login session's event stream.
///
/// # Safety
///
/// `callback` must follow the rules of an event-tap callback for `options` (a listen-only
/// tap returns the event it was given), and `info` must be valid for whatever `callback` does
/// with it until the returned port has been invalidated.
pub(crate) unsafe fn create_tap(
    placement: CGEventTapPlacement,
    options: CGEventTapOptions,
    events: CGEventMask,
    callback: RawTapCallBack,
    info: *mut c_void,
) -> Option<CFRetained<CFMachPort>> {
    // SAFETY: the two function types differ only in the event parameter, `*mut CGEvent`
    // against `NonNull<CGEvent>`, which have the same representation and calling convention.
    // The system may pass null there for a notice, which is why callbacks here take a plain
    // pointer.
    let callback = unsafe { std::mem::transmute::<RawTapCallBack, BoundTapCallBack>(callback) };
    // SAFETY: the caller vouches for `callback` and `info`.
    unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            placement,
            options,
            events,
            Some(callback),
            info,
        )
    }
}

/// The tap callback: observe and return the event unchanged.
unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: *mut CGEvent,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let entered = Instant::now();
    // SAFETY: `user_info` is the pointer `install` gave to `CGEventTapCreate`: the thread's
    // `RefCell<ThreadState>`, which outlives the tap (see `Installed::remove`). The callback
    // runs only on that thread, and the cell guards against re-entry.
    let cell = unsafe { &*user_info.cast::<RefCell<ThreadState>>() };
    // SAFETY: the system passes null or an event that is valid for the duration of the call.
    let event_ref = unsafe { event.as_ref() };
    guarded(cell, |state| {
        if let (Some(core), Some(raw)) = (state.core.as_mut(), raw_event(kind, event_ref)) {
            core.handle(&raw, entered);
        }
    });
    event
}

/// The request source's callback.
unsafe extern "C-unwind" fn requests_ready(info: *mut c_void) {
    // SAFETY: `info` is the pointer `install` gave to `RunLoopSignal::install`: the thread's
    // `RefCell<ThreadState>`, valid until the signal is retired in `Installed::remove`.
    let cell = unsafe { &*info.cast::<RefCell<ThreadState>>() };
    guarded(cell, ThreadState::drain);
}

/// The tap, its run-loop source and the state the callbacks use, on the listener thread.
struct Installed {
    state: Rc<RefCell<ThreadState>>,
    port: CFRetained<CFMachPort>,
    tap_source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}

/// What a listener is started with.
pub(crate) struct Parts {
    /// The listener's sequence numbers.
    pub(crate) seq: Arc<SeqSource>,
    /// Where events go.
    pub(crate) sink: Box<dyn EventSink>,
    /// The keyboard layout and input-method state.
    pub(crate) inputs: Arc<dyn InputSources>,
}

fn install(shared: &Arc<Shared>, parts: Parts) -> Result<Installed> {
    let state = Rc::new(RefCell::new(ThreadState {
        shared: Arc::clone(shared),
        core: None,
        #[cfg(test)]
        port: None,
    }));
    // The callbacks get the address of the cell. It does not move while the `Rc` lives, and
    // every access, here and in the callbacks, goes through the cell's shared reference.
    let info = Rc::as_ptr(&state).cast_mut().cast::<c_void>();

    // SAFETY: `tap_callback` observes the event and returns it unchanged, as a listen-only
    // tap must. `info` points to the thread state, which stays at its address and is dropped
    // only after `Installed::remove` invalidated the port, so no callback can see it dangling.
    let port = unsafe {
        create_tap(
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            EVENT_MASK,
            tap_callback,
            info,
        )
    }
    .ok_or(PlatformError::os(0, "CGEventTapCreate"))?;
    let tap_source = CFMachPort::new_run_loop_source(None, Some(&port), 0)
        .ok_or(PlatformError::os(0, "CFMachPortCreateRunLoopSource"))?;
    let run_loop = CFRunLoop::current().ok_or(PlatformError::os(0, "CFRunLoopGetCurrent"))?;

    {
        let mut state = state.borrow_mut();
        state.core = Some(ListenerCore::new(CoreParts {
            seq: parts.seq,
            sink: parts.sink,
            tap: Box::new(PortSwitch { port: port.clone() }),
            inputs: parts.inputs,
            self_tests: Arc::clone(&shared.self_tests),
            stats: Arc::clone(&shared.stats),
        }));
        #[cfg(test)]
        {
            state.port = Some(port.clone());
        }
    }

    // SAFETY: `requests_ready` reads `info` as the thread state, which stays valid until
    // `Installed::remove` has retired the signal.
    let signal = unsafe { RunLoopSignal::install(run_loop.clone(), requests_ready, info) }
        .ok_or(PlatformError::os(0, "CFRunLoopSourceCreate"))?;
    *lock(&shared.signal) = Some(signal);

    run_loop.add_source(Some(&tap_source), common_modes());
    CGEvent::tap_enable(&port, true);
    if let Some(core) = state.borrow_mut().core.as_mut() {
        core.started();
    }
    Ok(Installed {
        state,
        port,
        tap_source,
        run_loop,
    })
}

impl Installed {
    /// Runs the run loop until `stop` is asked for, looking at the tap between runs.
    fn run(&self, shared: &Shared) {
        while !shared.stop.load(Ordering::Acquire) {
            let result =
                CFRunLoop::run_in_mode(default_mode(), WATCHDOG_INTERVAL.as_secs_f64(), false);
            if result == CFRunLoopRunResult::Finished {
                // No source left on the run loop: the tap is gone. Leave; the worker's
                // self-test will notice and reinstall the listener.
                break;
            }
            guarded(&self.state, ThreadState::tick);
        }
    }

    /// Removes the tap and the request source. After this no callback runs, so the state can
    /// be dropped.
    fn remove(self, shared: &Shared) {
        CGEvent::tap_enable(&self.port, false);
        self.run_loop
            .remove_source(Some(&self.tap_source), common_modes());
        self.port.invalidate();
        if let Some(signal) = lock(&shared.signal).take() {
            signal.retire();
        }
        // Dropping the state drops the core and with it the sink.
        drop(self.state);
    }
}

/// Marks the listener thread as gone when the thread function ends, however it ends.
struct Exit<'a>(&'a Shared);

impl Drop for Exit<'_> {
    fn drop(&mut self) {
        self.0.mark_exited();
    }
}

fn thread_main(shared: &Arc<Shared>, parts: Parts, ready: &SyncSender<Result<()>>) {
    let _exit = Exit(shared);
    match install(shared, parts) {
        Ok(installed) => {
            let _ = ready.send(Ok(()));
            installed.run(shared);
            installed.remove(shared);
        }
        Err(error) => {
            if let Some(signal) = lock(&shared.signal).take() {
                signal.retire();
            }
            let _ = ready.send(Err(error));
        }
    }
}

/// A running listener: the thread and what it shares.
pub(crate) struct Running {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// One self-test at a time.
    self_test_gate: Mutex<()>,
}

/// Starts the listener thread and returns once its tap is installed.
///
/// The caller has checked the permissions: without them the system would refuse the tap, or
/// show its own prompt.
///
/// # Errors
///
/// [`Os`](PlatformError::Os) when the tap or the thread cannot be created.
pub(crate) fn start(parts: Parts) -> Result<Running> {
    let shared = Arc::new(Shared::new());
    let (ready, is_ready) = sync_channel(1);
    let thread_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("iamb-listener".to_owned())
        .spawn(move || thread_main(&thread_shared, parts, &ready))
        .map_err(|error| {
            PlatformError::os(
                error.raw_os_error().map_or(0, i64::from),
                "spawning the listener thread",
            )
        })?;
    match is_ready.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) => Ok(Running {
            shared,
            thread: Some(thread),
            self_test_gate: Mutex::new(()),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            // The thread is stuck before its run loop; tell it to leave when it gets there.
            shared.request_stop();
            Err(PlatformError::Timeout)
        }
    }
}

impl Running {
    /// What other threads use to reach the listener thread.
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Removes the tap and ends the thread; the sink is dropped.
    ///
    /// # Errors
    ///
    /// [`Timeout`](PlatformError::Timeout) when the thread did not leave its run loop in time.
    /// It is then left to end on its own.
    pub(crate) fn stop(mut self) -> Result<()> {
        self.shared.request_stop();
        if !self.shared.wait_for_exit(STOP_TIMEOUT) {
            return Err(PlatformError::Timeout);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        Ok(())
    }

    /// Posts a marked no-op event and waits until the tap saw it (architecture §3.2).
    ///
    /// While secure keyboard entry is on (a password field has the focus) the system keeps
    /// keyboard events from every event tap, so the tap cannot be tested and nothing is
    /// wrong with it. A test whose event was not seen therefore asks the main thread whether
    /// secure keyboard entry is on, and counts as passed when it is. Nothing expands in
    /// that state either, and the next self-test after it ends is a real one. The question
    /// can make a failing test return up to [`SECURE_INPUT_QUESTION`] after `timeout`.
    ///
    /// The first half of `timeout` is for the event to arrive. If it does not, the listener
    /// thread looks at its tap and enables it when the system had disabled it, and a second
    /// event is posted. If that one is not seen either, the tap is dead.
    ///
    /// # Errors
    ///
    /// [`Timeout`](PlatformError::Timeout) when the tap did not see the event, or the listener
    /// thread is gone; [`Os`](PlatformError::Os) when the event cannot be created.
    pub(crate) fn self_test(&self, timeout: Duration) -> Result<()> {
        let _one_at_a_time = lock(&self.self_test_gate);
        if self.shared.has_exited() {
            return Err(PlatformError::Timeout);
        }
        let deadline = Instant::now() + timeout;
        let nonce = self.shared.self_tests.begin();
        let result = self.self_test_until(nonce, deadline, timeout / 2);
        self.shared.self_tests.end();
        match result {
            Err(PlatformError::Timeout)
                if main_thread::call(SECURE_INPUT_QUESTION, secure_input_enabled) == Some(true) =>
            {
                Ok(())
            }
            other => other,
        }
    }

    fn self_test_until(&self, nonce: u32, deadline: Instant, first_wait: Duration) -> Result<()> {
        let tests = &self.shared.self_tests;
        post_self_test_event(nonce)?;
        if tests.wait_until(nonce, deadline.min(Instant::now() + first_wait)) {
            return Ok(());
        }
        // The tap may be disabled: have the listener thread look, which enables it. Whatever
        // it finds, the first event is lost or late, so one more is posted. (The watchdog may
        // have enabled the tap in the meantime; the look then finds nothing wrong, and the
        // second event is still the one that gets through.)
        let _ = self.check_tap(deadline);
        post_self_test_event(nonce)?;
        if tests.wait_until(nonce, deadline) {
            Ok(())
        } else {
            Err(PlatformError::Timeout)
        }
    }

    /// Asks the listener thread to look at its tap; `None` when it does not answer by
    /// `deadline`.
    pub(crate) fn check_tap(&self, deadline: Instant) -> Option<TapCheck> {
        let (reply, answer) = sync_channel(1);
        self.shared.send(Request::CheckTap(reply));
        answer
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    }

    /// The tap's health as the listener thread last published it: after each look of the
    /// watchdog and each request, so at most one watchdog interval old.
    pub(crate) fn tap_health(&self) -> (TapHealth, RecoveryStats) {
        *lock(&self.shared.health)
    }

    /// Whether the listener thread has ended.
    pub(crate) fn has_exited(&self) -> bool {
        self.shared.has_exited()
    }

    /// The listener's counters.
    pub(crate) fn stats(&self) -> StatsSnapshot {
        self.shared.stats.snapshot()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // A listener that is dropped without `stop` must not leave its tap behind.
        self.shared.request_stop();
    }
}

/// Whether secure keyboard entry is on: some application asked the system to deliver keys to
/// it alone, as password fields do. Event taps see no keyboard events until it is off again.
pub(crate) fn secure_input_enabled(_mtm: MainThreadMarker) -> bool {
    // SAFETY: the function takes no arguments and reads a flag of the login session; the
    // marker proves this is the main thread, which its header asks for.
    unsafe { sys::IsSecureEventInputEnabled() != 0 }
}

/// Posts the self-test's no-op event into the session's event stream, where the tap sees it.
fn post_self_test_event(nonce: u32) -> Result<()> {
    // A private source: the event neither reads nor changes the modifier state of the real
    // keyboard.
    let source = CGEventSource::new(CGEventSourceStateID::Private);
    let event = CGEvent::new_keyboard_event(source.as_deref(), SELF_TEST_KEY, false)
        .ok_or(PlatformError::os(0, "CGEventCreateKeyboardEvent"))?;
    CGEvent::set_integer_value_field(
        Some(&event),
        CGEventField::EventSourceUserData,
        Marker::SelfTest(nonce).encode_i64(),
    );
    CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
    Ok(())
}
