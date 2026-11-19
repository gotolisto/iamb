//! `SessionEvents`: screen lock, session switch and sign-out (architecture §4.5; the 2.8
//! privacy review's finding 13).
//!
//! One thread, `iamb-session-watch`, owns a hidden top-level window and a window-event hook,
//! and serves both from its message loop:
//!
//! | Signal | Reported as | Raised |
//! | --- | --- | --- |
//! | `EVENT_SYSTEM_DESKTOPSWITCH` | `Locked` | the input desktop changed: the lock screen (also on the way to another user's session, which locks first), the secure desktop of a credential prompt, Ctrl+Alt+Del. Also the switch back; a report too many only forgets a form's remembered answers |
//! | `WM_QUERYENDSESSION` | `SigningOut` | sent to every top-level window before sign-out, shutdown or restart; answered `TRUE` (Iamb never holds the session up) |
//! | `WM_ENDSESSION` with `wParam` `TRUE` | `SigningOut` | the session ends |
//!
//! The window is a top-level window that is never shown, not a message-only window: the
//! system sends `WM_QUERYENDSESSION` and `WM_ENDSESSION` to top-level windows only.
//!
//! `WM_WTSSESSION_CHANGE` would say "locked" directly, but a window receives it only after
//! `WTSRegisterSessionNotification`, which `windows-sys` declares in the Remote Desktop
//! namespace (`wtsapi32.dll`); that namespace is one the service may not link (architecture
//! §2, `crates/iamb-platform/tests/lints.rs`).
//!
//! Like the focus watcher's hooks, the hook is out of context: the system loads nothing into
//! other applications and calls the procedure on this thread while it retrieves messages, so
//! the sink lives in this thread's own storage, with no lock.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use iamb_platform::{PlatformError, Result, SessionEvent, SessionEvents, SessionSink};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EVENT_SYSTEM_DESKTOPSWITCH,
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassW,
    UnregisterClassW, WINEVENT_OUTOFCONTEXT, WM_ENDSESSION, WM_QUERYENDSESSION, WM_QUIT, WM_USER,
    WNDCLASSW, WS_OVERLAPPED,
};

use crate::Platform;
use crate::listener::os::last_error;

/// How often `stop` tries to post the message that ends the loop to a full queue.
const STOP_ATTEMPTS: u32 = 50;

/// The window class of the hidden window (NUL-terminated UTF-16).
const CLASS: &str = "IambSessionWatch";

/// The class name as the system wants it.
fn class_name() -> Vec<u16> {
    CLASS.encode_utf16().chain([0]).collect()
}

/// The running watcher, if any.
#[derive(Debug, Default)]
pub(crate) struct State(Mutex<Option<Running>>);

/// A running watcher thread.
#[derive(Debug)]
struct Running {
    thread: JoinHandle<()>,
    thread_id: u32,
}

thread_local! {
    /// The sink of the watcher that runs on this thread.
    static SINK: RefCell<Option<Box<dyn SessionSink>>> = const { RefCell::new(None) };
}

/// Hands `event` to this thread's sink. Never panics: a panic of the sink ends that report
/// and nothing else.
fn report(event: SessionEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = SINK.try_with(|sink| {
            if let Ok(mut sink) = sink.try_borrow_mut()
                && let Some(sink) = sink.as_mut()
            {
                sink.session_event(event);
            }
        });
    }));
}

/// What a window message means for the session, if anything.
fn event_of_message(message: u32, wparam: WPARAM) -> Option<SessionEvent> {
    match message {
        WM_QUERYENDSESSION => Some(SessionEvent::SigningOut),
        WM_ENDSESSION if wparam != 0 => Some(SessionEvent::SigningOut),
        _ => None,
    }
}

/// The hidden window's procedure.
extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(event) = event_of_message(message, wparam) {
        report(event);
        if message == WM_QUERYENDSESSION {
            // TRUE: the session may end; Iamb never holds it up.
            return 1;
        }
        return 0;
    }
    // SAFETY: the arguments are the ones the system passed for this window.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// The window-event procedure of the desktop-switch hook.
extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_SYSTEM_DESKTOPSWITCH {
        report(SessionEvent::Locked);
    }
}

/// The hidden window, its class and the hook; undone in reverse order when dropped.
struct Installed {
    window: HWND,
    hook: HWINEVENTHOOK,
    class: Vec<u16>,
}

impl Installed {
    fn new() -> Result<Self> {
        let class = class_name();
        // SAFETY: a null name asks for the module of the running executable.
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let spec = WNDCLASSW {
            lpfnWndProc: Some(window_procedure),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..WNDCLASSW::default()
        };
        // SAFETY: `spec` is a valid class description; its name outlives the class, which is
        // unregistered in `drop` before `class` is freed. A class registered already by an
        // earlier watcher of this process is reported as an error and used as it is.
        let atom = unsafe { RegisterClassW(&raw const spec) };
        if atom == 0 {
            // ERROR_CLASS_ALREADY_EXISTS (1410) is fine: the same procedure.
            let code = last_error();
            if code != 1410 {
                return Err(PlatformError::os(
                    code,
                    "cannot register the session window class",
                ));
            }
        }
        // SAFETY: the class was registered above; every other argument is a plain value. The
        // window is never shown (no WS_VISIBLE, no ShowWindow).
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            )
        };
        if window.is_null() {
            let code = last_error();
            // SAFETY: the class name is the one registered above.
            unsafe { UnregisterClassW(class.as_ptr(), instance) };
            return Err(PlatformError::os(code, "cannot create the session window"));
        }
        // SAFETY: an out-of-context hook with a procedure of the required signature; no
        // module is loaded into other processes.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_DESKTOPSWITCH,
                EVENT_SYSTEM_DESKTOPSWITCH,
                ptr::null_mut(),
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_null() {
            // SAFETY: the window was created above, on this thread.
            unsafe { DestroyWindow(window) };
            // SAFETY: the class name is the one registered above.
            unsafe { UnregisterClassW(class.as_ptr(), instance) };
            return Err(PlatformError::os(
                0,
                "cannot install the desktop-switch hook",
            ));
        }
        Ok(Self {
            window,
            hook,
            class,
        })
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by this value and is removed once.
        unsafe { UnhookWinEvent(self.hook) };
        // SAFETY: the window was created by this value, on this thread, and is destroyed once.
        unsafe { DestroyWindow(self.window) };
        // SAFETY: a null name asks for the module of the running executable; the class name is
        // the one registered.
        unsafe { UnregisterClassW(self.class.as_ptr(), GetModuleHandleW(ptr::null())) };
    }
}

