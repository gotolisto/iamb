//! The probe window of the end-to-end latency measurement (`docs/test-strategy.md` §6), behind
//! the `probe` feature. Test tooling for `iamb-probe` (in `iamb-testkit`): the feature is off by
//! default and no shipped binary enables it.
//!
//! One top-level window with one Win32 Rich Edit control (`RICHEDIT50W` of `Msftedit.dll`, in
//! plain-text mode), on a thread of its own with a message loop. The window reports two kinds
//! of [`Event`] to the sink it was opened with, each stamped with [`Instant::now`] (which is
//! `QueryPerformanceCounter` on Windows) before anything else is done:
//!
//! - [`Event::Key`]: the control received a key that produced a character. The stamp is taken
//!   when the message loop takes the key's `WM_KEYDOWN` for the control from the thread's
//!   queue; the character comes with the `WM_CHAR` that `TranslateMessage` makes of it.
//! - [`Event::Changed`]: the control's text changed (`EN_CHANGE`, which the control sends to
//!   its parent window after every edit, typed, pasted or set). The stamp is taken when the
//!   notification arrives; the whole text is read after it.
//!
//! Which key is the last of an abbreviation and which change completes the expansion is
//! decided by the caller (`iamb_testkit::compat::probe::session`). The window itself keeps no
//! text, writes nothing and logs nothing.
//!
//! The parent is a window of the system's dialog class (as in the focus tests), subclassed to
//! receive the control's notifications, so no window class of its own has to be registered.

#![allow(
    clippy::missing_errors_doc,
    reason = "test tooling: errors are readable strings for the person running the probe"
)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
    SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallWindowProcW, CreateWindowExW, DestroyWindow, DispatchMessageW, EN_CHANGE,
    ES_AUTOVSCROLL, ES_MULTILINE, GUITHREADINFO, GWLP_WNDPROC, GetClientRect, GetForegroundWindow,
    GetGUIThreadInfo, GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, MSG, MoveWindow, PostThreadMessageW, SMTO_NORMAL, SW_SHOW,
    SendMessageTimeoutW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowTextW,
    ShowWindow, TranslateMessage, WM_APP, WM_CHAR, WM_COMMAND, WM_KEYDOWN, WM_QUIT, WM_SIZE,
    WM_USER, WNDPROC, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

/// What the probe window reports. Both stamps are [`Instant`]s: `QueryPerformanceCounter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// The control received a key that produced `ch`.
    Key {
        /// When the key reached the control's thread.
        at: Instant,
        /// The character the key produced.
        ch: char,
    },
    /// The control's text changed.
    Changed {
        /// When the change notification arrived (before the text was read).
        at: Instant,
        /// The control's whole text after the change (line breaks as the control keeps
        /// them: `\r`).
        text: &'a str,
    },
}

