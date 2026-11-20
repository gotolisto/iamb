//! `make_non_activating_panel` against the real `AppKit`, with a real main loop and real
//! windows (a test with its own `main`: the standard harness keeps the main thread for
//! itself, and windows live there). Built only with the `panel` feature.
//!
//! The main thread runs the application event loop (`run_with_main_loop`, an accessory app as
//! the UI host is); the checks run on the work thread and hand the main thread what must
//! happen there through a run-loop source.
//!
//! The window converted is an `NSWindow` subclass with an instance variable that decides
//! `canBecomeKeyWindow`, borderless and hidden: what Tauri creates for a panel (its window
//! class keeps a `focusable` flag the same way).
//!
//! | Check | Needs | Without it |
//! | --- | --- | --- |
//! | off the main thread the call is refused | nothing | |
//! | the window becomes an `IambNonActivatingPanel` (an `NSPanel`) with the non-activating style, `becomesKeyOnlyIfNeeded` off, floating, `hidesOnDeactivate` off, on every Space and over full-screen apps | nothing | |
//! | the original class's instance variable still reads and decides `canBecomeKeyWindow`; the panel never becomes main | nothing | |
//! | a second call changes nothing; a window of another class is refused and keeps its class | nothing | |
//! | before the panel is shown, the launch activation has been served and another app is the active one, as when the UI host shows a panel (see `make_another_app_active`) | another app that can be active (Finder is) | failed |
//! | shown, the panel becomes the key window and the app does not become active; while its flag says not focusable it does not become key | a window server session | skipped |
//!
//! A skipped check writes `SKIPPED` and its reason to standard error; a failed one writes
//! `FAILED` and the process exits with a failure. Nothing is typed.

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::io::Write;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::process::ExitCode;
    use std::ptr::{self, NonNull};
    use std::sync::mpsc::channel;
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use iamb_platform::PlatformError;
    use iamb_platform_macos::{make_non_activating_panel, run_with_main_loop};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    };
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSPanel, NSResponder, NSView, NSWindow,
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_core_foundation::{
        CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CGPoint, CGRect, CGSize,
        kCFRunLoopCommonModes,
    };
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };

    /// What the closure returns when no check failed.
    const PASSED: i32 = 42;
    /// How long the whole test may take before the watchdog fails it.
    const DEADLINE: Duration = Duration::from_mins(1);
    /// How long the main thread gets for one job.
    const MAIN_TIMEOUT: Duration = Duration::from_secs(5);
    /// How long the panel gets to become key (or to show that it does not).
    const SETTLE: Duration = Duration::from_secs(3);
    /// How long the process must stay inactive before the panel is shown.
    const STAY: Duration = Duration::from_millis(500);

    fn say(line: &str) {
        let _ = writeln!(std::io::stderr(), "panel: {line}");
    }

    #[derive(Default)]
    struct Run {
        failed: bool,
    }

    impl Run {
        fn check(&mut self, what: &str, passed: bool) {
            if passed {
                say(&format!("ok: {what}"));
            } else {
                say(&format!("FAILED: {what}"));
                self.failed = true;
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

    // ---- The windows, on the main thread --------------------------------------------------

    define_class!(
        // SAFETY: `NSWindow` may be subclassed; the override has the signature `AppKit`
        // declares, the instance variable is a plain Rust value with no requirement on the
        // superclass, and the class does not implement `Drop`. Windows are main-thread only.
        #[unsafe(super(NSWindow, NSResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "IambPanelTestWindow"]
        #[ivars = Cell<bool>]
        struct TestWindow;

        impl TestWindow {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key_window(&self) -> bool {
                self.ivars().get()
            }
        }
    );

    impl TestWindow {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(Cell::new(true));
            // SAFETY: the designated initialiser of `NSWindow`, with a valid frame, style and
            // backing type, on the main thread.
            let window: Retained<Self> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: rect(240.0, 240.0, 320.0, 160.0),
                    styleMask: NSWindowStyleMask::Borderless,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            // SAFETY: the window is owned by the `Retained` kept in `UI`; it must not also
            // release itself when it is closed.
            unsafe { window.setReleasedWhenClosed(false) };
            window
        }
    }

    /// A window as `raw-window-handle` hands it over: its content view.
    struct Handle<'a>(&'a NSView);

    impl HasWindowHandle for Handle<'_> {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let view = NonNull::from(self.0).cast::<c_void>();
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(view));
            // SAFETY: the view is borrowed for as long as the handle and stays in its window
            // (the test closes no window while a handle exists).
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    struct Ui {
        panel: Retained<TestWindow>,
        plain: Retained<NSWindow>,
    }

    thread_local! {
        /// The two windows. Only the main thread ever touches it.
        static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    }

    const fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }

    fn convert(window: &NSWindow) -> Result<(), PlatformError> {
        match window.contentView() {
            Some(view) => make_non_activating_panel(&Handle(&view)),
            None => Err(PlatformError::Invalid {
                context: "the test window has no content view",
            }),
        }
    }

    fn class_name(window: &NSWindow) -> String {
        let object: &AnyObject = window;
        object.class().name().to_string_lossy().into_owned()
    }

    /// What the conversion did, read on the main thread: the conversion's error, if any, and
    /// each check with its outcome.
    type Converted = (Option<String>, Vec<(&'static str, bool)>);

    fn create_and_convert(mtm: MainThreadMarker) -> Converted {
        let panel = TestWindow::new(mtm);
        let result = convert(&panel);
        let class = class_name(&panel);
        let as_panel = panel.downcast_ref::<NSPanel>();
        let behaviour = panel.collectionBehavior().contains(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        // The original class's instance variable, read and written through its own type as
        // Tauri does, still decides whether the panel may become key.
        let yes = panel.canBecomeKeyWindow() && panel.ivars().get();
        panel.ivars().set(false);
        let no = !panel.canBecomeKeyWindow();
        panel.ivars().set(true);
        let again = convert(&panel);
        // SAFETY: a new window from a valid frame, style and backing type, created on the
        // main thread.
        let plain = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect(260.0, 260.0, 320.0, 160.0),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: as for the test window: owned by `UI`.
        unsafe { plain.setReleasedWhenClosed(false) };
        let other = convert(&plain);
        let checks = vec![
            ("the conversion succeeds", result.is_ok()),
            (
                "the window's class is IambNonActivatingPanel, an NSPanel",
                class == "IambNonActivatingPanel" && as_panel.is_some(),
            ),
            (
                "the style mask has NonactivatingPanel",
                panel
                    .styleMask()
                    .contains(NSWindowStyleMask::NonactivatingPanel),
            ),
            (
                "becomesKeyOnlyIfNeeded is off",
                as_panel.is_some_and(|p| !p.becomesKeyOnlyIfNeeded()),
            ),
            (
                "the panel floats",
                as_panel.is_some_and(NSPanel::isFloatingPanel),
            ),
            ("hidesOnDeactivate is off", !panel.hidesOnDeactivate()),
            (
                "the panel joins every Space and goes over full-screen apps",
                behaviour,
            ),
            (
                "the original class's flag still decides canBecomeKeyWindow",
                yes && no,
            ),
            (
                "the panel never becomes the main window",
                !panel.canBecomeMainWindow(),
            ),
            (
                "a second call succeeds and changes nothing",
                again.is_ok() && class_name(&panel) == class,
            ),
            (
                "a window of another class is refused and keeps its class",
                matches!(other, Err(PlatformError::Invalid { .. }))
                    && class_name(&plain) == "NSWindow",
            ),
        ];
        UI.with(|ui| *ui.borrow_mut() = Some(Ui { panel, plain }));
        (result.err().map(|e| e.to_string()), checks)
    }

    /// Orders the panel out, sets its flag, and shows it as Tauri's `show` does.
    fn show_panel(focusable: bool) -> bool {
        on_main(move |_mtm| {
            UI.with(|ui| {
                ui.borrow().as_ref().is_some_and(|ui| {
                    ui.panel.orderOut(None);
                    ui.panel.ivars().set(focusable);
                    ui.panel.makeKeyAndOrderFront(None);
                    true
                })
            })
        })
        .unwrap_or(false)
    }

    /// (the panel is the key window, the app is active).
    fn state() -> Option<(bool, bool)> {
        on_main(|mtm| {
            let active = NSApplication::sharedApplication(mtm).isActive();
            UI.with(|ui| {
                ui.borrow()
                    .as_ref()
                    .map(|ui| (ui.panel.isKeyWindow(), active))
            })
        })
        .flatten()
    }

    /// Orders both windows out, closes them and releases them, inside a pool of its own so
    /// that whatever the release autoreleases is drained here, in this job, and not later by the
    /// event loop as it stops.
    fn close_windows() {
        let closed = on_main(|_mtm| {
            objc2::rc::autoreleasepool(|_| {
                UI.with(|ui| {
                    if let Some(ui) = ui.borrow_mut().take() {
                        ui.panel.orderOut(None);
                        ui.plain.orderOut(None);
                        ui.panel.close();
                        ui.plain.close();
                    }
                });
            });
        });
        say(if closed.is_some() {
            "the windows were closed and released"
        } else {
            "the main thread did not close the windows"
        });
    }

    // ---- The checks, on the work thread ---------------------------------------------------

    fn the_window_becomes_a_panel(run: &mut Run) {
        let off_main = matches!(
            make_non_activating_panel(&Unreachable),
            Err(PlatformError::Invalid { .. })
        );
        run.check("off the main thread the call is refused", off_main);
        let Some((error, checks)) = on_main(create_and_convert) else {
            run.check("the main thread converts the window", false);
            return;
        };
        if let Some(e) = error {
            say(&format!("the conversion failed: {e}"));
        }
        for (what, passed) in checks {
            run.check(what, passed);
        }
    }

    /// Waits for `wanted` (key, active) states; returns the last one seen and whether the app
    /// was active at any time.
    fn settle(wanted: impl Fn((bool, bool)) -> bool) -> (Option<(bool, bool)>, bool) {
        let start = Instant::now();
        let mut last = None;
        let mut was_active = false;
        while start.elapsed() < SETTLE {
            last = state();
            was_active |= last.is_some_and(|(_, active)| active);
            if last.is_some_and(&wanted) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        (last, was_active)
    }

    /// Puts the test process where the UI host is when a panel is asked for: running for a
    /// while, and not the active app (the user is typing into another one).
    ///
    /// A process that starts the application event loop asks to be activated:
    /// `-[NSApplication run]` calls `finishLaunching`, which "activates the app" (Apple's
    /// documentation of `finishLaunching`). The request is served asynchronously, so a process
    /// that is inactive just after start can become active a moment later, whatever window it
    /// shows; the first run of this test on a CI runner saw exactly that. The UI host does the
    /// same at login (and Tauri asks to be activated at launch too), long before any panel is
    /// shown, and the user has made another app active since. So the test waits for the launch
    /// activation, hands the active state to another app (`deactivate`, then `hide:` and
    /// `unhideWithoutActivation` if that was not enough), and checks that it stays inactive
    /// before the panel is shown. `false` (a failure) when the process cannot be made inactive.
    fn make_another_app_active(run: &mut Run) -> bool {
        let active = || on_main(|mtm| NSApplication::sharedApplication(mtm).isActive());
        let launched_active = wait_until(SETTLE, || active() == Some(true));
        say(&format!(
            "the system {} this process at launch",
            if launched_active {
                "activated"
            } else {
                "did not activate"
            }
        ));
        let _ = on_main(|mtm| NSApplication::sharedApplication(mtm).deactivate());
        if !wait_until(SETTLE, || active() == Some(false)) {
            let _ = on_main(|mtm| NSApplication::sharedApplication(mtm).hide(None));
            let _ = wait_until(SETTLE, || active() == Some(false));
            let _ = on_main(|mtm| NSApplication::sharedApplication(mtm).unhideWithoutActivation());
        }
        // It must stay inactive for a while: a launch activation served late would show here.
        let start = Instant::now();
        let mut inactive = true;
        while start.elapsed() < STAY {
            inactive &= active() == Some(false);
            thread::sleep(Duration::from_millis(50));
        }
        run.check(
            "before the panel is shown, another app is the active one",
            inactive,
        );
        inactive
    }

    /// Polls `done` until it is true, at most `timeout`; whether it became true.
    fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            if done() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn the_panel_takes_the_keyboard_without_activating(run: &mut Run) {
        let what = "shown, the panel becomes key and the app stays inactive";
        if state().is_none() {
            run.check("the panel exists", false);
            return;
        }
        if !make_another_app_active(run) {
            return;
        }
        if !show_panel(true) {
            run.check("the panel can be shown", false);
            return;
        }
        let (last, was_active) = settle(|(key, _)| key);
        if was_active {
            run.check(&format!("{what} (Iamb became the active app)"), false);
            return;
        }
        if last.is_some_and(|(key, _)| key) {
            run.check(what, true);
        } else {
            say(&format!(
                "SKIPPED {what}: the panel did not become key (no window server session?)"
            ));
            return;
        }
        let what = "while its flag says not focusable, the panel does not become key";
        if !show_panel(false) {
            run.check("the panel can be shown again", false);
            return;
        }
        let (last, was_active) = settle(|(key, _)| key);
        run.check(what, last.is_some_and(|(key, _)| !key) && !was_active);
    }

    /// A handle that is never asked for on the main thread: the call refuses before.
    struct Unreachable;

    impl HasWindowHandle for Unreachable {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            Err(HandleError::Unavailable)
        }
    }

    fn checks() -> i32 {
        let mut run = Run::default();
        the_window_becomes_a_panel(&mut run);
        the_panel_takes_the_keyboard_without_activating(&mut run);
        close_windows();
        if run.failed { 1 } else { PASSED }
    }

    pub(super) fn main() -> ExitCode {
        if MainThreadMarker::new().is_none() {
            say("FAILED: the test's own `main` is not on the main thread");
            return ExitCode::FAILURE;
        }
        // Registered here, so that the class exists before any check reads its name.
        let _ = TestWindow::class();
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
