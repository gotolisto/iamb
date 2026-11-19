//! `FocusInspector` against the real system, with a real main loop, a real window and a real
//! password field (a test with its own `main`: the standard harness keeps the main thread
//! for itself, and the secure-entry flag, the activation notification and the window all
//! need it).
//!
//! The main thread runs the application event loop (`run_with_main_loop`); the checks run on
//! the work thread, as the service's threads do, and hand the main thread what must happen
//! there through a run-loop source.
//!
//! # What runs where
//!
//! | Check | Needs | Without it |
//! | --- | --- | --- |
//! | `focused_app` names an application | a frontmost application | skipped |
//! | the test window is the focus token; the test process is one of Iamb's own | the window can come to the front (an unlocked, interactive session) | skipped, and the checks below use whatever window has the focus |
//! | a plain text field is `Normal` | the Accessibility API answers this process | skipped when it refuses for lack of the Accessibility permission |
//! | secure keyboard entry switched on by this process is `Secure`, and off again is as before | a focused window | skipped |
//! | a focused `NSSecureTextField` is `Secure`, and the plain field after it is as before | the test window in front | skipped |
//! | an answer comes within the service's timeout | a focused window | skipped |
//! | `watch`: first report, `AlreadyRunning`, a field change, `unwatch` | Accessibility for the process that runs the tests | checked to be refused with `PermissionDenied(Accessibility)` |
//! | the listener stamps a key that goes to the test window with this process (finding 5 on macOS), and says which source stamped it | the test window in front, Input Monitoring and Accessibility | skipped |
//! | `restore_focus` on a process that does not exist; with the test window brought back to the front first (and that checked), `restore_focus` on it takes the fast path | a focused window | the second half is skipped |
//!
//! A skipped check writes `SKIPPED` and its reason to standard error; a failed one writes
//! `FAILED` and the process exits with a failure. Nothing is typed (the stamp check presses
//! Left, which types nothing), and no field's content is read: the fields stay empty.
//!
//! If secure keyboard entry is already on when the test starts (another program left it on,
//! or the screen is locked), every answer is `Secure` and the checks that need `Normal` are
//! skipped.

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::io::Write;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::process::ExitCode;
    use std::ptr;
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use iamb_platform::{
        EventSink, FieldSecurity, FocusChange, FocusInspector, FocusToken, Grant, KeyListener,
        KeyTarget, ListenerEvent, PermissionKind, Permissions, PlatformError, SECURE_STATE_TIMEOUT,
        UnknownReason,
    };
    use iamb_platform_macos::{Platform, run_with_main_loop};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSResponder, NSSecureTextField, NSTextField, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_core_foundation::{
        CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CGPoint, CGRect, CGSize,
        kCFRunLoopCommonModes,
    };
    use objc2_core_graphics::{
        CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    };

    /// What the closure returns when no check failed.
    const PASSED: i32 = 42;
    /// How long the whole test may take before the watchdog fails it.
    const DEADLINE: Duration = Duration::from_mins(2);
    /// How long the main thread gets for one job.
    const MAIN_TIMEOUT: Duration = Duration::from_secs(5);
    /// How long the test window gets to come to the front.
    const FRONT_TIMEOUT: Duration = Duration::from_secs(5);
    /// How long a change of the focused field gets to show in the answer.
    const SETTLE: Duration = Duration::from_secs(3);
    /// A generous timeout for `secure_state`, so that a slow runner is not a failure.
    const PATIENT: Duration = Duration::from_secs(2);

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        /// `OSStatus EnableSecureEventInput(void)` (`HIToolbox/CarbonEventsCore.h`): switches
        /// secure keyboard entry on for this process; calls are counted.
        fn EnableSecureEventInput() -> i32;
        /// `OSStatus DisableSecureEventInput(void)`: undoes one `EnableSecureEventInput`.
        fn DisableSecureEventInput() -> i32;
        /// `Boolean IsSecureEventInputEnabled(void)`: whether any process has it on.
        fn IsSecureEventInputEnabled() -> u8;
    }

    fn say(line: &str) {
        let _ = writeln!(std::io::stderr(), "focus: {line}");
    }

    /// The outcome of the checks so far.
    #[derive(Default)]
    struct Run {
        failed: bool,
    }

    impl Run {
        fn ok(what: &str) {
            say(&format!("ok: {what}"));
        }

        fn skipped(what: &str, reason: &str) {
            say(&format!("SKIPPED {what}: {reason}"));
        }

        fn failed(&mut self, what: &str, why: &str) {
            say(&format!("FAILED: {what}: {why}"));
            self.failed = true;
        }

        fn check(&mut self, what: &str, passed: bool, why: impl FnOnce() -> String) {
            if passed {
                Self::ok(what);
            } else {
                self.failed(what, &why());
            }
        }
    }

    // ---- Work for the main thread ---------------------------------------------------------

    type Job = Box<dyn FnOnce(MainThreadMarker) + Send>;

    static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
    static SOURCE: OnceLock<Option<MainSource>> = OnceLock::new();

    /// A run-loop source on the main run loop that runs the queued jobs.
    struct MainSource {
        run_loop: CFRetained<CFRunLoop>,
        source: CFRetained<CFRunLoopSource>,
    }

    // SAFETY: used only to call `CFRunLoopSourceSignal` and `CFRunLoopWakeUp`, which Core
    // Foundation documents as callable from any thread.
    unsafe impl Send for MainSource {}
    // SAFETY: as for `Send`; neither call needs exclusive access.
    unsafe impl Sync for MainSource {}

    fn jobs() -> std::sync::MutexGuard<'static, VecDeque<Job>> {
        JOBS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The source's callback, on the main thread.
    unsafe extern "C-unwind" fn perform(_info: *mut c_void) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        loop {
            let Some(job) = jobs().pop_front() else {
                return;
            };
            let _ = catch_unwind(AssertUnwindSafe(|| job(mtm)));
        }
    }

    fn install() -> Option<MainSource> {
        let run_loop = CFRunLoop::main()?;
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
            perform: Some(perform),
        };
        // SAFETY: `context` is a valid version-0 context for the duration of the call (Core
        // Foundation copies it); `info` is null and `perform` does not read it.
        let source = unsafe { CFRunLoopSource::new(None, 0, &raw mut context) }?;
        // SAFETY: a constant the framework initialises before any code of this process runs,
        // and never changes.
        let modes = unsafe { kCFRunLoopCommonModes };
        run_loop.add_source(Some(&source), modes);
        Some(MainSource { run_loop, source })
    }

    /// Runs `work` on the main thread and returns its result; `None` when the main thread did
    /// not get to it.
    fn on_main<R: Send + 'static>(
        work: impl FnOnce(MainThreadMarker) -> R + Send + 'static,
    ) -> Option<R> {
        let (sender, receiver) = channel();
        jobs().push_back(Box::new(move |mtm| {
            let _ = sender.send(work(mtm));
        }));
        let main = SOURCE.get_or_init(install).as_ref()?;
        main.source.signal();
        main.run_loop.wake_up();
        receiver.recv_timeout(MAIN_TIMEOUT).ok()
    }

    // ---- The test window, on the main thread ----------------------------------------------

    struct Ui {
        window: Retained<NSWindow>,
        plain: Retained<NSTextField>,
        secret: Retained<NSSecureTextField>,
    }

    thread_local! {
        /// The window and its two fields. Only the main thread ever touches it.
        static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Copy)]
    enum Field {
        Plain,
        Secret,
    }

    const fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }

    /// Opens a small window with an empty text field and an empty password field, brings it
    /// to the front and gives the text field the focus. Returns the window's number.
    fn open_window(mtm: MainThreadMarker) -> u64 {
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: a new window from a valid frame, style and backing type, created on the
        // main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect(200.0, 200.0, 360.0, 120.0),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the window is owned by the `Retained` kept in `UI`; it must not also
        // release itself when it is closed.
        unsafe { window.setReleasedWhenClosed(false) };
        let plain = NSTextField::initWithFrame(mtm.alloc(), rect(20.0, 70.0, 320.0, 24.0));
        let secret = NSSecureTextField::initWithFrame(mtm.alloc(), rect(20.0, 30.0, 320.0, 24.0));
        if let Some(content) = window.contentView() {
            content.addSubview(&plain);
            content.addSubview(&secret);
        }
        window.makeKeyAndOrderFront(None);
        // The newer `activate` only asks; a test binary started from a terminal has to take
        // the front.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        let responder: &NSResponder = &plain;
        let _ = window.makeFirstResponder(Some(responder));
        let number = u64::try_from(window.windowNumber()).unwrap_or(0);
        UI.with(|ui| {
            *ui.borrow_mut() = Some(Ui {
                window,
                plain,
                secret,
            });
        });
        number
    }

    /// Gives `field` the keyboard focus; `false` when the window refused.
    fn focus_field(field: Field) -> bool {
        on_main(move |_mtm| {
            UI.with(|ui| {
                ui.borrow().as_ref().is_some_and(|ui| {
                    let responder: &NSResponder = match field {
                        Field::Plain => &ui.plain,
                        Field::Secret => &ui.secret,
                    };
                    ui.window.makeFirstResponder(Some(responder))
                })
            })
        })
        .unwrap_or(false)
    }

    /// Brings the test window to the front again and gives the plain field the focus.
    fn bring_window_back() -> bool {
        on_main(|mtm| {
            UI.with(|ui| {
                ui.borrow().as_ref().is_some_and(|ui| {
                    ui.window.makeKeyAndOrderFront(None);
                    #[allow(deprecated)]
                    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
                    let responder: &NSResponder = &ui.plain;
                    ui.window.makeFirstResponder(Some(responder))
                })
            })
        })
        .unwrap_or(false)
    }

    fn close_window() {
        let _ = on_main(|_mtm| {
            UI.with(|ui| {
                if let Some(ui) = ui.borrow_mut().take() {
                    ui.window.orderOut(None);
                }
            });
        });
    }

    fn secure_input_is_on() -> Option<bool> {
        // SAFETY: no arguments; reads a flag of the login session, on the main thread.
        on_main(|_mtm| unsafe { IsSecureEventInputEnabled() != 0 })
    }

    // ---- The checks, on the work thread ---------------------------------------------------

    /// Polls `read` until it gives `Some`, at most `timeout`.
    fn wait_for<T>(timeout: Duration, mut read: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = read() {
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Polls `secure_state` until it answers `wanted`, at most [`SETTLE`]; returns the last
    /// answer.
    fn settles_to(platform: &Platform, token: FocusToken, wanted: FieldSecurity) -> FieldSecurity {
        let mut last = platform.secure_state(&token, PATIENT);
        let deadline = Instant::now() + SETTLE;
        while last != wanted && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
            last = platform.secure_state(&token, PATIENT);
        }
        last
    }

    fn focused_app_names_an_application(run: &mut Run, platform: &Platform) {
        let what = "focused_app names an application";
        match platform.focused_app() {
            Ok(app) => run.check(what, !app.app.0.is_empty() && app.pid != 0, || {
                "an empty name or no process id".to_owned()
            }),
            Err(PlatformError::NoFocus) => {
                Run::skipped(what, "no application is frontmost in this session");
            }
            Err(other) => run.failed(what, &other.to_string()),
        }
    }

    /// Opens the test window and waits for it to be the focused one. `Some(token)` when it
    /// is.
    fn window_comes_to_the_front(run: &mut Run, platform: &Platform) -> Option<FocusToken> {
        let what = "the test window is the focus token";
        let own = std::process::id();
        let Some(number) = on_main(open_window) else {
            run.failed(what, "the main thread did not open the window");
            return None;
        };
        let front = wait_for(FRONT_TIMEOUT, || {
            platform.focus_token().ok().filter(|t| t.pid() == own)
        });
        let Some(token) = front else {
            Run::skipped(
                what,
                "the window did not come to the front (no interactive session, or another \
                 application keeps the front)",
            );
            return None;
        };
        run.check(what, token.window() == number && number != 0, || {
            format!(
                "the token names window {}, the window's number is {number}",
                token.window()
            )
        });
        match platform.focused_app() {
            Ok(app) => run.check(
                "the test process in front is one of Iamb's own",
                app.pid == own && app.own,
                || format!("pid {} (own pid {own}), own: {}", app.pid, app.own),
            ),
            Err(error) => run.failed(
                "focused_app with the test window in front",
                &error.to_string(),
            ),
        }
        Some(token)
    }

    /// The answer for the focused plain field; `Some(Normal)` when the element path works.
    fn a_plain_field_is_normal(
        run: &mut Run,
        platform: &Platform,
        token: FocusToken,
        accessibility: bool,
    ) -> FieldSecurity {
        let what = "a plain text field is not secure";
        let answer = settles_to(platform, token, FieldSecurity::Normal);
        match answer {
            FieldSecurity::Normal => Run::ok(what),
            FieldSecurity::Unknown(UnknownReason::NotInspectable) if !accessibility => {
                Run::skipped(
                    what,
                    "the Accessibility API does not answer this process: grant Accessibility \
                     to the program that runs the tests",
                );
            }
            other => run.failed(what, &format!("{other:?}")),
        }
        answer
    }

    fn secure_keyboard_entry_is_secure(
        run: &mut Run,
        platform: &Platform,
        token: FocusToken,
        before: FieldSecurity,
    ) {
        // SAFETY: no arguments; counted, and undone below, on the main thread.
        let Some(0) = on_main(|_mtm| unsafe { EnableSecureEventInput() }) else {
            run.failed(
                "switching secure keyboard entry on",
                "the system refused, or the main thread did not answer",
            );
            return;
        };
        let during = platform.secure_state(&token, PATIENT);
        // SAFETY: undoes the one `EnableSecureEventInput` above.
        let disabled = on_main(|_mtm| unsafe { DisableSecureEventInput() });
        run.check(
            "secure keyboard entry switched on is secure",
            during == FieldSecurity::Secure,
            || format!("{during:?}"),
        );
        if disabled != Some(0) {
            run.failed(
                "switching secure keyboard entry off",
                "the system refused, or the main thread did not answer",
            );
            return;
        }
        let after = settles_to(platform, token, before);
        run.check(
            "secure keyboard entry switched off is as before",
            after == before,
            || format!("{after:?}, before {before:?}"),
        );
    }

    fn a_password_field_is_secure(
        run: &mut Run,
        platform: &Platform,
        token: FocusToken,
        before: FieldSecurity,
    ) {
        if !focus_field(Field::Secret) {
            run.failed(
                "a focused password field is secure",
                "the window did not give the password field the focus",
            );
            return;
        }
        let during = settles_to(platform, token, FieldSecurity::Secure);
        match secure_input_is_on() {
            Some(on) => say(&format!(
                "note: with the password field focused, secure keyboard entry is {}",
                if on { "on" } else { "off" }
            )),
            None => say("note: the main thread did not say whether secure keyboard entry is on"),
        }
        run.check(
            "a focused password field is secure",
            during == FieldSecurity::Secure,
            || format!("{during:?}"),
        );
        if !focus_field(Field::Plain) {
            run.failed(
                "the plain field after the password field",
                "the window did not give the plain field the focus",
            );
            return;
        }
        let after = settles_to(platform, token, before);
        run.check(
            "the plain field after the password field is as before",
            after == before,
            || format!("{after:?}, before {before:?}"),
        );
    }

    fn an_answer_comes_within_the_services_timeout(
        run: &mut Run,
        platform: &Platform,
        token: FocusToken,
    ) {
        let started = Instant::now();
        let answer = platform.secure_state(&token, SECURE_STATE_TIMEOUT);
        let waited = started.elapsed();
        say(&format!(
            "note: secure_state answered {answer:?} after {waited:?}"
        ));
        // The caller is never held longer than its timeout, whatever the answer; the margin
        // is for a busy runner.
        run.check(
            "secure_state returns within its timeout",
            waited < SECURE_STATE_TIMEOUT + Duration::from_millis(250),
            || format!("{waited:?}"),
        );
    }

    fn next_change(changes: &Receiver<FocusChange>, timeout: Duration) -> Option<FocusChange> {
        changes.recv_timeout(timeout).ok()
    }

    fn the_watcher_reports(
        run: &mut Run,
        platform: &Platform,
        window: Option<FocusToken>,
        accessibility: bool,
    ) {
        let (sender, changes) = channel();
        let sink = move |change: FocusChange| {
            let _ = sender.send(change);
        };
        let started = platform.watch(Box::new(sink));
        if !accessibility {
            run.check(
                "watch is refused without Accessibility",
                started
                    == Err(PlatformError::PermissionDenied(
                        PermissionKind::Accessibility,
                    )),
                || format!("{started:?}"),
            );
            Run::skipped(
                "the watcher's reports",
                "the process that runs the tests has no Accessibility permission",
            );
            return;
        }
        if let Err(error) = started {
            run.failed("watch starts", &error.to_string());
            return;
        }
        // The current focus, once, at once: it is there when `watch` returns.
        match (changes.try_recv().ok(), platform.focused_app().ok()) {
            (Some(first), Some(app)) => run.check(
                "watch reports the current focus at once",
                first.app.pid == app.pid && first.token.pid() == app.pid,
                || format!("reported pid {}, frontmost pid {}", first.app.pid, app.pid),
            ),
            (Some(_), None) => Run::ok("watch reports the current focus at once"),
            (None, Some(_)) => run.failed(
                "watch reports the current focus at once",
                "nothing was reported when watch returned",
            ),
            (None, None) => Run::skipped(
                "watch reports the current focus at once",
                "no application is frontmost",
            ),
        }
        let again = platform.watch(Box::new(|_: FocusChange| {}));
        run.check(
            "a second watch is refused",
            again == Err(PlatformError::AlreadyRunning),
            || format!("{again:?}"),
        );
        run.check("the watcher's thread runs", platform.is_watching(), || {
            "is_watching is false while watching".to_owned()
        });

        if let Some(token) = window {
            // Let the reports of the start (the settling looks) pass.
            while next_change(&changes, Duration::from_secs(1)).is_some() {}
            let what = "a move to the password field is reported";
            if focus_field(Field::Secret) {
                match next_change(&changes, SETTLE) {
                    Some(change) => run.check(what, change.token == token, || {
                        format!(
                            "the report names {:?}, the window is {token:?}",
                            change.token
                        )
                    }),
                    None => run.failed(what, "nothing was reported"),
                }
            } else {
                run.failed(what, "the window did not give the password field the focus");
            }
        } else {
            Run::skipped(
                "a move to the password field is reported",
                "the test window is not in front",
            );
        }

        let stopped = platform.unwatch();
        run.check("unwatch stops the watcher", stopped == Ok(()), || {
            format!("{stopped:?}")
        });
        run.check(
            "is_watching after unwatch is false",
            !platform.is_watching(),
            || "is_watching is true after unwatch".to_owned(),
        );
        while changes.try_recv().is_ok() {}
        if window.is_some() {
            let _ = focus_field(Field::Plain);
            let late = next_change(&changes, Duration::from_millis(700));
            run.check("nothing is reported after unwatch", late.is_none(), || {
                "a change arrived after unwatch returned".to_owned()
            });
        }
        let again = platform.unwatch();
        run.check("unwatch twice is not an error", again == Ok(()), || {
            format!("{again:?}")
        });
    }

    /// A sink that keeps each event with the target it was delivered with.
    struct Stamps(std::sync::mpsc::Sender<(ListenerEvent, Option<KeyTarget>)>);

    impl EventSink for Stamps {
        fn deliver(&mut self, event: ListenerEvent) {
            let _ = self.0.send((event, None));
        }
        fn deliver_to(&mut self, event: ListenerEvent, target: KeyTarget) {
            let _ = self.0.send((event, Some(target)));
        }
    }

    /// One of the listener's stamp counters, read from the platform's health report.
    fn counter(report: &str, name: &str) -> u64 {
        report
            .split(&format!("{name}: "))
            .nth(1)
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }

    /// The 2.8 review's finding 5 on macOS: with the test window in front and the watcher
    /// running, Left is pressed at the hardware end of the stream. The listener must stamp it
    /// with this process. Which source stamped it (the event's `kCGEventTargetUnixProcessID`,
    /// or the frontmost application as the watcher noted it) is written as a note, so the
    /// CI log says whether the system fills the field in for a session event tap.
    fn keys_are_stamped_with_this_process(run: &mut Run, platform: &Platform, window: bool) {
        let what = "the listener stamps a key typed into the test window with this process";
        if !window {
            Run::skipped(what, "the test window is not in front");
            return;
        }
        if let Err(error) = platform.watch(Box::new(|_: FocusChange| {})) {
            Run::skipped(what, &format!("the focus watcher did not start: {error}"));
            return;
        }
        let (sender, stamps) = channel();
        match platform.start(Box::new(Stamps(sender))) {
            Ok(()) => {}
            Err(error) => {
                let _ = platform.unwatch();
                Run::skipped(what, &format!("the listener did not start: {error}"));
                return;
            }
        }
        let own = std::process::id();
        let front = bring_window_back()
            && wait_for(FRONT_TIMEOUT, || {
                platform.focus_token().ok().filter(|t| t.pid() == own)
            })
            .is_some();
        if !front {
            let _ = platform.stop();
            let _ = platform.unwatch();
            Run::skipped(what, "the test window did not come back to the front");
            return;
        }
        // Let the watcher's looks of the move settle, so the frontmost note is this process.
        thread::sleep(Duration::from_millis(300));
        while stamps.try_recv().is_ok() {}
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
        for down in [true, false] {
            if let Some(event) = CGEvent::new_keyboard_event(source.as_deref(), 0x7B, down) {
                CGEvent::set_flags(Some(&event), CGEventFlags(0));
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stamp = None;
        while let Ok((event, target)) =
            stamps.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if matches!(event, ListenerEvent::Boundary { .. }) {
                stamp = Some(target);
                break;
            }
        }
        let report = format!("{platform:?}");
        let _ = platform.stop();
        let _ = platform.unwatch();
        let (event, frontmost, unknown, disagreements) = (
            counter(&report, "targets_from_event"),
            counter(&report, "targets_from_frontmost"),
            counter(&report, "targets_unknown"),
            counter(&report, "target_disagreements"),
        );
        let by = if event > 0 {
            "the event's own kCGEventTargetUnixProcessID"
        } else if frontmost > 0 {
            "the frontmost application as the watcher noted it (the event's field was 0)"
        } else {
            "nothing (no source knew)"
        };
        say(&format!(
            "note: key stamp source: {by}; key presses stamped from the event {event}, from the \
             frontmost application {frontmost}, not stamped {unknown}; the event's field \
             disagreed with the frontmost application {disagreements} times"
        ));
        match stamp {
            None => run.failed(what, "the key did not reach the listener's sink"),
            Some(target) => run.check(what, target == Some(KeyTarget::process(own)), || {
                format!("stamped {target:?}, this process is {own}")
            }),
        }
    }

    /// `restore_focus` on the window that has the focus takes the fast path: it sees the
    /// window in front and returns without activating anything. The slow path (activate,
    /// then wait for the window) is covered by the unit tests of `focus::restore`, which
    /// script when the window comes back.
    ///
    /// The step first makes sure the window really is in front. On a CI runner it once was
    /// not (an earlier step or another program had left something else in front), the fast
    /// path missed, the activation took 500 ms, and the check failed for a reason it does not
    /// test. What was in front is said as a note.
    fn the_focus_is_given_back(
        run: &mut Run,
        platform: &Platform,
        window: Option<FocusToken>,
        target: Option<FocusToken>,
    ) {
        let nowhere = FocusToken::from_raw(u32::MAX - 1, 1);
        let lost = platform.restore_focus(&nowhere);
        run.check(
            "restore_focus on a process that does not exist is focus lost",
            lost == Err(PlatformError::FocusLost),
            || format!("{lost:?}"),
        );
        let Some(target) = target else {
            Run::skipped(
                "restore_focus on the focused window",
                "no window has the focus",
            );
            return;
        };
        let in_front = platform.focus_token().ok();
        if in_front != Some(target) {
            say(&format!(
                "note: before restore_focus, {in_front:?} was in front, not {target:?} \
                 (frontmost application: {:?})",
                platform.focused_app().map(|app| app.pid)
            ));
        }
        let token = if window.is_some() {
            let what = "the test window is in front again before restore_focus";
            let back = bring_window_back()
                && wait_for(FRONT_TIMEOUT, || {
                    platform.focus_token().ok().filter(|t| *t == target)
                })
                .is_some();
            run.check(what, back, || {
                format!(
                    "{:?} is in front, the test window is {target:?}",
                    platform.focus_token()
                )
            });
            if !back {
                return;
            }
            target
        } else {
            // Without the test window: whatever window has the focus now.
            let Ok(token) = platform.focus_token() else {
                Run::skipped(
                    "restore_focus on the focused window",
                    "no window has the focus",
                );
                return;
            };
            token
        };
        let started = Instant::now();
        let restored = platform.restore_focus(&token);
        run.check(
            "restore_focus on the focused window succeeds at once",
            restored == Ok(()) && started.elapsed() < Duration::from_millis(250),
            || format!("{restored:?} after {:?}", started.elapsed()),
        );
    }

    /// The closure: every check, on the work thread. Returns [`PASSED`] or 1.
    fn checks() -> i32 {
        let mut run = Run::default();
        let platform = Platform::new();
        let accessibility = platform.permission_status().accessibility == Grant::Granted;
        say(&format!(
            "the process that runs the tests {} the Accessibility permission",
            if accessibility {
                "has"
            } else {
                "does not have"
            }
        ));

        focused_app_names_an_application(&mut run, &platform);
        let allowed = platform.allow_foreground(std::process::id());
        run.check("allow_foreground succeeds", allowed == Ok(()), || {
            format!("{allowed:?}")
        });

        let window = window_comes_to_the_front(&mut run, &platform);
        // Without the test window, whatever window has the focus serves for the checks that
        // only need one.
        let target = window.or_else(|| platform.focus_token().ok());

        let stuck = secure_input_is_on();
        match (target, stuck) {
            (None, _) => Run::skipped("the secure-field checks", "no window has the focus"),
            (Some(_), None) => run.failed(
                "reading the secure-entry flag",
                "the main thread did not answer",
            ),
            (Some(token), Some(true)) => {
                let answer = platform.secure_state(&token, PATIENT);
                run.check(
                    "secure keyboard entry left on by another program is secure",
                    answer == FieldSecurity::Secure,
                    || format!("{answer:?}"),
                );
                Run::skipped(
                    "the checks that need a normal field",
                    "secure keyboard entry was already on when the test started",
                );
            }
            (Some(token), Some(false)) => {
                let before = if window.is_some() {
                    a_plain_field_is_normal(&mut run, &platform, token, accessibility)
                } else {
                    Run::skipped(
                        "a plain text field is not secure",
                        "the test window is not in front",
                    );
                    platform.secure_state(&token, PATIENT)
                };
                secure_keyboard_entry_is_secure(&mut run, &platform, token, before);
                if window.is_some() {
                    a_password_field_is_secure(&mut run, &platform, token, before);
                } else {
                    Run::skipped(
                        "a focused password field is secure",
                        "the test window is not in front",
                    );
                }
                an_answer_comes_within_the_services_timeout(&mut run, &platform, token);
            }
        }

        if secure_input_is_on() == Some(false) {
            keys_are_stamped_with_this_process(&mut run, &platform, window.is_some());
        } else {
            Run::skipped(
                "the listener stamps a key typed into the test window with this process",
                "secure keyboard entry is on: no tap sees keys",
            );
        }
        the_watcher_reports(&mut run, &platform, window, accessibility);
        the_focus_is_given_back(&mut run, &platform, window, target);
        close_window();
        if run.failed { 1 } else { PASSED }
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
                say("FAILED: the checks did not finish in time");
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
}
