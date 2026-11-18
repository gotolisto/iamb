//! What the injector and clipboard tests share: a test window to type into, a stand-in for
//! the listener that seals the hand-off when the injector's start marker goes by, raw
//! clipboard access, and the rule for skipping.
//!
//! This file is a module of `injector_roundtrip.rs` and `clipboard_restore.rs`. Cargo also
//! builds it as a test of its own, which has no tests.

#![cfg(windows)]
// Each test uses a part of this file, and the stand-alone build uses none of it.
#![allow(dead_code)]

use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use iamb_platform::core_types::{ExpansionId, Sensitive};
use iamb_platform::{ListenerEvent, Marker, TypedHandoff};
use windows_sys::Win32::Foundation::{GlobalFree, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, SetFocus,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallNextHookEx, CreateWindowExW, DestroyWindow, DispatchMessageW,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, GUITHREADINFO, GetForegroundWindow,
    GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId, HC_ACTION, HWND_MESSAGE,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SW_SHOW,
    SendMessageW, SetForegroundWindow, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_GETTEXT, WM_GETTEXTLENGTH, WM_QUIT, WM_SETTEXT,
    WM_USER, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

/// `EM_SETSEL` (its namespace is not among the crate's bindings).
const EM_SETSEL: u32 = 0x00B1;
/// A key no keyboard has and no application reacts to.
const VK_UNASSIGNED: u16 = 0xE8;

/// The tests of one file share the keyboard and the clipboard: one at a time.
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

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------------------------
// The hand-off, sealed by a hook that plays the listener
// ---------------------------------------------------------------------------------------------

/// The hand-off the tests inject with. The hook below seals it.
pub(crate) static HANDOFF: LazyLock<TypedHandoff> = LazyLock::new(TypedHandoff::new);

static NEXT_EXPANSION: AtomicU64 = AtomicU64::new(1);

/// A fresh expansion id, with the hand-off armed for it.
pub(crate) fn arm() -> ExpansionId {
    let expansion = ExpansionId::from_raw(NEXT_EXPANSION.fetch_add(1, Ordering::SeqCst));
    HANDOFF.arm(expansion);
    expansion
}

/// The user typed `ch` after the trigger: as the engine-feeding thread would, the hand-off
/// sees it before the start marker.
pub(crate) fn typed_after_trigger(ch: char) {
    HANDOFF.observe(&ListenerEvent::Char {
        ch: Sensitive::new(ch),
        seq: 0,
    });
}

/// What the real listener does with Iamb's markers; every other event passes untouched.
unsafe extern "system" fn seal(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if u32::try_from(code) == Ok(HC_ACTION) {
        // SAFETY: a `WH_KEYBOARD_LL` hook procedure called with `HC_ACTION`, not yet
        // returned: `lparam` is the address of the event.
        let event = unsafe { ptr::read(lparam as *const KBDLLHOOKSTRUCT) };
        if event.flags & LLKHF_INJECTED != 0 {
            match Marker::decode(event.dwExtraInfo as u64) {
                Some(Marker::InjectionStart(expansion)) => {
                    HANDOFF.observe(&ListenerEvent::InjectionStarted { expansion, seq: 1 });
                }
                Some(Marker::InjectionEnd(expansion)) => {
                    HANDOFF.observe(&ListenerEvent::InjectionFinished { expansion, seq: 2 });
                }
                _ => {}
            }
        }
    }
    // SAFETY: the arguments are the ones the system called this procedure with.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// A thread with the sealing hook.
pub(crate) struct Sealer {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl Sealer {
    pub(crate) fn start() -> Option<Self> {
        let (ready, announced) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut message = MSG::default();
            // SAFETY: `message` is a valid place for one message; nothing is removed. The
            // call gives this thread a message queue.
            unsafe {
                PeekMessageW(
                    &raw mut message,
                    ptr::null_mut(),
                    WM_USER,
                    WM_USER,
                    PM_NOREMOVE,
                );
            }
            // SAFETY: a null name asks for this executable's module, `seal` is a hook
            // procedure of it, and a low-level hook takes thread id 0.
            let hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(seal), GetModuleHandleW(ptr::null()), 0)
            };
            // SAFETY: the call takes no arguments.
            let thread_id = unsafe { GetCurrentThreadId() };
            let _ = ready.send((!hook.is_null()).then_some(thread_id));
            if hook.is_null() {
                return;
            }
            // SAFETY: `message` is a valid place for one message; the loop ends on the quit
            // message (0) or an error (-1).
            while unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) } > 0 {}
            // SAFETY: the hook came from `SetWindowsHookExW` above and is removed once.
            unsafe { UnhookWindowsHookEx(hook) };
        });
        if let Ok(Some(thread_id)) = announced.recv() {
            Some(Self {
                thread: Some(thread),
                thread_id,
            })
        } else {
            let _ = thread.join();
            None
        }
    }
}

