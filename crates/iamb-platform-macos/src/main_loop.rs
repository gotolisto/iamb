//! The worker's main thread: the application event loop (architecture §3.1).
//!
//! # Why the application event loop, and not a bare run loop
//!
//! The listener hands its main-thread work (the keyboard layout through Text Input Sources,
//! the Carbon hotkey calls) to the main thread through a run-loop source
//! (`listener::main_thread`), and the system delivers the input-source notification on the
//! main run loop. A bare `CFRunLoopRun` would serve both. It would not reliably deliver
//! hotkey presses: those are Carbon events, and Carbon events reach their handlers when the
//! application takes them from its event queue, which is what `NSApplication`'s `run` does
//! (Carbon's own `RunApplicationEventLoop` is for 32-bit programs only; see `hotkeys.rs`).
//! `run` runs the main run loop in the default mode, which is one of the common modes the
//! listener's source is added to, so the listener's jobs are served too. (The system's
//! `NSRunningApplication` docs add one more reason: what it reports about an application,
//! such as being frontmost, is refreshed only as the main run loop turns.)
//!
//! # The application
//!
//! The worker is a background process: it has no window, no Dock icon and no menu bar. Its
//! activation policy is set to `Accessory`, the policy of agent applications that take global
//! hotkeys and show nothing (an `LSUIElement` bundle gets it from its `Info.plist`; setting it
//! here makes a bare development binary behave the same). `Prohibited` (background-only) would
//! hide the process just as well, but whether the system delivers hotkey presses to a
//! background-only process has not been tried here, and a hotkey that silently never arrives
//! is the worse failure.
//!
//! # Stopping
//!
//! `NSApplication`'s `stop:` ends `run` only after `run` has handled one more event, and the
//! call that asks for the stop runs from a run-loop source, not from an event. So the stop
//! posts an empty application-defined event at the head of the queue, which `run` takes at
//! once and then returns. If `run` returns while `work` is still running (something else
//! stopped it), it is run again.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
};
use objc2_core_foundation::CGPoint;

use crate::listener::main_thread;

/// The exit code for a `work` that panicked: the code of a Rust program whose `main`
/// panicked.
const PANICKED: i32 = 101;

/// Runs `work` beside the application event loop and returns `work`'s exit code.
///
/// **Call it from the process's main thread, once, at the start of a worker that uses
/// [`Platform`](crate::Platform).** The main thread becomes the application's (an
/// `NSApplication` with no Dock icon and no menu bar) and runs its event loop, which serves
/// what the listener and the hotkeys must do on the main thread; `work` runs on a thread of
/// its own, named `iamb-work`. When `work` returns or panics the event loop is stopped and
/// this function returns: `work`'s result, or 101 when it panicked (the panic hook has run by
/// then).
///
/// Called from any other thread there is no main thread to give: `work` then runs on the
/// calling thread, and the listener's `start` fails as it does without the loop. When no
/// thread can be created for `work`, it runs on the main thread instead, without the loop.
///
/// This is the crate's one public item outside the platform traits (see the crate docs).
#[must_use = "the result is `work`'s exit code"]
pub fn run_with_main_loop<F>(work: F) -> i32
where
    F: FnOnce() -> i32 + Send + 'static,
{
    let Some(mtm) = MainThreadMarker::new() else {
        return run_caught(work);
    };
    let app = NSApplication::sharedApplication(mtm);
    // A refusal leaves the policy the process already has; the loop runs either way.
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let finished = Arc::new(AtomicBool::new(false));
    // Kept here as well, so that the work can still run when its thread cannot be created.
    let slot = Arc::new(Mutex::new(Some(work)));
    let spawned = {
        let finished = Arc::clone(&finished);
        let slot = Arc::clone(&slot);
        thread::Builder::new()
            .name("iamb-work".to_owned())
            .spawn(move || {
                let code = take(&slot).map_or(PANICKED, run_caught);
                finished.store(true, Ordering::Release);
                main_thread::post(stop);
                code
            })
    };
    let Ok(worker) = spawned else {
        return take(&slot).map_or(PANICKED, run_caught);
    };
    while !finished.load(Ordering::Acquire) {
        app.run();
    }
    worker.join().unwrap_or(PANICKED)
}

fn take<F>(slot: &Mutex<Option<F>>) -> Option<F> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

fn run_caught(work: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or(PANICKED)
}

/// Ends the current `run` of the application event loop. Runs on the main thread.
fn stop(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    // `stop:` takes effect after the next event; this one is it.
    let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        CGPoint::ZERO,
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(wake) = wake {
        app.postEvent_atStart(&wake, true);
    }
}
