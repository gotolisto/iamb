//! The focus watcher: the workspace's activation notification on the main thread, and a
//! thread with a run loop that holds the Accessibility observer of the frontmost application
//! and calls the focus sink.
//!
//! What is reported, and when the thread looks again, is decided by
//! [`Tracker`] and [`Attachment`] (`track`); this file only gathers
//! what the system says and does what they decide.
//!
//! # The main thread
//!
//! `NSWorkspaceDidActivateApplicationNotification` is posted on the workspace's own
//! notification centre, on the main thread, when the main run loop turns.
//! [`observe_activations`] registers one observer object for the process, once and for good;
//! its only method sets a flag on every living watcher and wakes it. Nothing slow happens on
//! the main thread.
//!
//! # The watcher thread
//!
//! One turn of the thread ([`Watcher::turn`]):
//!
//! 1. **Look** (fast, no application is asked anything): the frontmost application, its
//!    front window, the secure-entry flag.
//! 2. **Report** what the tracker says is new. An activation is reported before anything is
//!    asked of the application, so the engine is suspended at once.
//! 3. **Upkeep** (slow, messages to the application, each with a timeout): move the
//!    Accessibility observer to the frontmost application; read which element has the focus
//!    when the tracker does not know.
//!
//! Then the thread runs its run loop until the next notification or the tracker's next
//! timer. An idle watcher has no timer and sleeps.
//!
//! # The Accessibility observer
//!
//! `AXObserver` delivers through a run-loop source, here on the watcher thread's run loop.
//! Its callback only notes what it was told in the thread's [`Inbox`] (that a window changed;
//! which element now has the focus) and returns; the turn that follows deals with it. The
//! element is kept as a reference to compare with the next one. Its attributes are never
//! read here.
//!
//! # Lifetimes
//!
//! The observer callback and the wake source receive a pointer: the callback to the thread's
//! inbox, which the thread owns and drops after the observer; the wake source null.

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use iamb_platform::{FocusChange, FocusSink, PlatformError, Result};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{ClassType, MainThreadMarker, define_class, msg_send, sel};
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidActivateApplicationNotification};
use objc2_application_services::{AXError, AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopRunResult, CFRunLoopSource, CFString, Type,
};
use objc2_foundation::NSNotification;

use super::WATCH_MESSAGE_TIMEOUT;
use super::field::{self, Registration, notification};
use super::system;
use super::track::{Attachment, Seen, Signal, Step, Tracker};
use crate::listener::main_thread::{RunLoopSignal, common_modes, default_mode};

