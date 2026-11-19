//! `SessionEvents` with the real main thread (a test with its own `main`, as `main_loop.rs`):
//! the observer is registered on the main run loop, a session notification posted on the
//! workspace's centre and the screen-lock notification posted on the distributed centre reach
//! the sink, and nothing reaches it after `unwatch_session`.
//!
//! A real screen lock, user switch or logout cannot be caused from a test; those need a person
//! at a Mac (`docs/compatibility-run.md`).

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::Write;
    use std::process::ExitCode;
    use std::sync::mpsc::{Receiver, channel};
    use std::thread;
    use std::time::Duration;

    use iamb_platform::{PlatformError, SessionEvent, SessionEvents};
    use iamb_platform_macos::{Platform, run_with_main_loop};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceWillPowerOffNotification};
    use objc2_foundation::{NSDistributedNotificationCenter, NSString};

    const PASSED: i32 = 42;
    const DEADLINE: Duration = Duration::from_secs(30);
    const WAIT: Duration = Duration::from_secs(5);

    fn say(line: &str) {
        let _ = writeln!(std::io::stderr(), "session: {line}");
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
                say("FAILED: the test did not end in time");
                std::process::exit(1);
            });
        if run_with_main_loop(checks) == PASSED {
            say("passed");
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    /// Posts the workspace's power-off notification. The workspace's centre is an ordinary
    /// notification centre: it delivers on the posting thread, at once.
    fn post_power_off() {
        // SAFETY: a constant the framework initialises when it is loaded.
        let name = unsafe { NSWorkspaceWillPowerOffNotification };
        // SAFETY: a notification with no object.
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .postNotificationName_object(name, None);
        }
    }

    fn next(events: &Receiver<SessionEvent>) -> Option<SessionEvent> {
        events.recv_timeout(WAIT).ok()
    }

    fn checks() -> i32 {
        let platform = Platform::new();
        let (tx, events) = channel();
        if let Err(e) = platform.watch_session(Box::new(move |e: SessionEvent| {
            let _ = tx.send(e);
        })) {
            say(&format!("FAILED: watch_session: {e}"));
            return 1;
        }
        let mut code = PASSED;
        let mut check = |what: &str, ok: bool| {
            if ok {
                say(&format!("ok: {what}"));
            } else {
                say(&format!("FAILED: {what}"));
                code = 1;
            }
        };
        check(
            "a second watcher is refused",
            matches!(
                platform.watch_session(Box::new(|_: SessionEvent| {})),
                Err(PlatformError::AlreadyRunning)
            ),
        );
        post_power_off();
        check(
            "the workspace's power-off notification is a sign-out",
            next(&events) == Some(SessionEvent::SigningOut),
        );
        let name = NSString::from_str("com.apple.screenIsLocked");
        // SAFETY: a notification with no object; delivered to observers in this process too.
        unsafe {
            NSDistributedNotificationCenter::defaultCenter()
                .postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true);
        }
        check(
            "the distributed screen-lock notification is a lock",
            next(&events) == Some(SessionEvent::Locked),
        );
        check("unwatch succeeds", platform.unwatch_session().is_ok());
        post_power_off();
        check(
            "nothing is reported after unwatch",
            events.recv_timeout(Duration::from_millis(500)).is_err(),
        );
        code
    }
}
