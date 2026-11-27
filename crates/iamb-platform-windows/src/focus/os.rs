//! The system calls of the focus module, each behind a small safe function.
//!
//! Apart from UI Automation (`uia.rs`), every `unsafe` block of the focus module is here.
//! None of these calls waits for another application: they read what the system itself
//! keeps about windows and processes (handles, process ids, the executable's path, window
//! classes and styles, privilege levels) or ask the system to do something it does without
//! waiting. None reads a window's title or text.

use std::ptr;
use std::sync::OnceLock;

use iamb_platform::{AppIdentity, PlatformError, Result};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE, HWND, LPARAM,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND,
    EnumChildWindows, FindWindowExW, GA_ROOTOWNER, GUITHREADINFO, GWL_STYLE, GetAncestor,
    GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowLongW,
    GetWindowThreadProcessId, IsIconic, IsWindow, KillTimer, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, RealGetWindowClassW, SW_RESTORE, SetForegroundWindow, SetTimer,
    ShowWindowAsync, WINEVENT_OUTOFCONTEXT, WM_APP, WM_QUIT, WM_TIMER, WM_USER,
};
use windows_sys::core::BOOL;

use super::classify;
use super::identity;
use super::rules::{EventKind, Relation};
use super::secure::{Control, Desk, Front, Privilege};
use crate::listener::os::{last_error, os_error};

/// Thread message to the watcher: the state of the window whose handle is the message's
/// first parameter could not be told; report it again after a pause.
pub(super) const WM_LOOK_AGAIN: u32 = WM_APP + 1;
/// Thread message to the watcher: the late answer about the window whose handle is the
/// message's first parameter has arrived; report it again now.
pub(super) const WM_LATE_ANSWER: u32 = WM_APP + 2;
/// Ends the watcher thread's message loop.
pub(super) const WM_STOP: u32 = WM_QUIT;

/// The window class of the frame the system puts around an application that runs in an
/// application container, and of the application's own window inside it.
const FRAME_CLASS: &str = "ApplicationFrameWindow";
const CORE_CLASS: &str = "Windows.UI.Core.CoreWindow";

/// The signature of a window-event procedure.
pub(super) type EventProcedure =
    unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32);

/// A window handle from its number.
fn handle(window: u64) -> HWND {
    // Iamb supports 64-bit Windows only (`injector/os.rs` asserts it); a number that does
    // not fit is no window.
    usize::try_from(window).unwrap_or(0) as HWND
}

/// A window handle as a number.
pub(super) fn number(window: HWND) -> u64 {
    window as usize as u64
}

/// The thread and the process that own `window`; `None` for a handle that names no window.
fn owner(window: HWND) -> Option<(u32, u32)> {
    if window.is_null() {
        return None;
    }
    let mut pid = 0;
    // SAFETY: a stale handle is answered with 0; `pid` is a valid place for the process id.
    let thread = unsafe { GetWindowThreadProcessId(window, &raw mut pid) };
    (thread != 0).then_some((thread, pid))
}

/// `window` with its process, when it is a window.
fn front_of(window: HWND) -> Option<Front> {
    owner(window).map(|(_, pid)| Front {
        window: number(window),
        pid,
    })
}

/// The window in front and its process.
pub(super) fn foreground() -> Option<Front> {
    // SAFETY: the call takes no arguments.
    front_of(unsafe { GetForegroundWindow() })
}

/// Where keys go now, for the injector's focus guard: the window in front, its process, and
/// the control with the keyboard focus in that window's thread (0 when the system names
/// none: a window that draws its own fields, a browser engine). `None` when no window is in
/// front, as while the foreground passes from one window to another.
///
/// Three reads of what the system keeps about windows and input; none sends the window a
/// message or waits for its thread, so a hung application cannot hold the caller.
pub(crate) fn keyboard_focus() -> Option<(u64, u32, u64)> {
    // SAFETY: the call takes no arguments.
    let window = unsafe { GetForegroundWindow() };
    let (_, pid) = owner(window)?;
    let control = focused_control(window).map_or(0, number);
    Some((number(window), pid, control))
}