/// How long `start` waits for the watcher thread's first report.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `stop` waits for the watcher thread to leave its run loop.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a look waits for the main thread to say whether secure keyboard entry is on.
/// Short, because the report waits for it; an idle main thread answers in well under a
/// millisecond.
const SECURE_INPUT_LIMIT: Duration = Duration::from_millis(20);
/// How long the run loop runs when the tracker asks for no timer.
const IDLE: Duration = Duration::from_hours(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Everything behind these mutexes is replaced whole.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the watcher thread shares with the main thread and with whoever stops it.
struct Shared {
    /// Set on the main thread when an application was activated.
    activated: AtomicBool,
    stop: AtomicBool,
    /// Wakes the thread's run loop; present while the run loop exists.
    signal: Mutex<Option<RunLoopSignal>>,
    exited: Mutex<bool>,
    exit: Condvar,
}

impl Shared {
    fn wake(&self) {
        if let Some(signal) = lock(&self.signal).as_ref() {
            signal.raise();
        }
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(signal) = lock(&self.signal).as_ref() {
            signal.raise();
            signal.stop_run_loop();
        }
    }

    fn mark_exited(&self) {
        if let Some(signal) = lock(&self.signal).take() {
            signal.retire();
        }
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

/// Every watcher of this process. One list, because the activation observer is one.
static WATCHERS: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
static OBSERVING: AtomicBool = AtomicBool::new(false);

/// Tells every living watcher that an application was activated. Runs on the main thread.
fn notify_watchers() {
    let mut watchers = lock(&WATCHERS);
    watchers.retain(|watcher| watcher.strong_count() > 0);
    for watcher in watchers.iter().filter_map(Weak::upgrade) {
        watcher.activated.store(true, Ordering::Release);
        watcher.wake();
    }
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and this class has no instance
    // variables and does not implement `Drop`.
    #[unsafe(super(NSObject))]
    struct ActivationObserver;

    impl ActivationObserver {
        /// The target of the workspace's notification. The notification names the
        /// application; the watcher thread asks the workspace itself, so it is not read.
        #[unsafe(method(applicationActivated:))]
        fn application_activated(&self, _notification: &NSNotification) {
            // A panic must not unwind into the notification centre.
            let _ = catch_unwind(notify_watchers);
        }
    }
);

/// Asks the workspace, once per process, to say when an application is activated. Main
/// thread only, which the marker proves: the notification is delivered on the thread whose
/// run loop the workspace uses, the main one.
pub(crate) fn observe_activations(_mtm: MainThreadMarker) {
    if OBSERVING.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: `new` on a class without instance variables returns a new, initialised object
    // of that class.
    let observer: Retained<ActivationObserver> =
        unsafe { msg_send![ActivationObserver::class(), new] };
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the selector names the method defined above, which takes the one argument the
    // centre passes, a notification. The name is a constant the framework initialises when
    // it is loaded. The centre does not retain its observers; this one is never freed (it
    // is leaked below), so the centre's reference stays valid for the life of the process.
    unsafe {
        center.addObserver_selector_name_object(
            &observer,
            sel!(applicationActivated:),
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
        );
    }
    let _ = Retained::into_raw(observer);
}

/// An Accessibility element as the tracker compares it: two are equal when they are the
/// same element (`CFEqual`). Nothing is ever read from it.
struct Element(CFRetained<AXUIElement>);

impl PartialEq for Element {
    fn eq(&self, other: &Self) -> bool {
        *self.0 == *other.0
    }
}

/// What the observer callback noted since the last turn.
#[derive(Default)]
struct Inbox {
    /// The focused or the main window of the application changed.
    window: bool,
    /// The element that now has the keyboard focus (the last one, when there were several).
    element: Option<CFRetained<AXUIElement>>,
}

/// The `AXObserverCallback`: notes the notification and returns. Runs on the watcher thread,
/// from its run loop.
unsafe extern "C-unwind" fn focus_notified(
    _observer: NonNull<AXObserver>,
    element: NonNull<AXUIElement>,
    name: NonNull<CFString>,
    refcon: *mut c_void,
) {
    // A panic must not unwind into the run loop.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `refcon` is the address of the watcher thread's inbox, given at
        // registration; the thread drops every observer before the inbox, and this callback
        // runs on that thread only.
        let Some(inbox) = (unsafe { refcon.cast::<RefCell<Inbox>>().as_ref() }) else {
            return;
        };
        // SAFETY: the system passes two valid objects that live at least as long as the
        // callback; the element is retained before it is kept.
        let (element, name) = unsafe { (element.as_ref(), name.as_ref()) };
        let Ok(mut inbox) = inbox.try_borrow_mut() else {
            return;
        };
        if name.to_string() == notification::FOCUSED_ELEMENT_CHANGED {
            inbox.element = Some(element.retain());
        } else {
            inbox.window = true;
        }
    }));
}

/// The wake source's callback. That the source fired is what ends the run loop's wait.
unsafe extern "C-unwind" fn woken(_info: *mut c_void) {}

/// The Accessibility observer of one application, on the watcher thread's run loop.
struct Observed {
    app: CFRetained<AXUIElement>,
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
    /// Kept alive for as long as its source is on the run loop.
    _observer: CFRetained<AXObserver>,
}

impl Observed {
    /// Registers for the focus notifications of application `pid`. `None` when the
    /// application did not take the registration (it is starting, hung, or gone); an
    /// application that does not know one of the notifications is still observed for the
    /// others.
    fn attach(
        pid: u32,
        run_loop: &CFRetained<CFRunLoop>,
        inbox: &Rc<RefCell<Inbox>>,
    ) -> Option<Self> {
        let app = system::application(pid)?;
        system::set_timeout(&app, WATCH_MESSAGE_TIMEOUT);
        let process = libc::pid_t::try_from(pid).ok()?;
        let mut observer: *mut AXObserver = ptr::null_mut();
        // SAFETY: `focus_notified` has the callback's signature, and `observer` is a valid
        // place for the result.
        let error = unsafe {
            AXObserver::create(process, Some(focus_notified), NonNull::from(&mut observer))
        };
        if error != AXError::Success {
            return None;
        }
        // SAFETY: on success the function stored an observer the caller owns (the "Create"
        // rule).
        let observer = unsafe { CFRetained::from_raw(NonNull::new(observer)?) };
        let refcon = Rc::as_ptr(inbox).cast_mut().cast::<c_void>();
        for name in notification::ALL {
            let name = CFString::from_static_str(name);
            // SAFETY: the observer, the element and the name are valid for the call.
            // `refcon` is the address of the thread's inbox, which outlives the observer
            // (see `Watcher`'s field order); the callback reads it through a shared
            // reference.
            let error = unsafe { observer.add_notification(&app, &name, refcon) };
            if field::registration(error.0) == Registration::Failed {
                return None;
            }
        }
        // SAFETY: the observer is valid; its source lives as long as the observer does.
        let source = unsafe { observer.run_loop_source() };
        run_loop.add_source(Some(&source), common_modes());
        Some(Self {
            app,
            source,
            run_loop: run_loop.clone(),
            _observer: observer,
        })
    }

