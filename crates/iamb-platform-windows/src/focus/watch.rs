//! The watcher thread: two window-event hooks and the message loop that serves them.
//!
//! The hooks are "out of context": the system loads nothing into other applications. It
//! queues each event for the thread that installed the hook and calls the procedure on that
//! thread while it retrieves messages. So everything the procedure touches lives in this
//! thread's own storage, with no lock.
//!
//! | Event | Raised | Reported when |
//! | --- | --- | --- |
//! | change of foreground | by the system, for every change of the window in front | always |
//! | focus | by the system for each control that takes the keyboard focus, and by applications that draw their own fields (browsers among them) for each of their elements | it may be about the field in front (`rules::reportable`) |
//!
//! What an event says is not used beyond its kind and its window: every report is read
//! fresh from the system when the event is handled (the window in front, its process, the
//! path of its executable). The procedure never calls into the application and never waits
//! for it: a hung application must not hold up the report that the user has moved on to
//! another one.
//!
//! The same thread reports a window again after a pause when `secure_state` asks for it
//! (`rules::Settle`).

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use iamb_platform::{FocusChange, FocusSink, PlatformError, Result};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK;

use super::os::{self, EventHook, Message};
use super::rules::{EventKind, Settle, reportable};
use super::secure::Front;

/// How often `stop` tries to post the message that ends the loop to a full queue.
const STOP_ATTEMPTS: u32 = 50;

/// The watcher's state, owned by the watcher thread.
struct Watcher {
    sink: Box<dyn FocusSink>,
    /// The window last reported.
    last: Option<Front>,
    settle: Settle,
    /// The timer that is running for `settle`, if any.
    timer: Option<usize>,
}

impl Watcher {
    /// Reports `front` as the focus.
    fn report(&mut self, front: Front) {
        self.last = Some(front);
        self.sink.focus_changed(FocusChange {
            app: os::identity_of(front),
            token: front.token(),
        });
    }

    /// Reports the window in front after something happened to it.
    fn changed(&mut self, front: Front) {
        self.cancel_timer();
        self.settle.event(front.window);
        self.report(front);
    }

    /// A window event.
    fn event(&mut self, kind: EventKind, window: u64) {
        let front = os::foreground().or_else(|| {
            // Between two windows the system may name none in front for a moment; the
            // event itself names the one that is coming.
            (kind == EventKind::Foreground)
                .then(|| os::window_of_event(window))
                .flatten()
        });
        let Some(front) = front else {
            return;
        };
        let moved = self.last != Some(front);
        if reportable(kind, moved, os::relation(window, front)) {
            self.changed(front);
        }
    }

    /// The window in front, whatever was reported before.
    fn current(&mut self) {
        if let Some(front) = os::foreground() {
            self.changed(front);
        }
    }

    /// `secure_state` could not tell the state of `window`: report it again after a pause.
    fn look_again(&mut self, window: u64) {
        if let Some(pause) = self.settle.look_again(window) {
            self.cancel_timer();
            // Without a timer the window is not reported again: its state stays unknown.
            self.timer = os::start_timer(pause);
        }
    }

    /// A timer is due.
    fn timer(&mut self, id: usize) {
        // The system's timers repeat; each of this thread's is wanted once.
        os::stop_timer(id);
        if self.timer != Some(id) {
            return;
        }
        self.timer = None;
        match os::foreground() {
            // Not a real event: the count of `settle` goes on.
            Some(front) if front.window == self.settle.window() => self.report(front),
            // The window in front changed and no event said so. Report that.
            Some(front) => self.changed(front),
            None => {}
        }
    }

    fn cancel_timer(&mut self) {
        if let Some(id) = self.timer.take() {
            os::stop_timer(id);
        }
    }
}

