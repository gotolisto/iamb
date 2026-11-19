//! The probe window of the end-to-end latency measurement (`docs/test-strategy.md` §6), behind
//! the `probe` feature. Test tooling for `iamb-probe` (in `iamb-testkit`): the feature is off by
//! default and no shipped binary enables it.
//!
//! One window whose content is one `NSTextView` (plain text, every automatic substitution and
//! correction off, so the text is exactly what arrived). The view is a subclass that reports
//! two kinds of [`Event`] to the sink it was opened with, each stamped with [`Instant::now`]
//! (`clock_gettime(CLOCK_UPTIME_RAW)`, the clock of `mach_absolute_time`) before anything else
//! is done:
//!
//! - [`Event::Key`]: `keyDown:` reached the view; one event per character of the key event,
//!   reported before the view handles the key.
//! - [`Event::Changed`]: `didChangeText`, which the view calls after every edit the user (or a
//!   key sender, or a paste) makes; the whole text is read after the stamp. Setting the text
//!   from code ([`ProbeWindow::reset`]) does not call it.
//!
//! The window lives on the main thread, which must run the application event loop: call
//! [`ProbeWindow::open`] from the work closure of
//! [`run_with_main_loop`](crate::run_with_main_loop). The window keeps no text of its own,
//! writes nothing and logs nothing. Which key is the last of an abbreviation and which change
//! completes the expansion is decided by the caller
//! (`iamb_testkit::compat::probe::session`).

#![allow(
    clippy::missing_errors_doc,
    reason = "test tooling: errors are readable strings for the person running the probe"
)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent,
    NSEventModifierFlags, NSEventType, NSResponder, NSText, NSTextView, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

use crate::listener::main_thread;

/// What the probe window reports. Both stamps are [`Instant`]s (`CLOCK_UPTIME_RAW`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// The view received a key that produced `ch`.
    Key {
        /// When `keyDown:` reached the view.
        at: Instant,
        /// The character.
        ch: char,
    },
    /// The view's text changed.
    Changed {
        /// When `didChangeText` was called (before the text was read).
        at: Instant,
        /// The view's whole text after the change.
        text: &'a str,
    },
}

/// Where the window sends its events. Called on the main thread; it should return quickly. A
/// panic in it is caught and the event dropped.
pub type Sink = Arc<dyn Fn(Event<'_>) + Send + Sync>;

/// How long a call waits for the main thread.
const MAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long [`ProbeWindow::focus`] keeps trying.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(3);

fn report(sink: &Sink, event: Event<'_>) {
    // A panic must not unwind into the framework's frames.
    let _ = catch_unwind(AssertUnwindSafe(|| sink(event)));
}

define_class!(
    // SAFETY: `NSTextView` may be subclassed; the two overrides call the superclass's
    // implementation with the same arguments, the instance variable is a plain Rust value
    // with no requirement on the superclass, and the class does not implement `Drop`. All
    // `NSView`s are main-thread only.
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Sink]
    struct ProbeTextView;

    impl ProbeTextView {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let at = Instant::now();
            if let Some(chars) = event.characters() {
                for ch in chars.to_string().chars() {
                    report(self.ivars(), Event::Key { at, ch });
                }
            }
            // SAFETY: `NSTextView` implements `keyDown:`, which takes one event and returns
            // nothing.
            let () = unsafe { msg_send![super(self), keyDown: event] };
        }

        #[unsafe(method(didChangeText))]
        fn did_change_text(&self) {
            let at = Instant::now();
            let text = self.string().to_string();
            report(self.ivars(), Event::Changed { at, text: &text });
            // SAFETY: `NSTextView` implements `didChangeText`, which takes no argument and
            // returns nothing.
            let () = unsafe { msg_send![super(self), didChangeText] };
        }
    }
);