    /// The element of the application that has the keyboard focus now: one message to the
    /// application, with the watcher's timeout.
    fn focused_element(&self) -> Option<Element> {
        system::focused_element(&self.app).ok().map(Element)
    }
}

impl Drop for Observed {
    fn drop(&mut self) {
        // After this the callback is not called again; releasing the observer ends the
        // registrations without another message to the application.
        self.run_loop
            .remove_source(Some(&self.source), common_modes());
    }
}

/// What the watcher thread works with. The observer is declared, and so dropped, before the
/// inbox its callback points to.
struct Watcher {
    shared: Arc<Shared>,
    sink: Box<dyn FocusSink>,
    tracker: Tracker<Element>,
    attachment: Attachment,
    /// The process id of the application seen frontmost by the last look.
    front: Option<u32>,
    observed: Option<Observed>,
    inbox: Rc<RefCell<Inbox>>,
    run_loop: CFRetained<CFRunLoop>,
}

impl Watcher {
    /// Looks at the system and reports to the sink what the tracker says is new. Asks no
    /// application anything. `true` when something was reported.
    fn look(&mut self, signal: Signal<Element>) -> bool {
        let front = system::front();
        self.front = front.as_ref().map(|(app, _)| app.pid);
        let seen = Seen {
            change: front.map(|(app, token)| FocusChange { app, token }),
            // A main thread that does not answer counts as "off": the inspection, which
            // asks again, is what decides, and it fails closed.
            secure_input: system::secure_input(SECURE_INPUT_LIMIT).unwrap_or(false),
        };
        let Some(change) = self.tracker.observe(signal, Instant::now(), seen) else {
            return false;
        };
        if self.shared.stop.load(Ordering::Acquire) {
            // `unwatch` was called while this look was under way: the sink hears no more.
            return false;
        }
        self.sink.focus_changed(change);
        true
    }

    /// Keeps the Accessibility observer on the frontmost application and learns which
    /// element has the focus when the tracker does not know. Sends messages to the
    /// application, each with [`WATCH_MESSAGE_TIMEOUT`].
    fn upkeep(&mut self) {
        match self.attachment.step(self.front, Instant::now()) {
            Step::Keep => {}
            Step::Detach => self.drop_observer(),
            Step::Attach { pid } => {
                self.drop_observer();
                self.observed = Observed::attach(pid, &self.run_loop, &self.inbox);
                let late = self
                    .attachment
                    .attached(self.observed.is_some(), Instant::now());
                if late {
                    // The application was not observed since it was activated.
                    self.look(Signal::Attached);
                }
            }
        }
        if self.tracker.wants_element()
            && let Some(observed) = &self.observed
        {
            let element = observed.focused_element();
            self.tracker.learn_element(element);
        }
    }

    /// Drops the observer and what it noted: notes about an application that is no longer
    /// observed mean nothing.
    fn drop_observer(&mut self) {
        self.observed = None;
        self.inbox.take();
    }

    /// One turn: deals with what happened since the last one.
    fn turn(&mut self) {
        let activated = self.shared.activated.swap(false, Ordering::AcqRel);
        let Inbox { window, element } = self.inbox.take();
        // One look per turn for what the application said: a window and the element in it
        // are one change, and the element signal reports the window with it.
        let signal = match (activated, window, element) {
            // The notes are about the application that had the focus before.
            (true, _, _) => Some(Signal::Activated),
            (false, _, Some(element)) => Some(Signal::Element(Element(element))),
            (false, true, None) => Some(Signal::Window),
            (false, false, None) => None,
        };
        let mut looked = false;
        if let Some(signal) = signal {
            self.look(signal);
            looked = true;
        }
        let now = Instant::now();
        if self.tracker.due(now) || self.attachment.due(now) {
            self.look(Signal::Timer);
            looked = true;
        }
        if looked {
            self.upkeep();
        }
    }