/// The window an event names, as the window in front. Used when the system names no
/// foreground window at the moment a foreground event is handled.
pub(super) fn window_of_event(window: u64) -> Option<Front> {
    front_of(handle(window))
}

/// The class name `window` was registered under, as the window system keeps it (`None` for a
/// handle that names no window). The UI host asks it of the window in front to tell the
/// shell's taskbar and notification-area windows from an application's (`iamb-desktop`'s tray
/// menu). One read of what the system keeps; no message is sent to the window.
#[must_use]
pub fn window_class(window: u64) -> Option<String> {
    let window = handle(window);
    owner(window)?;
    let mut units = [0_u16; 256];
    // SAFETY: the buffer holds the number of units passed. The call reads what the system
    // keeps about the window and sends it no message.
    let count = unsafe { GetClassNameW(window, units.as_mut_ptr(), 256) };
    let count = usize::try_from(count).unwrap_or(0).min(units.len());
    (count > 0).then(|| String::from_utf16_lossy(&units[..count]))
}

/// The class `window` is built on: for a control that a toolkit registered under its own
/// name on top of a system class, the system class.
fn real_class(window: HWND) -> String {
    let mut units = [0_u16; 128];
    // SAFETY: the buffer holds the number of units passed. The call reads what the system
    // keeps about the window and sends it no message.
    let count = unsafe { RealGetWindowClassW(window, units.as_mut_ptr(), 128) };
    let count = usize::try_from(count).unwrap_or(0).min(units.len());
    String::from_utf16_lossy(&units[..count])
}

/// The control with the keyboard focus in the thread of `window`, if the system names one.
fn focused_control(window: HWND) -> Option<HWND> {
    let (thread, _) = owner(window)?;
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
        ..GUITHREADINFO::default()
    };
    // SAFETY: `info` is a valid structure with its size set. The call reads the thread's
    // input state from the system and does not wait for the thread.
    if unsafe { GetGUIThreadInfo(thread, &raw mut info) } == 0 || info.hwndFocus.is_null() {
        return None;
    }
    Some(info.hwndFocus)
}

/// The calling thread's last error code.
fn error_code() -> u32 {
    // SAFETY: the call takes no arguments and reads a value of the calling thread.
    unsafe { GetLastError() }
}

/// An open handle; closed when dropped.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Opens process `pid` for the questions any process may ask about another.
///
/// # Errors
///
/// The system's error code.
fn open_process(pid: u32) -> std::result::Result<Handle, u32> {
    // SAFETY: plain values; the handle is not inherited.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        Err(error_code())
    } else {
        Ok(Handle(process))
    }
}

/// The path of the executable of `process`.
fn image_path(process: HANDLE) -> Option<String> {
    // Long enough for nearly every path; the second size is the longest path there is.
    for size in [1024_usize, 32 * 1024] {
        let mut units = vec![0_u16; size];
        let mut len = u32::try_from(size).unwrap_or(0);
        // SAFETY: `units` holds `len` units, and `len` is a valid place for the length
        // written.
        let ok = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                units.as_mut_ptr(),
                &raw mut len,
            )
        };
        if ok != 0 {
            units.truncate(usize::try_from(len).unwrap_or(0));
            return Some(String::from_utf16_lossy(&units));
        }
        if error_code() != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
    }
    None
}

/// The path of the running executable, in the form [`image_path`] gives every other one.
fn own_image() -> Option<&'static str> {
    static OWN: OnceLock<Option<String>> = OnceLock::new();
    OWN.get_or_init(|| {
        // SAFETY: the call takes no arguments; the pseudo handle needs no closing.
        image_path(unsafe { GetCurrentProcess() })
    })
    .as_deref()
}

