//! `SessionEvents`: screen lock, session switch, sign-out and sleep (architecture §4.5; the
//! 2.8 privacy review's finding 13).
//!
//! One observer object for the process, registered on the main thread once and for good (as
//! the focus watcher's activation observer), hears:
//!
//! | Notification | Centre | Reported as |
//! | --- | --- | --- |
//! | `com.apple.screenIsLocked` | the distributed centre | `Locked` |
//! | `NSWorkspaceScreensDidSleepNotification` | the workspace's centre | `Locked` (a sleeping display usually locks; a report too many only forgets a form's remembered answers) |
//! | `NSWorkspaceSessionDidResignActiveNotification` | the workspace's centre | `SwitchedOut` |
//! | `NSWorkspaceWillPowerOffNotification` | the workspace's centre | `SigningOut` (also shutdown and restart) |
//! | `NSWorkspaceWillSleepNotification` | the workspace's centre | `Sleeping` |
//!
//! The four workspace notifications are documented by Apple. `com.apple.screenIsLocked` is
//! not: it is the name the system has posted on the distributed centre when the screen locks
//! since early releases of macOS, and is used here in addition, never alone.
//!
//! Both centres deliver on the main thread's run loop, which the worker runs
//! (`run_with_main_loop`); the distributed one at once, without waiting for the application to
//! become active, which the worker's never does. Each method of the observer only hands the
//! event to the sink, which must not block (the service's sends on a channel); the sink lives
//! in a process-wide slot, because the observer is process-wide.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use iamb_platform::{PlatformError, Result, SessionEvent, SessionEvents, SessionSink};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{ClassType, MainThreadMarker, define_class, msg_send, sel};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceScreensDidSleepNotification,
    NSWorkspaceSessionDidResignActiveNotification, NSWorkspaceWillPowerOffNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSString,
};

use crate::Platform;
use crate::listener::main_thread;

/// The distributed notification posted when the screen locks (see the module docs).
const SCREEN_LOCKED: &str = "com.apple.screenIsLocked";

/// How long `watch_session` waits for the main thread to register the observer.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(2);

/// The sink, while something watches.
static SINK: Mutex<Option<Box<dyn SessionSink>>> = Mutex::new(None);

/// The observer is registered (once per process).
static OBSERVING: AtomicBool = AtomicBool::new(false);

fn sink() -> MutexGuard<'static, Option<Box<dyn SessionSink>>> {
    // The slot is replaced whole.
    SINK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hands `event` to the sink, if there is one. Runs on the main thread; never unwinds into
/// the notification centre.
fn deliver(event: SessionEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(sink) = sink().as_mut() {
            sink.session_event(event);
        }
    }));
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and this class has no instance
    // variables and does not implement `Drop`.
    #[unsafe(super(NSObject))]
    struct SessionObserver;

    impl SessionObserver {
        /// `com.apple.screenIsLocked`.
        #[unsafe(method(screenLocked:))]
        fn screen_locked(&self, _notification: &NSNotification) {
            deliver(SessionEvent::Locked);
        }

        /// `NSWorkspaceScreensDidSleepNotification`.
        #[unsafe(method(screensSlept:))]
        fn screens_slept(&self, _notification: &NSNotification) {
            deliver(SessionEvent::Locked);
        }

        /// `NSWorkspaceSessionDidResignActiveNotification`.
        #[unsafe(method(sessionResigned:))]
        fn session_resigned(&self, _notification: &NSNotification) {
            deliver(SessionEvent::SwitchedOut);
        }

        /// `NSWorkspaceWillPowerOffNotification`.
        #[unsafe(method(willPowerOff:))]
        fn will_power_off(&self, _notification: &NSNotification) {
            deliver(SessionEvent::SigningOut);
        }

        /// `NSWorkspaceWillSleepNotification`.
        #[unsafe(method(willSleep:))]
        fn will_sleep(&self, _notification: &NSNotification) {
            deliver(SessionEvent::Sleeping);
        }
    }
);

/// Registers the observer with both centres, once per process. Main thread only, which the
/// marker proves: both centres deliver on the main run loop.
fn observe(_mtm: MainThreadMarker) {
    if OBSERVING.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: `new` on a class without instance variables returns a new, initialised object
    // of that class.
    let observer: Retained<SessionObserver> = unsafe { msg_send![SessionObserver::class(), new] };
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: each selector names a method defined above, which takes the one argument the
    // centre passes, a notification. The names are constants the framework initialises when
    // it is loaded. The centre does not retain its observers; this one is never freed (it is
    // leaked below), so the centre's reference stays valid for the life of the process.
    unsafe {
        workspace.addObserver_selector_name_object(
            &observer,
            sel!(screensSlept:),
            Some(NSWorkspaceScreensDidSleepNotification),
            None,
        );
        workspace.addObserver_selector_name_object(
            &observer,
            sel!(sessionResigned:),
            Some(NSWorkspaceSessionDidResignActiveNotification),
            None,
        );
        workspace.addObserver_selector_name_object(
            &observer,
            sel!(willPowerOff:),
            Some(NSWorkspaceWillPowerOffNotification),
            None,
        );
        workspace.addObserver_selector_name_object(
            &observer,
            sel!(willSleep:),
            Some(NSWorkspaceWillSleepNotification),
            None,
        );
    }
    let name = NSString::from_str(SCREEN_LOCKED);
    // SAFETY: as above; the selector names `screenLocked:`. The distributed centre keeps a
    // weak reference too, and the observer is never freed. Delivered at once: the centre
    // holds back notifications for an application that is not active, and the worker's
    // application never is.
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .addObserver_selector_name_object_suspensionBehavior(
                &observer,
                sel!(screenLocked:),
                Some(&name),
                None,
                NSNotificationSuspensionBehavior::DeliverImmediately,
            );
    }
    let _ = Retained::into_raw(observer);
}

impl SessionEvents for Platform {
    fn watch_session(&self, new: Box<dyn SessionSink>) -> Result<()> {
        if sink().is_some() {
            return Err(PlatformError::AlreadyRunning);
        }
        // The wait for the main thread happens before the slot is taken.
        if main_thread::call(MAIN_THREAD_TIMEOUT, observe).is_none() {
            return Err(PlatformError::os(
                0,
                "the main run loop does not run; session events cannot be heard",
            ));
        }
        let mut slot = sink();
        if slot.is_some() {
            return Err(PlatformError::AlreadyRunning);
        }
        *slot = Some(new);
        Ok(())
    }

    fn unwatch_session(&self) -> Result<()> {
        *sink() = None;
        Ok(())
    }
}