thread_local! {
    /// The state of the watcher that runs on this thread.
    static WATCHER: RefCell<Option<Watcher>> = const { RefCell::new(None) };
    /// An event arrived while the state was in use (the sink retrieved messages, which it
    /// must not do): the window in front has to be reported once the state is free.
    static MISSED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `work` on this thread's watcher state. Never panics: a panic of the sink ends the
/// report it was called for and nothing else.
fn with_watcher(work: impl FnOnce(&mut Watcher)) {
    let done = catch_unwind(AssertUnwindSafe(|| {
        WATCHER
            .try_with(|state| {
                let Ok(mut state) = state.try_borrow_mut() else {
                    return false;
                };
                if let Some(watcher) = state.as_mut() {
                    work(watcher);
                    // Fail closed: an event that could not be handled may have been the
                    // move into a password field.
                    while MISSED.replace(false) {
                        watcher.current();
                    }
                }
                true
            })
            .unwrap_or(true)
    }));
    if matches!(done, Ok(false)) {
        // Thread-local storage that is being torn down has nobody left to tell.
        let _ = MISSED.try_with(|missed| missed.set(true));
    }
}

/// The window-event procedure of both hooks.
extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if let Some(kind) = EventHook::kind_of(event) {
        let window = os::number(window);
        with_watcher(|watcher| watcher.event(kind, window));
    }
}

/// The watcher thread: installs the hooks, reports the focus as it is, announces itself and
/// serves the hooks until the stop message.
fn run(sink: Box<dyn FocusSink>, ready: &SyncSender<Result<u32>>) {
    os::create_message_queue();
    let hooks = EventHook::install(EventKind::Foreground, on_event).and_then(|foreground| {
        EventHook::install(EventKind::Focus, on_event).map(|focus| [foreground, focus])
    });
    let hooks = match hooks {
        Ok(hooks) => hooks,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let installed = WATCHER
        .try_with(|state| {
            *state.borrow_mut() = Some(Watcher {
                sink,
                last: None,
                settle: Settle::default(),
                timer: None,
            });
        })
        .is_ok();
    if !installed {
        let _ = ready.send(Err(PlatformError::os(
            0,
            "the watcher thread has no storage",
        )));
        return;
    }
    // The hooks are in place, so nothing can fall between this report and the first event.
    with_watcher(Watcher::current);
    if ready.send(Ok(os::current_thread_id())).is_ok() {
        loop {
            match os::next_message() {
                Message::Stop | Message::Failed(_) => break,
                Message::LookAgain(window) => with_watcher(|watcher| watcher.look_again(window)),
                Message::Timer(id) => with_watcher(|watcher| watcher.timer(id)),
                Message::Other => {}
            }
        }
    }
    with_watcher(Watcher::cancel_timer);
    // The hooks go first, so that no event finds the state gone; then the sink is dropped.
    drop(hooks);
    let _ = WATCHER.try_with(|state| state.borrow_mut().take());
}

/// A running watcher thread.
pub(super) struct Running {
    thread: JoinHandle<()>,
    thread_id: u32,
}

/// Starts the watcher thread. Returns once its hooks are installed and the focus as it is
/// has been reported to `sink`.
pub(super) fn start(sink: Box<dyn FocusSink>) -> Result<Running> {
    let (ready, announced) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("iamb-focus-watch".to_owned())
        .spawn(move || run(sink, &ready))
        .map_err(|error| os::thread_error(&error))?;
    match announced.recv() {
        Ok(Ok(thread_id)) => Ok(Running { thread, thread_id }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(PlatformError::os(
                0,
                "the watcher thread ended while it started",
            ))
        }
    }
}

impl Running {
    /// False once the thread has ended (its message loop failed).
    pub(super) fn is_running(&self) -> bool {
        !self.thread.is_finished()
    }

    /// Asks the thread to report `window` again after a pause.
    pub(super) fn look_again(&self, window: u64) {
        if self.is_running()
            && let Ok(window) = usize::try_from(window)
        {
            // If the message cannot be posted the window is not reported again: its state
            // stays unknown, which is the safe side.
            let _ = os::post_thread_message(self.thread_id, os::WM_LOOK_AGAIN, window);
        }
    }

    /// Ends the thread and waits for it, which drops the sink. On an error the thread may
    /// still run; `self` comes back so that the caller can try again.
    pub(super) fn stop(self) -> std::result::Result<(), (Self, PlatformError)> {
        let mut attempts = 0;
        // A thread that has ended has an id the system may have given to another thread.
        while self.is_running() {
            match os::post_thread_message(self.thread_id, os::WM_STOP, 0) {
                Ok(()) => break,
                // The queue is full: the thread is working it off.
                Err(_) if attempts < STOP_ATTEMPTS => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err((self, error)),
            }
        }
        // A panic is caught inside the thread; if it ends by one all the same, it has ended.
        let _ = self.thread.join();
        Ok(())
    }
}