impl Drop for Sealer {
    fn drop(&mut self) {
        // SAFETY: plain values; the thread ends its loop on this message.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The test window
// ---------------------------------------------------------------------------------------------

/// A visible, multi-line edit control on a thread of its own, with a message loop.
pub(crate) struct TestWindow {
    window: usize,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl TestWindow {
    pub(crate) fn open() -> Option<Self> {
        let (ready, announced) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let class = wide("EDIT");
            let title = wide("Iamb injector test");
            let style = WS_OVERLAPPEDWINDOW
                | WS_VISIBLE
                | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN).cast_unsigned();
            // SAFETY: `class` names a system class and `title` is NUL-terminated; no parent,
            // menu or creation data.
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    style,
                    100,
                    100,
                    600,
                    400,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            // SAFETY: the call takes no arguments.
            let thread_id = unsafe { GetCurrentThreadId() };
            let _ = ready.send((!window.is_null()).then_some((window as usize, thread_id)));
            if window.is_null() {
                return;
            }
            // SAFETY: `window` is this thread's window.
            unsafe { ShowWindow(window, SW_SHOW) };
            let mut message = MSG::default();
            // SAFETY: `message` is a valid place for one message. `TranslateMessage` turns
            // the Unicode key events into characters, as in any application.
            while unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) } > 0 {
                // SAFETY: `message` was just filled by `GetMessageW`.
                unsafe {
                    TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
            // SAFETY: the window belongs to this thread and is destroyed once.
            unsafe { DestroyWindow(window) };
        });
        if let Ok(Some((window, thread_id))) = announced.recv() {
            Some(Self {
                window,
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

    /// True when the window is in front and has the keyboard focus.
    pub(crate) fn has_focus(&self) -> bool {
        // SAFETY: the call takes no arguments.
        if unsafe { GetForegroundWindow() } != self.hwnd() {
            return false;
        }
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).expect("small"),
            ..GUITHREADINFO::default()
        };
        // SAFETY: `info` is a valid structure with its size set.
        let known = unsafe { GetGUIThreadInfo(self.thread_id, &raw mut info) } != 0;
        known && info.hwndFocus == self.hwnd()
    }

    /// Brings the window to the front with the keyboard focus. False when the session does
    /// not allow it.
    pub(crate) fn focus(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.has_focus() {
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
            // SAFETY: the window handle is this test's window; the calls fail harmlessly when
            // the system refuses.
            unsafe {
                AttachThreadInput(me, self.thread_id, 1);
                BringWindowToTop(self.hwnd());
                SetForegroundWindow(self.hwnd());
                SetFocus(self.hwnd());
                AttachThreadInput(me, self.thread_id, 0);
                if attached {
                    AttachThreadInput(me, front_thread, 0);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.has_focus()
    }

    /// Replaces the text and puts the caret at its end.
    pub(crate) fn set_text(&self, text: &str) {
        let wide = wide(text);
        let len = wide.len() - 1;
        // SAFETY: `wide` is NUL-terminated and outlives the call, which the window's thread
        // handles while this one waits.
        unsafe {
            SendMessageW(self.hwnd(), WM_SETTEXT, 0, wide.as_ptr() as LPARAM);
            SendMessageW(self.hwnd(), EM_SETSEL, len, len.cast_signed());
        }
    }

    /// The text, exactly as the control holds it (CR LF line breaks).
    pub(crate) fn text(&self) -> String {
        // SAFETY: plain values.
        let len = unsafe { SendMessageW(self.hwnd(), WM_GETTEXTLENGTH, 0, 0) };
        let len = usize::try_from(len).unwrap_or(0);
        let mut buffer = vec![0u16; len + 1];
        // SAFETY: `buffer` has room for `len + 1` units, the NUL included.
        let copied = unsafe {
            SendMessageW(
                self.hwnd(),
                WM_GETTEXT,
                buffer.len(),
                buffer.as_mut_ptr() as LPARAM,
            )
        };
        let copied = usize::try_from(copied).unwrap_or(0).min(len);
        String::from_utf16_lossy(&buffer[..copied])
    }

    /// Waits until the text is `expected` (the application handles injected keys after
    /// `SendInput` returns), then returns the text.
    pub(crate) fn wait_for(&self, expected: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = self.text();
            if text == expected || Instant::now() > deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
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

// ---------------------------------------------------------------------------------------------
// Raw clipboard access, independent of the code under test
// ---------------------------------------------------------------------------------------------

pub(crate) const CF_UNICODETEXT: u32 = 13;
pub(crate) const CF_DIB: u32 = 8;
pub(crate) const CF_HDROP: u32 = 15;

/// The number of a registered format.
pub(crate) fn registered(name: &str) -> u32 {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated.
    let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    assert_ne!(format, 0, "RegisterClipboardFormatW");
    format
}

/// The clipboard, open on this thread with a message-only owner window; closed on drop.
pub(crate) struct Clip {
    owner: HWND,
}

// The methods take `&self` as proof that the clipboard is open on this thread.
#[allow(clippy::unused_self)]
impl Clip {
    /// Opens the clipboard, trying for up to a second. `None` when the session has none.
    pub(crate) fn open() -> Option<Self> {
        let class = wide("STATIC");
        // SAFETY: a message-only window of a system class; plain values otherwise.
        let owner = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if owner.is_null() {
            return None;
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // SAFETY: `owner` is this thread's window.
            if unsafe { OpenClipboard(owner) } != 0 {
                return Some(Self { owner });
            }
            if Instant::now() > deadline {
                // SAFETY: this thread's window, destroyed once.
                unsafe { DestroyWindow(owner) };
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Replaces the clipboard with these formats.
    pub(crate) fn put(&self, formats: &[(u32, Vec<u8>)]) {
        // SAFETY: the clipboard is open on this thread with an owner window.
        assert_ne!(unsafe { EmptyClipboard() }, 0, "EmptyClipboard");
        for (format, bytes) in formats {
            let size = bytes.len().max(1);
            // SAFETY: plain values.
            let block = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) };
            assert!(!block.is_null(), "GlobalAlloc");
            // SAFETY: a fresh block of `size >= bytes.len()` bytes, locked for the copy.
            unsafe {
                let base = GlobalLock(block).cast::<u8>();
                assert!(!base.is_null(), "GlobalLock");
                ptr::copy_nonoverlapping(bytes.as_ptr(), base, bytes.len());
                GlobalUnlock(block);
            }
            // SAFETY: the clipboard is open and owned by this thread's window; the system
            // takes the block over on success.
            if unsafe { SetClipboardData(*format, block) }.is_null() {
                // SAFETY: not taken over, so still ours; freed once.
                unsafe { GlobalFree(block) };
                panic!("SetClipboardData");
            }
        }
    }

    /// The formats on the clipboard.
    pub(crate) fn formats(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut format = 0;
        loop {
            // SAFETY: the clipboard is open on this thread.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                return out;
            }
            out.push(format);
        }
    }

    /// The bytes of a memory format, as `GlobalSize` reports them.
    pub(crate) fn get(&self, format: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open on this thread. The tests ask only for formats whose
        // data is global memory.
        unsafe {
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            let base = GlobalLock(handle).cast::<u8>();
            if base.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(base, size).to_vec();
            GlobalUnlock(handle);
            Some(bytes)
        }
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        // SAFETY: opened by this thread; closed once, then its window destroyed once.
        unsafe {
            CloseClipboard();
            DestroyWindow(self.owner);
        }
    }
}

/// UTF-16 little-endian bytes of `text` with a NUL.
pub(crate) fn utf16_bytes(text: &str) -> Vec<u8> {
    wide(text).into_iter().flat_map(u16::to_le_bytes).collect()
}