/// The integrity level of `process`: the last part of the label in its token.
///
/// # Errors
///
/// The system's error code.
fn integrity(process: HANDLE) -> std::result::Result<u32, u32> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is an open process handle with query rights; `token` is a valid
    // place for the result.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(error_code());
    }
    let token = Handle(token);
    // A label is the structure and one short identifier; 128 bytes hold any. `u64` aligns
    // the buffer for the structure.
    let mut buffer = [0_u64; 16];
    let mut len = 0;
    // SAFETY: `buffer` is writable for the size passed and aligned for the structure; `len`
    // is a valid place for the length written.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&buffer)).unwrap_or(0),
            &raw mut len,
        )
    };
    if ok == 0 {
        return Err(error_code());
    }
    // SAFETY: on success the buffer starts with a `TOKEN_MANDATORY_LABEL`, whose identifier
    // pointer points into the same buffer.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
    if sid.is_null() {
        return Err(0);
    }
    // SAFETY: `sid` is the valid identifier the system just wrote; the call returns a
    // pointer to its count of parts, inside `buffer`.
    let parts = unsafe { *GetSidSubAuthorityCount(sid) };
    if parts == 0 {
        return Err(0);
    }
    // SAFETY: `sid` is valid and has `parts` parts, so the index is in range; the call
    // returns a pointer to that part, inside `buffer`.
    Ok(unsafe { *GetSidSubAuthority(sid, u32::from(parts) - 1) })
}

/// This process's integrity level, read once.
fn own_integrity() -> Option<u32> {
    static OWN: OnceLock<Option<u32>> = OnceLock::new();
    *OWN.get_or_init(|| {
        // SAFETY: the call takes no arguments; the pseudo handle needs no closing.
        integrity(unsafe { GetCurrentProcess() }).ok()
    })
}

/// The system, for the steps in `secure`.
#[derive(Debug, Clone, Copy)]
pub(super) struct SystemDesk;

impl Desk for SystemDesk {
    fn front(&self) -> Option<Front> {
        foreground()
    }

    fn privilege(&self, pid: u32) -> Privilege {
        let Some(own) = own_integrity() else {
            return Privilege::Unknown;
        };
        // A process that may not even be asked its level is not one of this user's
        // ordinary ones.
        let theirs = open_process(pid).and_then(|process| integrity(process.0));
        match theirs {
            Ok(theirs) if theirs > own => Privilege::Higher,
            Ok(_) => Privilege::NotHigher,
            Err(ERROR_ACCESS_DENIED) => Privilege::Higher,
            Err(_) => Privilege::Unknown,
        }
    }

    fn control(&self, front: Front) -> Control {
        let window = handle(front.window);
        let control = focused_control(window).unwrap_or(window);
        // SAFETY: a stale handle is answered with 0. The call reads what the system keeps
        // about the window and sends it no message.
        let style = unsafe { GetWindowLongW(control, GWL_STYLE) }.cast_unsigned();
        Control {
            window: number(control),
            // A window that is gone meanwhile has no process; 0 is no process's id.
            pid: owner(control).map_or(0, |(_, pid)| pid),
            kind: classify::control_kind(&real_class(control), style),
        }
    }

    fn has_window_in(&self, front: Front, pid: u32) -> bool {
        /// What the enumeration looks for, and whether it found it.
        struct Search {
            pid: u32,
            found: bool,
        }
        unsafe extern "system" fn visit(window: HWND, search: LPARAM) -> BOOL {
            // SAFETY: `search` is the `Search` passed below, alive for the whole enumeration,
            // and only this callback uses it, on this thread.
            let search = unsafe { &mut *(search as *mut Search) };
            if owner(window).is_some_and(|(_, pid)| pid == search.pid) {
                search.found = true;
                // Stop: found.
                return 0;
            }
            1
        }
        let mut search = Search { pid, found: false };
        // SAFETY: the callback is a function of this crate with the right signature, and the
        // parameter is a pointer to `search`, which outlives the call. The enumeration walks
        // the system's list of windows and sends none of them a message.
        unsafe {
            EnumChildWindows(
                handle(front.window),
                Some(visit),
                (&raw mut search) as LPARAM,
            )
        };
        search.found
    }
}

