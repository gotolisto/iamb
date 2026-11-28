//! What the focus tests share: a test window with a plain text field, a password field and
//! a button, the way to give one of them the keyboard focus, and the rule for skipping.
//!
//! This file is a module of `focus_e2e.rs` and of the crate's own Windows tests
//! (`src/focus/os_tests.rs`). Cargo also builds it as a test of its own, which has no tests.

#![cfg(windows)]
// Each user takes a part of this file, and the stand-alone build uses none of it.
#![allow(dead_code)]

use std::ptr;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL,
    ES_PASSWORD, GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
    GetWindowThreadProcessId, LB_ADDSTRING, LB_SETCURSEL, LBS_NOTIFY, MSG, PostThreadMessageW,
    SW_SHOW, SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, WM_QUIT, WS_BORDER,
    WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

/// A key no keyboard has and no application reacts to.
const VK_UNASSIGNED: u16 = 0xE8;

/// The tests of one file share the foreground: one at a time.
pub(crate) fn alone() -> MutexGuard<'static, ()> {
    static DESK: Mutex<()> = Mutex::new(());
    DESK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Says why a test did not run. With `IAMB_REQUIRE_DESKTOP` set that is a failure.
pub(crate) fn skip(why: &str) {
    assert!(
        std::env::var_os("IAMB_REQUIRE_DESKTOP").is_none(),
        "IAMB_REQUIRE_DESKTOP is set, but {why}"
    );
    eprintln!("SKIPPED: {why}");
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A control of the test window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// A single-line edit control.
    Plain,
    /// A single-line edit control with the password style.
    Password,
    /// A push button: not an edit control, so only UI Automation can say what it is.
    Button,
    /// A list box with one item, selected: when it has the focus, the focused element is the
    /// item, an element that is not a window.
    List,
}

/// A visible dialog-class window with the three [`Field`]s as child controls, on a thread
/// of its own with a message loop. Nothing is ever typed into it.
pub(crate) struct TestWindow {
    window: usize,
    fields: [usize; 4],
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// Creates a child control of a system class.
fn child(parent: HWND, class: &str, style: u32, row: i32) -> HWND {
    let class = wide(class);
    // SAFETY: `class` names a system class and is NUL-terminated; `parent` is a window of
    // the calling thread; no menu, module or creation data.
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            20,
            20 + 40 * row,
            300,
            28,
            parent,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    }
}

impl TestWindow {
    /// Opens the window. `None` when the session cannot create one.
    pub(crate) fn open() -> Option<Self> {
        let (ready, announced) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            // The system's dialog class: a plain container that hands the keyboard focus
            // back to the control that had it when the window comes to the front again.
            let class = wide("#32770");
            let title = wide("Iamb focus test");
            // SAFETY: `class` names a system class and `title` is NUL-terminated; no
            // parent, menu, module or creation data.
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    120,
                    120,
                    360,
                    260,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            let fields = if window.is_null() {
                [ptr::null_mut(); 4]
            } else {
                let edit = WS_BORDER | ES_AUTOHSCROLL.cast_unsigned();
                let list = child(window, "LISTBOX", WS_BORDER | LBS_NOTIFY.cast_unsigned(), 3);
                if !list.is_null() {
                    let item = wide("item");
                    // SAFETY: `list` is this thread's list box; the string is NUL-terminated
                    // and outlives the call, which copies it.
                    unsafe {
                        SendMessageW(list, LB_ADDSTRING, 0, item.as_ptr() as LPARAM);
                        SendMessageW(list, LB_SETCURSEL, 0, 0);
                    }
                }
                [
                    child(window, "EDIT", edit, 0),
                    child(window, "EDIT", edit | ES_PASSWORD.cast_unsigned(), 1),
                    // A push button is style 0 of its class.
                    child(window, "BUTTON", 0, 2),
                    list,
                ]
            };
            // SAFETY: the call takes no arguments.
            let thread_id = unsafe { GetCurrentThreadId() };
            let created = !window.is_null() && fields.iter().all(|field| !field.is_null());
            let _ = ready.send(created.then(|| {
                (
                    window as usize,
                    fields.map(|field| field as usize),
                    thread_id,
                )
            }));
            if window.is_null() {
                return;
            }
            if created {
                // SAFETY: `window` is this thread's window.
                unsafe { ShowWindow(window, SW_SHOW) };
                let mut message = MSG::default();
                // SAFETY: `message` is a valid place for one message; the loop ends on the
                // quit message (0) or an error (-1).
                while unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) } > 0 {
                    // SAFETY: `message` was just filled by `GetMessageW`.
                    unsafe {
                        TranslateMessage(&raw const message);
                        DispatchMessageW(&raw const message);
                    }
                }
            }
            // SAFETY: the window belongs to this thread and is destroyed once, with its
            // child controls.
            unsafe { DestroyWindow(window) };
        });
        if let Ok(Some((window, fields, thread_id))) = announced.recv() {
            Some(Self {
                window,
                fields,
                thread_id,
                thread: Some(thread),
            })
        } else {
            let _ = thread.join();
            None
        }
    }

    fn hwnd(&self) -> HWND {
        self.window as HWND
    }

    fn field(&self, field: Field) -> HWND {
        let index = match field {
            Field::Plain => 0,
            Field::Password => 1,
            Field::Button => 2,
            Field::List => 3,
        };
        self.fields[index] as HWND
    }

    /// The handle of `field`'s control, as a number.
    pub(crate) fn field_number(&self, field: Field) -> u64 {
        self.field(field) as u64
    }

    /// The window's handle as the number a focus token holds.
    pub(crate) fn number(&self) -> u64 {
        self.window as u64
    }

    /// True when the window is in front.
    pub(crate) fn in_front(&self) -> bool {
        // SAFETY: the call takes no arguments.
        unsafe { GetForegroundWindow() == self.hwnd() }
    }

    /// True when the window is in front and `field` has the keyboard focus.
    pub(crate) fn has_focus(&self, field: Field) -> bool {
        if !self.in_front() {
            return false;
        }
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).expect("small"),
            ..GUITHREADINFO::default()
        };
        // SAFETY: `info` is a valid structure with its size set.
        let known = unsafe { GetGUIThreadInfo(self.thread_id, &raw mut info) } != 0;
        known && info.hwndFocus == self.field(field)
    }

    /// Brings the window to the front and gives `field` the keyboard focus. False when the
    /// session does not allow it.
    pub(crate) fn focus(&self, field: Field) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.has_focus(field) {
                return true;
            }
            // A program may take the foreground when it sent the last input event: post a
            // key no application reacts to first. Attaching to the thread in front lets the
            // focus be set across threads.
            post_unassigned_release();
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
            // SAFETY: the handles are this test's window and its control; the calls fail
            // harmlessly when the system refuses.
            unsafe {
                AttachThreadInput(me, self.thread_id, 1);
                BringWindowToTop(self.hwnd());
                SetForegroundWindow(self.hwnd());
                SetFocus(self.field(field));
                AttachThreadInput(me, self.thread_id, 0);
                if attached {
                    AttachThreadInput(me, front_thread, 0);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.has_focus(field)
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: plain values; the thread ends its loop on this message.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Posts one release of an unassigned key, unmarked.
fn post_unassigned_release() {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_UNASSIGNED,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let size = i32::try_from(size_of::<INPUT>()).expect("small");
    // SAFETY: one `INPUT`, with its count and its size.
    unsafe { SendInput(1, &raw const input, size) };
}

/// Calls `look` until it returns something, for at most five seconds.
pub(crate) fn eventually<T>(mut look: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(found) = look() {
            return Some(found);
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