/// Where the window sends its events. Called on the window's thread; it should return
/// quickly. A panic in it is caught and the event dropped.
pub type Sink = Arc<dyn Fn(Event<'_>) + Send + Sync>;

/// `EM_SETEVENTMASK` (`richedit.h`): which notifications the control sends its parent.
const EM_SETEVENTMASK: u32 = WM_USER + 69;
/// `ENM_CHANGE`: send `EN_CHANGE`.
const ENM_CHANGE: isize = 0x0000_0001;
/// `EM_SETTEXTMODE`: plain or rich text; only allowed while the control is empty.
const EM_SETTEXTMODE: u32 = WM_USER + 89;
/// `TM_PLAINTEXT`.
const TM_PLAINTEXT: usize = 1;
/// The window's own message: empty the control.
const WM_PROBE_CLEAR: u32 = WM_APP + 0x1A;
/// A key no keyboard has and no application reacts to (see `focus`).
const VK_UNASSIGNED: u16 = 0xE8;
/// How long [`ProbeWindow::reset`] waits for the window's thread.
const THREAD_TIMEOUT: Duration = Duration::from_secs(2);
/// How long [`ProbeWindow::focus`] keeps trying.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(3);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// What the window procedure needs, on the window's thread.
struct Local {
    sink: Sink,
    control: HWND,
    parent_proc: WNDPROC,
}

thread_local! {
    /// Set on the window's thread before the parent is subclassed; read by its procedure.
    static LOCAL: RefCell<Option<Rc<Local>>> = const { RefCell::new(None) };
}

fn local() -> Option<Rc<Local>> {
    LOCAL.with(|l| l.borrow().clone())
}

fn report(sink: &Sink, event: Event<'_>) {
    // A panic must not unwind into the system's frames.
    let _ = catch_unwind(AssertUnwindSafe(|| sink(event)));
}

/// The control's whole text.
fn read_text(control: HWND) -> String {
    // SAFETY: `control` is a window of the calling thread; a stale handle is answered with 0.
    let len = unsafe { GetWindowTextLengthW(control) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    let mut buf = vec![0_u16; len + 1];
    let capacity = i32::try_from(buf.len()).unwrap_or(i32::MAX);
    // SAFETY: `buf` holds `capacity` units; the call writes at most that many, NUL included.
    let copied = unsafe { GetWindowTextW(control, buf.as_mut_ptr(), capacity) };
    let copied = usize::try_from(copied).unwrap_or(0).min(len);
    String::from_utf16_lossy(&buf[..copied])
}

/// Fits the control to the parent's client area.
fn fit(parent: HWND, control: HWND) {
    let mut area = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `area` is a valid place for one rectangle; `parent` is a window of this thread.
    if unsafe { GetClientRect(parent, &raw mut area) } != 0 {
        // SAFETY: `control` is a child of `parent`, on this thread.
        unsafe {
            MoveWindow(
                control,
                8,
                8,
                (area.right - 16).max(16),
                (area.bottom - 16).max(16),
                1,
            );
        }
    }
}

/// The parent's procedure: the control's change notifications, the clear request and resizing;
/// everything else goes to the dialog class's own procedure.
unsafe extern "system" fn parent_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(local) = local() else {
        return 0;
    };
    match msg {
        WM_COMMAND
            if lparam as HWND == local.control && (wparam >> 16) & 0xFFFF == EN_CHANGE as usize =>
        {
            let at = Instant::now();
            let text = read_text(local.control);
            report(&local.sink, Event::Changed { at, text: &text });
            return 0;
        }
        WM_PROBE_CLEAR => {
            let empty = wide("");
            // SAFETY: `control` is a window of this thread; `empty` is NUL-terminated.
            unsafe { SetWindowTextW(local.control, empty.as_ptr()) };
            return 1;
        }
        WM_SIZE => fit(hwnd, local.control),
        _ => {}
    }
    // SAFETY: `parent_proc` is the procedure the dialog class gave this window; the arguments
    // are the ones this procedure received.
    unsafe { CallWindowProcW(local.parent_proc, hwnd, msg, wparam, lparam) }
}

/// Creates the windows on the calling thread. `None` when the session cannot.
fn create(title: &str, sink: Sink) -> Option<(HWND, HWND)> {
    let library = wide("Msftedit.dll");
    // SAFETY: a NUL-terminated name of a system library; it stays loaded for the life of the
    // process, which the control's class needs.
    if unsafe { LoadLibraryW(library.as_ptr()) }.is_null() {
        return None;
    }
    let class = wide("#32770");
    let title = wide(title);
    // SAFETY: `class` names a system class and `title` is NUL-terminated; no parent, menu,
    // module or creation data.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            120,
            120,
            660,
            440,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if window.is_null() {
        return None;
    }
    let edit_class = wide("RICHEDIT50W");
    let style = WS_CHILD
        | WS_VISIBLE
        | WS_TABSTOP
        | WS_BORDER
        | WS_VSCROLL
        | (ES_MULTILINE | ES_AUTOVSCROLL).cast_unsigned();
    // SAFETY: `edit_class` names the class `Msftedit.dll` registered and is NUL-terminated;
    // `window` is this thread's window; no menu, module or creation data.
    let control = unsafe {
        CreateWindowExW(
            0,
            edit_class.as_ptr(),
            ptr::null(),
            style,
            8,
            8,
            620,
            380,
            window,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if control.is_null() {
        // SAFETY: the window belongs to this thread and is destroyed once.
        unsafe { DestroyWindow(window) };
        return None;
    }
    // SAFETY: `control` is a Rich Edit control of this thread, still empty; both messages take
    // plain values.
    unsafe {
        SendMessageW(control, EM_SETTEXTMODE, TM_PLAINTEXT, 0);
        SendMessageW(control, EM_SETEVENTMASK, 0, ENM_CHANGE);
    }
    // SAFETY: `window` is this thread's window; the index names its procedure.
    let original = unsafe { GetWindowLongPtrW(window, GWLP_WNDPROC) };
    // SAFETY: the value is the window's procedure as the system keeps it: a function pointer of
    // the `WNDPROC` type, or 0, which is `None`. Both have the size of an `isize`.
    let original: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(original) };
    LOCAL.with(|l| {
        *l.borrow_mut() = Some(Rc::new(Local {
            sink,
            control,
            parent_proc: original,
        }));
    });
    let procedure: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = parent_proc;
    // SAFETY: `window` is this thread's window; the new procedure has the `WNDPROC` signature
    // and hands every message it does not handle to the original one, stored above.
    unsafe { SetWindowLongPtrW(window, GWLP_WNDPROC, (procedure as usize).cast_signed()) };
    fit(window, control);
    // SAFETY: `window` is this thread's window.
    unsafe { ShowWindow(window, SW_SHOW) };
    Some((window, control))
}

/// The probe window: one Rich Edit control in a top-level window, on a thread of its own.
/// Dropping it closes the window.
#[derive(Debug)]
pub struct ProbeWindow {
    window: usize,
    control: usize,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl ProbeWindow {
    /// Opens the window with `title`; its events go to `sink`. Fails when the session cannot
    /// create a window or the Rich Edit library is missing.
    pub fn open(title: &str, sink: Sink) -> Result<Self, String> {
        let title = title.to_owned();
        let (ready, announced) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("iamb-probe-window".to_owned())
            .spawn(move || {
                let created = create(&title, sink);
                // SAFETY: the call takes no arguments.
                let thread_id = unsafe { GetCurrentThreadId() };
                let _ = ready.send(created.map(|(w, c)| (w as usize, c as usize, thread_id)));
                let Some((window, control)) = created else {
                    return;
                };
                let mut last_key: Option<Instant> = None;
                let mut message = MSG::default();
                // SAFETY: `message` is a valid place for one message; the loop ends on the
                // quit message (0) or an error (-1).
                while unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) } > 0 {
                    if message.hwnd == control {
                        match message.message {
                            WM_KEYDOWN => last_key = Some(Instant::now()),
                            WM_CHAR => {
                                let at = last_key.take().unwrap_or_else(Instant::now);
                                let unit = u32::try_from(message.wParam).ok();
                                if let (Some(ch), Some(local)) =
                                    (unit.and_then(char::from_u32), local())
                                {
                                    report(&local.sink, Event::Key { at, ch });
                                }
                            }
                            _ => {}
                        }
                    }
                    // SAFETY: `message` was just filled by `GetMessageW`.
                    unsafe {
                        TranslateMessage(&raw const message);
                        DispatchMessageW(&raw const message);
                    }
                }
                // SAFETY: the window belongs to this thread and is destroyed once, with its
                // control. Its procedure still has its state, so the dialog class sees the
                // destroy messages.
                unsafe { DestroyWindow(window) };
                LOCAL.with(|l| l.borrow_mut().take());
            })
            .map_err(|e| format!("cannot start the window's thread: {e}"))?;
        if let Ok(Some((window, control, thread_id))) = announced.recv() {
            Ok(Self {
                window,
                control,
                thread_id,
                thread: Some(thread),
            })
        } else {
            let _ = thread.join();
            Err("this session cannot create the probe window (no desktop, or Msftedit.dll is missing)".to_owned())
        }
    }

    fn hwnd(&self) -> HWND {
        self.window as HWND
    }

    fn control_hwnd(&self) -> HWND {
        self.control as HWND
    }

    /// Empties the control (it reports the change) and gives it the keyboard focus. `Ok(false)`
    /// when the window could not take the focus.
    pub fn reset(&self) -> Result<bool, String> {
        let mut result = 0_usize;
        // SAFETY: plain values; the window's thread answers the message from its procedure,
        // and the call gives up after the timeout instead of waiting for a hung thread.
        let sent = unsafe {
            SendMessageTimeoutW(
                self.hwnd(),
                WM_PROBE_CLEAR,
                0,
                0,
                SMTO_NORMAL,
                u32::try_from(THREAD_TIMEOUT.as_millis()).unwrap_or(u32::MAX),
                &raw mut result,
            )
        };
        if sent == 0 || result != 1 {
            return Err("the probe window's thread did not answer".to_owned());
        }
        Ok(self.focus())
    }

    /// True when the window is in front and its control has the keyboard focus.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        // SAFETY: the call takes no arguments.
        if unsafe { GetForegroundWindow() } != self.hwnd() {
            return false;
        }
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..GUITHREADINFO::default()
        };
        // SAFETY: `info` is a valid structure with its size set.
        let known = unsafe { GetGUIThreadInfo(self.thread_id, &raw mut info) } != 0;
        known && info.hwndFocus == self.control_hwnd()
    }

    /// Brings the window to the front and gives the control the keyboard focus, trying for up
    /// to three seconds. False when the session does not allow it.
    ///
    /// When the system refuses the foreground, the next try first sends the release of a key
    /// no keyboard has (a program may take the foreground when it sent the last input event),
    /// as the focus tests do. It produces no character.
    #[must_use]
    pub fn focus(&self) -> bool {
        let deadline = Instant::now() + FOCUS_TIMEOUT;
        let mut tries = 0_u32;
        while Instant::now() < deadline {
            if self.has_focus() {
                return true;
            }
            if tries > 0 {
                send_keys(&[key_input(VK_UNASSIGNED, 0, KEYEVENTF_KEYUP)]);
            }
            tries += 1;
            // SAFETY: the call takes no arguments.
            let front = unsafe { GetForegroundWindow() };
            let front_thread = if front.is_null() {
                0
            } else {
                // SAFETY: a stale handle is answered with 0; no process id is asked for.
                unsafe { GetWindowThreadProcessId(front, ptr::null_mut()) }
            };
            // SAFETY: the call takes no arguments.
            let me = unsafe { GetCurrentThreadId() };
            // SAFETY: plain values; attaching to thread 0 or to itself fails harmlessly.
            let attached = front_thread != 0
                && front_thread != me
                && unsafe { AttachThreadInput(me, front_thread, 1) } != 0;
            // SAFETY: the handles are this window and its control; the calls fail harmlessly
            // when the system refuses.
            unsafe {
                AttachThreadInput(me, self.thread_id, 1);
                BringWindowToTop(self.hwnd());
                SetForegroundWindow(self.hwnd());
                SetFocus(self.control_hwnd());
                AttachThreadInput(me, self.thread_id, 0);
                if attached {
                    AttachThreadInput(me, front_thread, 0);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.has_focus()
    }

    /// Types `text` into whatever has the keyboard focus with `SendInput`, one Unicode key
    /// press and release per UTF-16 unit, unmarked: ordinary synthetic keys, as the system's
    /// own key senders make them. For the probe's self-test and the crate's tests; a latency
    /// run types through the driver instead.
    pub fn type_synthetic(&self, text: &str) -> Result<(), String> {
        let mut inputs = Vec::new();
        for unit in text.encode_utf16() {
            inputs.push(key_input(0, unit, KEYEVENTF_UNICODE));
            inputs.push(key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
        if send_keys(&inputs) == inputs.len() {
            Ok(())
        } else {
            Err("SendInput did not take every key (another desktop or a higher-integrity window is in front)".to_owned())
        }
    }
}

impl Drop for ProbeWindow {
    fn drop(&mut self) {
        // SAFETY: plain values; the thread ends its loop on this message.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Sends `inputs`; returns how many the system took.
fn send_keys(inputs: &[INPUT]) -> usize {
    let Ok(count) = u32::try_from(inputs.len()) else {
        return 0;
    };
    let size = i32::try_from(size_of::<INPUT>()).unwrap_or(0);
    // SAFETY: `inputs` holds `count` valid `INPUT`s of `size` bytes each.
    let sent = unsafe { SendInput(count, inputs.as_ptr(), size) };
    usize::try_from(sent).unwrap_or(0)
}