    /// Runs the run loop until `stop` is asked for, taking a turn whenever it returns.
    fn run(&mut self) {
        while !self.shared.stop.load(Ordering::Acquire) {
            let now = Instant::now();
            let wait = [self.tracker.next_timer(), self.attachment.next_timer()]
                .into_iter()
                .flatten()
                .min()
                .map_or(IDLE, |at| at.saturating_duration_since(now));
            if !wait.is_zero() {
                let result = CFRunLoop::run_in_mode(default_mode(), wait.as_secs_f64(), true);
                if result == CFRunLoopRunResult::Finished {
                    // No source is left on the run loop, not even the wake source: nothing
                    // could ever wake this thread again.
                    break;
                }
            }
            if self.shared.stop.load(Ordering::Acquire) {
                break;
            }
            // A panic in the sink must not take the thread down without a trace of order;
            // the watcher goes on with the next turn.
            let _ = catch_unwind(AssertUnwindSafe(|| self.turn()));
        }
    }
}

/// Marks the watcher thread as gone when the thread function ends, however it ends.
struct Exit<'a>(&'a Shared);

impl Drop for Exit<'_> {
    fn drop(&mut self) {
        self.0.mark_exited();
    }
}

fn thread_main(shared: &Arc<Shared>, sink: Box<dyn FocusSink>, ready: &SyncSender<Result<()>>) {
    let _exit = Exit(shared);
    let Some(run_loop) = CFRunLoop::current() else {
        let _ = ready.send(Err(PlatformError::os(0, "CFRunLoopGetCurrent")));
        return;
    };
    // SAFETY: `woken` ignores `info`, which is null.
    let signal = unsafe { RunLoopSignal::install(run_loop.clone(), woken, ptr::null_mut()) };
    let Some(signal) = signal else {
        let _ = ready.send(Err(PlatformError::os(0, "CFRunLoopSourceCreate")));
        return;
    };
    *lock(&shared.signal) = Some(signal);

    let mut watcher = Watcher {
        shared: Arc::clone(shared),
        sink,
        tracker: Tracker::new(),
        attachment: Attachment::default(),
        front: None,
        observed: None,
        inbox: Rc::new(RefCell::new(Inbox::default())),
        run_loop,
    };
    // The current focus is reported once, at once: before `watch` returns, and before the
    // application is asked anything.
    let first = catch_unwind(AssertUnwindSafe(|| {
        shared.activated.store(false, Ordering::Release);
        watcher.look(Signal::Activated);
    }));
    if first.is_err() {
        let _ = ready.send(Err(PlatformError::os(0, "the first focus report")));
        return;
    }
    let _ = ready.send(Ok(()));
    let _ = catch_unwind(AssertUnwindSafe(|| watcher.upkeep()));
    watcher.run();
    // Dropping the watcher removes the observer from the run loop and drops the sink.
}

/// A running watcher: the thread and what it shares.
pub(crate) struct Watching {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// Starts the watcher thread and returns once it has reported the current focus to `sink`.
///
/// The caller has checked the Accessibility permission and has called
/// [`observe_activations`] on the main thread.
///
/// # Errors
///
/// [`Os`](PlatformError::Os) when the thread or its run loop cannot be created;
/// [`Timeout`](PlatformError::Timeout) when the thread did not get to its first report.
pub(crate) fn start(sink: Box<dyn FocusSink>) -> Result<Watching> {
    let shared = Arc::new(Shared {
        activated: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        signal: Mutex::new(None),
        exited: Mutex::new(false),
        exit: Condvar::new(),
    });
    {
        let mut watchers = lock(&WATCHERS);
        watchers.retain(|watcher| watcher.strong_count() > 0);
        watchers.push(Arc::downgrade(&shared));
    }
    let (ready, is_ready) = sync_channel(1);
    let thread_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("iamb-focus-watch".to_owned())
        .spawn(move || thread_main(&thread_shared, sink, &ready))
        .map_err(|error| {
            PlatformError::os(
                error.raw_os_error().map_or(0, i64::from),
                "spawning the focus watcher thread",
            )
        })?;
    match is_ready.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) => Ok(Watching {
            shared,
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            // The thread is stuck before its run loop, or ended without a word; tell it to
            // leave when it gets there.
            shared.request_stop();
            Err(PlatformError::Timeout)
        }
    }
}

impl Watching {
    /// Ends the thread; the sink is dropped.
    ///
    /// # Errors
    ///
    /// [`Timeout`](PlatformError::Timeout) when the thread did not leave its run loop in
    /// time (it is waiting for an application). It is then left to end on its own, and the
    /// sink is called no more.
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
}

impl Drop for Watching {
    fn drop(&mut self) {
        // A platform dropped while it watches: the thread must not outlive it for long.
        self.shared.request_stop();
    }
}