impl ProbeTextView {
    fn new(mtm: MainThreadMarker, frame: CGRect, sink: Sink) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(sink);
        // SAFETY: `initWithFrame:` is the designated initialiser of `NSTextView`; it takes a
        // rectangle and returns the initialised view.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

struct Ui {
    window: Retained<NSWindow>,
    view: Retained<ProbeTextView>,
}

thread_local! {
    /// The window and its view. Only the main thread ever touches it.
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

const fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

fn open_on_main(mtm: MainThreadMarker, title: &str, sink: Sink) -> Result<isize, String> {
    if UI.with(|ui| ui.borrow().is_some()) {
        return Err("a probe window is already open in this process".to_owned());
    }
    let app = NSApplication::sharedApplication(mtm);
    // An ordinary application with a Dock icon, so that it can be activated like the apps of
    // the matrix; `run_with_main_loop` starts it as an accessory.
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let frame = rect(160.0, 160.0, 640.0, 400.0);
    // SAFETY: a new window from a valid frame, style and backing type, created on the main
    // thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is owned by the `Retained` kept in `UI`; it must not also release
    // itself when it is closed.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(title));
    let view = ProbeTextView::new(mtm, rect(0.0, 0.0, 640.0, 400.0), sink);
    view.setRichText(false);
    view.setAllowsUndo(false);
    view.setAutomaticQuoteSubstitutionEnabled(false);
    view.setAutomaticDashSubstitutionEnabled(false);
    view.setAutomaticTextReplacementEnabled(false);
    view.setAutomaticSpellingCorrectionEnabled(false);
    view.setAutomaticLinkDetectionEnabled(false);
    view.setAutomaticDataDetectionEnabled(false);
    view.setAutomaticTextCompletionEnabled(false);
    view.setContinuousSpellCheckingEnabled(false);
    view.setGrammarCheckingEnabled(false);
    view.setSmartInsertDeleteEnabled(false);
    let content: &NSView = &view;
    window.setContentView(Some(content));
    window.makeKeyAndOrderFront(None);
    // The newer `activate` only asks; a probe started from a terminal has to take the front.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let responder: &NSResponder = &view;
    let _ = window.makeFirstResponder(Some(responder));
    let number = window.windowNumber();
    UI.with(|ui| *ui.borrow_mut() = Some(Ui { window, view }));
    Ok(number)
}

/// Whether the window is the key window of the active application and the view its first
/// responder. Main thread.
fn focused_on_main(mtm: MainThreadMarker) -> bool {
    let app = NSApplication::sharedApplication(mtm);
    UI.with(|ui| {
        ui.borrow().as_ref().is_some_and(|ui| {
            let first = ui.window.firstResponder();
            let view: &NSResponder = &ui.view;
            app.isActive()
                && ui.window.isKeyWindow()
                && first.is_some_and(|f| std::ptr::eq(&raw const *f, view))
        })
    })
}

fn take_focus_on_main(mtm: MainThreadMarker) -> bool {
    let app = NSApplication::sharedApplication(mtm);
    UI.with(|ui| {
        if let Some(ui) = ui.borrow().as_ref() {
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            ui.window.makeKeyAndOrderFront(None);
            let responder: &NSResponder = &ui.view;
            let _ = ui.window.makeFirstResponder(Some(responder));
        }
    });
    focused_on_main(mtm)
}

/// The probe window: one `NSTextView` in a window, on the main thread. Dropping it closes the
/// window.
#[derive(Debug)]
pub struct ProbeWindow {
    number: isize,
}

impl ProbeWindow {
    /// Opens the window with `title`; its events go to `sink`. Call it from the work closure
    /// of [`run_with_main_loop`](crate::run_with_main_loop): it fails when the main thread does
    /// not answer, and when a probe window is already open in this process.
    pub fn open(title: &str, sink: Sink) -> Result<Self, String> {
        let title = title.to_owned();
        main_thread::call(MAIN_TIMEOUT, move |mtm| open_on_main(mtm, &title, sink))
            .ok_or_else(|| {
                "the main thread did not open the window (run the probe under run_with_main_loop)"
                    .to_owned()
            })?
            .map(|number| Self { number })
    }

    /// The window's number (`windowNumber`), which key events posted to it name.
    #[must_use]
    pub const fn window_number(&self) -> isize {
        self.number
    }

    /// Empties the view (from code, so no change is reported) and gives it the keyboard focus.
    /// `Ok(false)` when the window could not become the key window of the active application.
    pub fn reset(&self) -> Result<bool, String> {
        main_thread::call(MAIN_TIMEOUT, |_mtm| {
            UI.with(|ui| {
                if let Some(ui) = ui.borrow().as_ref() {
                    ui.view.setString(&NSString::from_str(""));
                }
            });
        })
        .ok_or_else(|| "the main thread did not answer".to_owned())?;
        Ok(self.focus())
    }

    /// True when the window is the key window of the active application and the view has the
    /// keyboard focus.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        main_thread::call(MAIN_TIMEOUT, focused_on_main).unwrap_or(false)
    }

    /// Activates the application, brings the window to the front and makes the view its first
    /// responder, trying for up to three seconds. False when the system does not allow it.
    #[must_use]
    pub fn focus(&self) -> bool {
        let deadline = Instant::now() + FOCUS_TIMEOUT;
        loop {
            if main_thread::call(MAIN_TIMEOUT, take_focus_on_main).unwrap_or(false) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Posts `text` as key-down and key-up events, one pair per character, to this
    /// application's own event queue, addressed to the probe window. They reach the view
    /// through the same `keyDown:` as keys from the system, but never pass the system's event
    /// tap, so Iamb's listener does not see them and no permission is needed. For the probe's
    /// self-test and the crate's tests; a latency run types through the driver instead.
    pub fn type_synthetic(&self, text: &str) -> Result<(), String> {
        let number = self.number;
        let text = text.to_owned();
        main_thread::call(MAIN_TIMEOUT, move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            for ch in text.chars() {
                let chars = NSString::from_str(&ch.to_string());
                for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
                    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                        kind,
                        CGPoint::ZERO,
                        NSEventModifierFlags::empty(),
                        0.0,
                        number,
                        None,
                        &chars,
                        &chars,
                        false,
                        0,
                    )
                    .ok_or("the system did not make a key event")?;
                    app.postEvent_atStart(&event, false);
                }
            }
            Ok(())
        })
        .ok_or_else(|| "the main thread did not answer".to_owned())?
    }
}

impl Drop for ProbeWindow {
    fn drop(&mut self) {
        let _ = main_thread::call(MAIN_TIMEOUT, |_mtm| {
            UI.with(|ui| {
                if let Some(ui) = ui.borrow_mut().take() {
                    ui.window.orderOut(None);
                }
            });
        });
    }
}