/// The process whose application `front` shows. For the frame the system puts around an
/// application that runs in an application container, that is the process of the
/// application's own window inside the frame, not the frame's.
fn app_process(front: Front) -> u32 {
    let window = handle(front.window);
    if real_class(window) != FRAME_CLASS {
        return front.pid;
    }
    let class: Vec<u16> = CORE_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `class` is NUL-terminated and outlives the call; a stale parent handle and no
    // match are both answered with null. The call walks the system's list of windows.
    let inner = unsafe { FindWindowExW(window, ptr::null_mut(), class.as_ptr(), ptr::null()) };
    owner(inner).map_or(front.pid, |(_, pid)| pid)
}

/// The application `front` shows.
pub(super) fn identity_of(front: Front) -> AppIdentity {
    let pid = app_process(front);
    let image = open_process(pid)
        .ok()
        .and_then(|process| image_path(process.0));
    identity::identify(image.as_deref(), pid, own_image())
}

/// How the window of a focus event relates to the window in front.
pub(super) fn relation(source: u64, front: Front) -> Relation {
    let source = handle(source);
    let Some((_, pid)) = owner(source) else {
        return Relation {
            unnamed: true,
            same_process: false,
            same_root: false,
        };
    };
    let root = |window| {
        // SAFETY: a stale handle is answered with null. The call walks the system's window
        // tree and sends no message.
        unsafe { GetAncestor(window, GA_ROOTOWNER) }
    };
    let source_root = root(source);
    Relation {
        unnamed: false,
        same_process: pid == front.pid,
        same_root: !source_root.is_null() && source_root == root(handle(front.window)),
    }
}

/// True when `window` exists and belongs to process `pid`.
pub(super) fn is_window_of(window: u64, pid: u32) -> bool {
    let window = handle(window);
    // SAFETY: any value is accepted; one that names no window is answered with 0.
    let exists = unsafe { IsWindow(window) } != 0;
    exists && owner(window).is_some_and(|(_, owner)| owner == pid)
}

/// Asks the system to bring `window` to the front, restoring it first if it is minimised.
/// False when the system refuses: it only lets the program that has the foreground, or
/// sent the last input, give it away.
pub(super) fn set_foreground(window: u64) -> bool {
    let window = handle(window);
    // SAFETY: a stale handle is refused by each of the three calls. They queue the change
    // for the window's thread and do not wait for it.
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindowAsync(window, SW_RESTORE);
        }
        SetForegroundWindow(window) != 0
    }
}