/// The watcher thread: creates its message queue, window and hook, announces itself and serves
/// them until `WM_QUIT`.
fn run(sink: Box<dyn SessionSink>, ready: &SyncSender<Result<u32>>) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid place for a message; the call creates this thread's queue so
    // that the stop message can be posted to it from now on.
    unsafe { PeekMessageW(&raw mut msg, ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
    let installed = match Installed::new() {
        Ok(installed) => installed,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if SINK
        .try_with(|slot| *slot.borrow_mut() = Some(sink))
        .is_err()
    {
        let _ = ready.send(Err(PlatformError::os(
            0,
            "the session thread has no storage",
        )));
        return;
    }
    // SAFETY: the call takes no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_ok() {
        loop {
            // SAFETY: `msg` is a valid place for a message; every window of the thread.
            let got = unsafe { GetMessageW(&raw mut msg, ptr::null_mut(), 0, 0) };
            // 0: WM_QUIT (the stop message); -1: the call failed.
            if got == 0 || got == -1 {
                break;
            }
            // SAFETY: a message retrieved just above; it goes to this thread's window or to
            // the hook's procedure.
            unsafe { DispatchMessageW(&raw const msg) };
        }
    }
    // The hook and the window go first, so that no event finds the sink gone.
    drop(installed);
    let _ = SINK.try_with(|slot| slot.borrow_mut().take());
}

impl State {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SessionEvents for Platform {
    fn watch_session(&self, sink: Box<dyn SessionSink>) -> Result<()> {
        let mut slot = self.session.lock();
        if slot.as_ref().is_some_and(|r| !r.thread.is_finished()) {
            return Err(PlatformError::AlreadyRunning);
        }
        let (ready, announced) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("iamb-session-watch".to_owned())
            .spawn(move || run(sink, &ready))
            .map_err(|_| PlatformError::os(0, "cannot start the session thread"))?;
        match announced.recv() {
            Ok(Ok(thread_id)) => {
                *slot = Some(Running { thread, thread_id });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(PlatformError::os(
                    0,
                    "the session thread ended while it started",
                ))
            }
        }
    }

    fn unwatch_session(&self) -> Result<()> {
        let Some(running) = self.session.lock().take() else {
            return Ok(());
        };
        let mut attempts = 0;
        while !running.thread.is_finished() {
            // SAFETY: plain values; a thread id that names no thread with a queue is refused.
            if unsafe { PostThreadMessageW(running.thread_id, WM_QUIT, 0, 0) } != 0 {
                break;
            }
            if attempts >= STOP_ATTEMPTS {
                let code = last_error();
                *self.session.lock() = Some(running);
                return Err(PlatformError::os(code, "cannot stop the session thread"));
            }
            attempts += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = running.thread.join();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, SendMessageW};

    use super::*;

    #[test]
    fn messages_that_end_the_session_are_sign_outs() {
        assert_eq!(
            event_of_message(WM_QUERYENDSESSION, 0),
            Some(SessionEvent::SigningOut)
        );
        assert_eq!(
            event_of_message(WM_ENDSESSION, 1),
            Some(SessionEvent::SigningOut)
        );
        // An end of session that was cancelled ends nothing.
        assert_eq!(event_of_message(WM_ENDSESSION, 0), None);
        assert_eq!(event_of_message(WM_USER, 0), None);
    }

    #[test]
    fn the_hidden_window_reports_a_sign_out_and_stops() {
        let platform = Platform::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        platform
            .watch_session(Box::new(move |e: SessionEvent| {
                log.lock().expect("not poisoned").push(e);
            }))
            .expect("watches");
        assert!(matches!(
            platform.watch_session(Box::new(|_: SessionEvent| {})),
            Err(PlatformError::AlreadyRunning)
        ));
        let class = class_name();
        // SAFETY: a class name registered by the watcher; no parent, the first match.
        let window = unsafe {
            FindWindowExW(
                ptr::null_mut(),
                ptr::null_mut(),
                class.as_ptr(),
                ptr::null(),
            )
        };
        assert!(!window.is_null(), "the hidden window exists");
        // SAFETY: a window of this process; the message carries no pointers.
        let answer = unsafe { SendMessageW(window, WM_QUERYENDSESSION, 0, 0) };
        assert_eq!(answer, 1, "the session may end");
        platform.unwatch_session().expect("stops");
        platform.unwatch_session().expect("stopping twice is fine");
        assert_eq!(
            *seen.lock().expect("not poisoned"),
            [SessionEvent::SigningOut]
        );
        // SAFETY: as above.
        let gone = unsafe {
            FindWindowExW(
                ptr::null_mut(),
                ptr::null_mut(),
                class.as_ptr(),
                ptr::null(),
            )
        };
        assert!(gone.is_null(), "the window went with the thread");
    }
}
