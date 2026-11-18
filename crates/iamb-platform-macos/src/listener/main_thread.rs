//! Running a piece of work on the main thread.
//!
//! Two things this crate needs are main-thread only on macOS: the Text Input Sources
//! functions (the current keyboard layout) and the Carbon event functions (hotkeys, the
//! secure-input flag). The listener has
//! its own thread, so it hands such work to the main thread through a run-loop source on the
//! main run loop. Each job receives a [`MainThreadMarker`], the proof the main-thread-only
//! wrappers ask for; there is no other way to obtain one off the main thread.
//!
//! # What the process must do
//!
//! Jobs run when the main thread runs its run loop in a common mode. **The worker's main
//! thread must therefore run a run loop** (`CFRunLoopRun`, or the application event loop,
//! which the hotkeys need anyway: see `hotkeys.rs`). When it does not, jobs stay queued:
//! [`call`] gives up after its timeout, and work posted with [`post`] simply happens later or
//! never. A caller that is itself on the main thread is served on the spot, so start-up code
//! on the main thread works before the run loop is running.
//!
//! Jobs run in the order they were posted.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, CFRunLoopSourceContext,
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};

use super::reply::Reply;

type Job = Box<dyn FnOnce(MainThreadMarker) + Send>;

static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
static SIGNAL: OnceLock<Option<RunLoopSignal>> = OnceLock::new();

/// The set of run-loop modes a source is normally added to: the default mode and the modes
/// the system runs while a menu or a modal panel is open.
pub(crate) fn common_modes() -> Option<&'static CFRunLoopMode> {
    // SAFETY: a constant the framework initialises before any code of this process runs, and
    // never changes.
    unsafe { kCFRunLoopCommonModes }
}

/// The run-loop mode a thread's own loop is normally run in.
pub(crate) fn default_mode() -> Option<&'static CFRunLoopMode> {
    // SAFETY: as for `common_modes`.
    unsafe { kCFRunLoopDefaultMode }
}

/// A way to make a run loop call a function soon: a run-loop source on that loop, which any
/// thread may signal. Signals are not lost: a source signalled while its run loop is busy, or
/// not running yet, fires when the loop next runs.
pub(crate) struct RunLoopSignal {
    run_loop: CFRetained<CFRunLoop>,
    source: CFRetained<CFRunLoopSource>,
}

// SAFETY: a `RunLoopSignal` is used only to call `CFRunLoopSourceSignal`, `CFRunLoopWakeUp`,
// `CFRunLoopStop` and `CFRunLoopSourceInvalidate`. Core Foundation documents run loops and
// run-loop sources as safe to use from any thread, and these calls exist to be made from
// other threads. The struct holds its own retained references to both objects.
unsafe impl Send for RunLoopSignal {}
// SAFETY: as for `Send`; none of the calls needs exclusive access.
unsafe impl Sync for RunLoopSignal {}

impl RunLoopSignal {
    /// Adds a source to `run_loop` (in the common modes) that calls `perform(info)` on that
    /// loop's thread each time the signal is raised.
    ///
    /// # Safety
    ///
    /// `info` must be valid for whatever `perform` does with it until [`retire`](Self::retire)
    /// has returned or the run loop is gone for good.
    pub(crate) unsafe fn install(
        run_loop: CFRetained<CFRunLoop>,
        perform: unsafe extern "C-unwind" fn(*mut c_void),
        info: *mut c_void,
    ) -> Option<Self> {
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info,
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform),
        };
        // SAFETY: `context` is a valid version-0 context for the duration of the call (Core
        // Foundation copies it). It has no retain or release callbacks, so the validity of
        // `info` is the caller's promise.
        let source = unsafe { CFRunLoopSource::new(None, 0, &raw mut context) }?;
        run_loop.add_source(Some(&source), common_modes());
        Some(Self { run_loop, source })
    }

    /// Makes the run loop call `perform` soon. Callable from any thread.
    pub(crate) fn raise(&self) {
        self.source.signal();
        self.run_loop.wake_up();
    }

    /// Makes the run loop's current run return. Callable from any thread.
    pub(crate) fn stop_run_loop(&self) {
        self.run_loop.stop();
    }

    /// Removes the source from its run loop: `perform` is not called again.
    pub(crate) fn retire(&self) {
        self.source.invalidate();
    }
}

fn install_on_main() -> Option<RunLoopSignal> {
    // SAFETY: `perform` ignores `info`, which is null.
    unsafe { RunLoopSignal::install(CFRunLoop::main()?, perform, ptr::null_mut()) }
}

/// The run-loop source's callback: the main thread runs what was queued.
unsafe extern "C-unwind" fn perform(_info: *mut c_void) {
    if let Some(mtm) = MainThreadMarker::new() {
        run_pending(mtm);
    }
}

fn jobs() -> MutexGuard<'static, VecDeque<Job>> {
    // A job runs outside the lock, so a panicking job cannot leave the queue half-changed.
    JOBS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queues `job` for the main thread and returns at once.
pub(crate) fn post(job: impl FnOnce(MainThreadMarker) + Send + 'static) {
    jobs().push_back(Box::new(job));
    if let Some(signal) = SIGNAL.get_or_init(install_on_main) {
        signal.raise();
    }
}

/// Runs every queued job now. The marker proves the caller is on the main thread.
pub(crate) fn run_pending(mtm: MainThreadMarker) {
    loop {
        let Some(job) = jobs().pop_front() else {
            return;
        };
        // A job must not take the run loop down with it, and unwinding must not reach the
        // Core Foundation frames below the callback.
        let _ = catch_unwind(AssertUnwindSafe(|| job(mtm)));
    }
}

/// Runs `work` on the main thread and waits for its result, at most `timeout`.
///
/// On the main thread the queued jobs and then `work` run at once. From any other thread the
/// work is queued; `None` means the main thread did not get to it in time, and the work is
/// then cancelled: it will not run later. (`None` is also the answer when `work` panicked.)
pub(crate) fn call<R: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce(MainThreadMarker) -> R + Send + 'static,
) -> Option<R> {
    if let Some(mtm) = MainThreadMarker::new() {
        run_pending(mtm);
        return Some(work(mtm));
    }
    let reply = Arc::new(Reply::default());
    let job_reply = Arc::clone(&reply);
    post(move |mtm| {
        // Cancelling and starting exclude each other, so the caller either gets a result or
        // knows the work never started.
        if !job_reply.claim() {
            return;
        }
        // The caller waits for a job that has started, so it must hear back even when the
        // work panics.
        let outcome = catch_unwind(AssertUnwindSafe(|| work(mtm))).ok();
        job_reply.finish(outcome);
    });
    reply.wait(timeout)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use super::*;

    // The test harness keeps the main thread to itself and never runs its run loop, which is
    // exactly the situation `call` must survive.
    #[test]
    fn a_call_that_the_main_thread_never_serves_times_out_and_is_cancelled() {
        if MainThreadMarker::new().is_some() {
            return;
        }
        let (ran, did_run) = channel();
        let started = Instant::now();
        let result = call(Duration::from_millis(50), move |_| {
            let _ = ran.send(());
            7
        });
        assert_eq!(result, None);
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(started.elapsed() < Duration::from_secs(5));
        // Even if the main thread came to the job later, it would do nothing.
        assert!(did_run.try_recv().is_err());
    }
}