/// Lets process `pid` bring a window of its own to the front.
pub(super) fn allow_set_foreground(pid: u32) -> Result<()> {
    // SAFETY: a plain value; a process that does not exist is refused.
    if unsafe { AllowSetForegroundWindow(pid) } == 0 {
        Err(os_error("AllowSetForegroundWindow"))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The watcher thread's hooks, timer and messages
// ---------------------------------------------------------------------------------------------

/// The id of the calling thread.
pub(super) fn current_thread_id() -> u32 {
    // SAFETY: the call takes no arguments and cannot fail.
    unsafe { GetCurrentThreadId() }
}

/// Gives the calling thread a message queue, so that messages can be posted to it.
pub(super) fn create_message_queue() {
    let mut message = MSG::default();
    // SAFETY: `message` is a valid place for one message; nothing is removed from the queue,
    // and a null window means "messages of this thread".
    unsafe {
        PeekMessageW(
            &raw mut message,
            ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        )
    };
}

/// An installed window-event hook. Removed when dropped, which must happen on the thread
/// that installed it.
#[derive(Debug)]
pub(super) struct EventHook(HWINEVENTHOOK);

impl EventHook {
    /// Has `procedure` called on the calling thread, while that thread retrieves messages,
    /// for every event of `kind` in any process of the desktop.
    pub(super) fn install(kind: EventKind, procedure: EventProcedure) -> Result<Self> {
        let event = match kind {
            EventKind::Foreground => EVENT_SYSTEM_FOREGROUND,
            EventKind::Focus => EVENT_OBJECT_FOCUS,
        };
        // SAFETY: `procedure` is a function of this executable with the signature of a
        // window-event procedure and lives as long as the process. Out of context, the
        // system loads nothing into other processes: it queues the events for this thread,
        // so no module handle is needed. Process 0 and thread 0 mean "all".
        let hook = unsafe {
            SetWinEventHook(
                event,
                event,
                ptr::null_mut(),
                Some(procedure),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_null() {
            Err(os_error("SetWinEventHook"))
        } else {
            Ok(Self(hook))
        }
    }

    /// The kind of event a procedure was called with.
    pub(super) const fn kind_of(event: u32) -> Option<EventKind> {
        match event {
            EVENT_SYSTEM_FOREGROUND => Some(EventKind::Foreground),
            EVENT_OBJECT_FOCUS => Some(EventKind::Focus),
            _ => None,
        }
    }
}

impl Drop for EventHook {
    fn drop(&mut self) {
        // SAFETY: the hook came from `SetWinEventHook` and is removed once. Nothing can be
        // done about a refusal.
        unsafe { UnhookWinEvent(self.0) };
    }
}

/// Starts a timer of the calling thread that posts [`Message::Timer`] to its queue every
/// `ms` milliseconds until it is stopped. `None` when the system has no timer left.
pub(super) fn start_timer(ms: u32) -> Option<usize> {
    // SAFETY: without a window and without a procedure the timer belongs to the calling
    // thread and announces itself with a message; the id passed is ignored.
    let id = unsafe { SetTimer(ptr::null_mut(), 0, ms, None) };
    (id != 0).then_some(id)
}

/// Stops a timer of the calling thread.
pub(super) fn stop_timer(id: usize) {
    // SAFETY: a timer of this thread without a window; an id that is not a timer is refused.
    unsafe { KillTimer(ptr::null_mut(), id) };
}

/// A message of the watcher thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Message {
    /// The loop must end.
    Stop,
    /// Retrieving a message failed, with this error code.
    Failed(i64),
    /// Report this window again after a pause.
    LookAgain(u64),
    /// A late answer about this window arrived: report it again now.
    LateAnswer(u64),
    /// The timer with this id is due.
    Timer(usize),
    /// Anything else.
    Other,
}

/// Waits for the next message of the calling thread. The window-event procedure is called
/// from inside this wait.
pub(super) fn next_message() -> Message {
    let mut message = MSG::default();
    // SAFETY: `message` is a valid place for one message; a null window and a 0..0 range
    // mean "any message of this thread".
    let got = unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) };
    match got {
        0 => return Message::Stop,
        -1 => return Message::Failed(last_error()),
        _ => {}
    }
    if !message.hwnd.is_null() {
        // The watcher creates no window, but the system may give the thread one of its own.
        // SAFETY: `message` was filled in by `GetMessageW` just now.
        unsafe { DispatchMessageW(&raw const message) };
        return Message::Other;
    }
    match message.message {
        WM_LOOK_AGAIN => Message::LookAgain(message.wParam as u64),
        WM_LATE_ANSWER => Message::LateAnswer(message.wParam as u64),
        WM_TIMER => Message::Timer(message.wParam),
        _ => Message::Other,
    }
}

/// Posts a message without pointers to the queue of `thread`.
pub(super) fn post_thread_message(thread: u32, message: u32, wparam: usize) -> Result<()> {
    // SAFETY: the parameters are plain numbers; the receiving loop does not treat them as
    // addresses.
    if unsafe { PostThreadMessageW(thread, message, wparam, 0) } == 0 {
        Err(os_error("PostThreadMessageW"))
    } else {
        Ok(())
    }
}

/// The error for a thread that could not be created.
pub(super) fn thread_error(error: &std::io::Error) -> PlatformError {
    PlatformError::os(error.raw_os_error().map_or(0, i64::from), "CreateThread")
}
