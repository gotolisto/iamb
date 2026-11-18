//! `run_with_main_loop` with the real main thread (a test with its own `main`: the standard
//! harness keeps the main thread for itself and runs every test on another thread).
//!
//! The closure runs on a thread of its own while the main thread runs the application event
//! loop, and
//!
//! 1. starts and stops the real listener. With Input Monitoring and Accessibility granted,
//!    `start` waits for the main thread to read the keyboard layout, so `Ok` proves the main
//!    thread answered. Without them (a hosted CI runner) `start` refuses with
//!    `PermissionDenied` before it asks the main thread anything, which proves nothing about
//!    the loop, so the test says so and relies on step 2;
//! 2. asks the main thread for work directly, through a run-loop source on the main run
//!    loop (the mechanism the listener uses), and checks that the answer comes from the main
//!    thread while the application event loop is running;
//! 3. checks that, called off the main thread, `run_with_main_loop` runs its closure inline.
//!
//! Then `run_with_main_loop` must stop the loop and return the closure's exit code. A
//! watchdog fails the test instead of letting it hang when the loop does not stop.

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::io::Write;
    use std::process::ExitCode;
    use std::ptr;
    use std::sync::Mutex;
    use std::sync::mpsc::{Sender, channel};
    use std::thread;
    use std::time::Duration;

    use iamb_platform::{EventSink, KeyListener, ListenerEvent, PlatformError};
    use iamb_platform_macos::{Platform, run_with_main_loop};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_core_foundation::{
        CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopCommonModes,
    };

    /// What the closure returns when every check passed.
    const PASSED: i32 = 42;
    /// How long the whole test may take before the watchdog fails it.
    const DEADLINE: Duration = Duration::from_secs(30);
    /// How long the main thread gets to answer the direct request.
    const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

    /// Where the run-loop source's callback sends what it saw: (on the main thread, the
    /// application event loop running).
    static ANSWER: Mutex<Option<Sender<(bool, bool)>>> = Mutex::new(None);

    fn say(line: &str) {
        let _ = writeln!(std::io::stderr(), "main_loop: {line}");
    }

    pub(super) fn main() -> ExitCode {
        if MainThreadMarker::new().is_none() {
            say("FAILED: the test's own `main` is not on the main thread");
            return ExitCode::FAILURE;
        }
        let _ = thread::Builder::new()
            .name("watchdog".to_owned())
            .spawn(|| {
                thread::sleep(DEADLINE);
                say("FAILED: the main loop did not stop in time");
                std::process::exit(1);
            });
        let code = run_with_main_loop(checks);
        if code == PASSED {
            say("passed");
            ExitCode::SUCCESS
        } else {
            say(&format!("FAILED: the closure returned {code}"));
            ExitCode::FAILURE
        }
    }

    /// The closure: every check, on the work thread. Returns [`PASSED`] or 1.
    fn checks() -> i32 {
        let results = [
            (
                "runs off the main thread",
                MainThreadMarker::new().is_none(),
            ),
            ("the listener starts and stops", listener_starts_and_stops()),
            (
                "the main thread serves the main run loop",
                main_thread_answers(),
            ),
            ("off the main thread the closure runs inline", runs_inline()),
        ];
        let mut code = PASSED;
        for (check, passed) in results {
            if passed {
                say(&format!("ok: {check}"));
            } else {
                say(&format!("FAILED: {check}"));
                code = 1;
            }
        }
        code
    }

    fn sink() -> Box<dyn EventSink> {
        let (sender, receiver) = channel::<ListenerEvent>();
        Box::new(move |event: ListenerEvent| {
            let _ = &receiver;
            let _ = sender.send(event);
        })
    }

    fn listener_starts_and_stops() -> bool {
        let platform = Platform::new();
        match platform.start(sink()) {
            Ok(()) => {
                say("the listener started: the main thread read the keyboard layout");
                platform.stop() == Ok(())
            }
            Err(PlatformError::PermissionDenied(kind)) => {
                say(&format!(
                    "the listener was refused for lack of {kind:?}, before it asked the main \
                     thread; the next check covers the main thread"
                ));
                true
            }
            Err(other) => {
                say(&format!("the listener did not start: {other}"));
                false
            }
        }
    }

    /// The run-loop source's callback, on the thread whose run loop runs it.
    unsafe extern "C-unwind" fn answer(_info: *mut c_void) {
        let seen = match MainThreadMarker::new() {
            Some(mtm) => (true, NSApplication::sharedApplication(mtm).isRunning()),
            None => (false, false),
        };
        if let Some(sender) = ANSWER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(seen);
        }
    }

    fn main_thread_answers() -> bool {
        let (sender, receiver) = channel();
        *ANSWER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        let Some(main) = CFRunLoop::main() else {
            say("there is no main run loop");
            return false;
        };
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(answer),
        };
        // SAFETY: `context` is a valid version-0 context for the duration of the call (Core
        // Foundation copies it); `info` is null and `answer` does not read it.
        let Some(source) = (unsafe { CFRunLoopSource::new(None, 0, &raw mut context) }) else {
            say("no run-loop source");
            return false;
        };
        // SAFETY: a constant the framework initialises before any code of this process runs,
        // and never changes.
        let modes = unsafe { kCFRunLoopCommonModes };
        main.add_source(Some(&source), modes);
        source.signal();
        main.wake_up();
        let seen = receiver.recv_timeout(ANSWER_TIMEOUT);
        source.invalidate();
        match seen {
            Ok((true, true)) => true,
            Ok((on_main, running)) => {
                say(&format!(
                    "answered on the main thread: {on_main}; event loop running: {running}"
                ));
                false
            }
            Err(_) => {
                say("the main thread did not answer");
                false
            }
        }
    }

    fn runs_inline() -> bool {
        let caller = thread::current().id();
        run_with_main_loop(move || i32::from(thread::current().id() == caller)) == 1
    }
}
