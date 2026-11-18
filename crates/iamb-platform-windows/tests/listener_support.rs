//! What the end-to-end tests of the listener and of the hotkeys share: posting synthetic key
//! events, a hook that keeps them away from applications, and the check that this session
//! has a desktop that delivers them.
//!
//! This file is a module of `listener_e2e.rs` and `hotkeys_e2e.rs`. Cargo also builds it as a
//! test of its own, which has no tests.

#![cfg(windows)]
// Each of the two tests uses a part of this file, and the stand-alone build uses none of it.
#![allow(dead_code)]

use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use iamb_platform::core_types::BoundaryKind;
use iamb_platform::{ListenerEvent, Marker};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    ToUnicodeEx,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HC_ACTION,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_QUIT, WM_USER,
};

/// A key no keyboard has and no application reacts to.
pub(crate) const VK_UNASSIGNED: u16 = 0xE8;

/// Marks a test's key events in the extra-info field. It is not an Iamb marker, so the
/// listener takes such an event for real typing, as it does any other program's injected
/// input.
pub(crate) const TEST_TAG: usize = 0x7E57_1A3B_0000_0002;

/// The tests of one file share the keyboard: one at a time.
pub(crate) fn alone() -> MutexGuard<'static, ()> {
    static KEYBOARD: Mutex<()> = Mutex::new(());
    KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Says why a test did not run. With `IAMB_REQUIRE_DESKTOP` set that is a failure.
pub(crate) fn skip(why: &str) {
    assert!(
        std::env::var_os("IAMB_REQUIRE_DESKTOP").is_none(),
        "IAMB_REQUIRE_DESKTOP is set, but {why}"
    );
    eprintln!("SKIPPED: {why}");
}

/// One synthetic key event.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Key {
    pub(crate) vk: u16,
    pub(crate) release: bool,
    pub(crate) extra: usize,
}

impl Key {
    /// A press of `vk` that the watcher swallows.
    pub(crate) const fn press(vk: u16) -> Self {
        Self {
            vk,
            release: false,
            extra: TEST_TAG,
        }
    }

    /// A release of `vk` that the watcher swallows.
    pub(crate) const fn release(vk: u16) -> Self {
        Self {
            vk,
            release: true,
            extra: TEST_TAG,
        }
    }

    /// The same event, marked as Iamb's own.
    pub(crate) fn marked(self, marker: Marker) -> Self {
        Self {
            extra: usize::try_from(marker.encode()).expect("a 64-bit system"),
            ..self
        }
    }

    /// The same event without any mark: it reaches the system and the window in front.
    pub(crate) const fn for_real(self) -> Self {
        Self { extra: 0, ..self }
    }
}

/// Posts `keys` through the system's input stream as one batch. Returns how many the system
/// took.
pub(crate) fn send(keys: &[Key]) -> usize {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|key| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key.vk,
                    wScan: 0,
                    dwFlags: if key.release { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: key.extra,
                },
            },
        })
        .collect();
    let count = u32::try_from(inputs.len()).expect("a small batch");
    let size = i32::try_from(size_of::<INPUT>()).expect("a small structure");
    // SAFETY: `inputs` holds `count` structures of the size passed.
    let sent = unsafe { SendInput(count, inputs.as_ptr(), size) };
    usize::try_from(sent).expect("a count")
}

/// Key events the watcher swallowed: the tests' own.
static SWALLOWED: AtomicUsize = AtomicUsize::new(0);
/// Key events the watcher let pass: somebody else's, or sent `for_real`.
static PASSED: AtomicUsize = AtomicUsize::new(0);

/// The watcher's hook procedure. It swallows the injected events that carry the test tag or
/// an Iamb marker, so that they reach neither an application nor the system's picture of the
/// keyboard, and counts everything else.
unsafe extern "system" fn watch(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if u32::try_from(code) == Ok(HC_ACTION) {
        // SAFETY: this is a `WH_KEYBOARD_LL` hook procedure called with `HC_ACTION`, and it
        // has not returned: `lparam` is the address of the event.
        let event = unsafe { ptr::read(lparam as *const KBDLLHOOKSTRUCT) };
        let ours =
            event.dwExtraInfo == TEST_TAG || Marker::decode(event.dwExtraInfo as u64).is_some();
        if event.flags & LLKHF_INJECTED != 0 && ours {
            SWALLOWED.fetch_add(1, Ordering::SeqCst);
            return 1;
        }
        PASSED.fetch_add(1, Ordering::SeqCst);
    }
    // SAFETY: the arguments are the ones the system called this procedure with.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// A thread with the watching hook.
///
/// Install it *before* the listener starts. Hooks are called newest first, so the listener
/// sees each event, passes it on, and the watcher then swallows it: the events exercise the
/// listener and never type into whatever window is in front. Every swallowed event is also
/// proof that the listener passed it on.
pub(crate) struct Watcher {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl Watcher {
    fn start() -> Option<Self> {
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
            // SAFETY: a null name asks for the module of the running executable; `watch` is
            // a hook procedure of this executable; thread id 0 is what a low-level hook
            // requires.
            let hook = unsafe {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(watch),
                    GetModuleHandleW(ptr::null()),
                    0,
                )
            };
            // SAFETY: the call takes no arguments.
            let thread_id = unsafe { GetCurrentThreadId() };
            let _ = ready.send((!hook.is_null()).then_some(thread_id));
            if hook.is_null() {
                return;
            }
            // SAFETY: `message` is a valid place for one message. The loop ends with the
            // quit message (0) or an error (-1).
            while unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) } > 0 {}
            // SAFETY: `hook` came from `SetWindowsHookExW` above and is removed once.
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

/// How many key events the watcher has swallowed so far: the tests' own.
pub(crate) fn swallowed() -> usize {
    SWALLOWED.load(Ordering::SeqCst)
}

/// How many key events the watcher has let pass so far: somebody else's, or sent for real.
pub(crate) fn passed() -> usize {
    PASSED.load(Ordering::SeqCst)
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: plain values; the thread ends its loop on this message.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A watcher, if this session has a desktop that delivers synthetic input to low-level hooks.
/// Otherwise says why not and returns `None`.
pub(crate) fn desktop() -> Option<Watcher> {
    let Some(watcher) = Watcher::start() else {
        skip("this session does not allow a low-level keyboard hook");
        return None;
    };
    let before = swallowed();
    let sent = send(&[Key::release(VK_UNASSIGNED)]);
    let deadline = Instant::now() + Duration::from_secs(2);
    while swallowed() == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if sent != 1 || swallowed() == before {
        skip("this session's desktop does not deliver synthetic input (no interactive desktop)");
        return None;
    }
    Some(watcher)
}

/// The letter keys and the character each types right now, lower case: on the layout of the
/// thread of the window in front, with no key held, as the listener translates them.
pub(crate) fn letters() -> Vec<(u16, String)> {
    // SAFETY: the call takes no arguments.
    let window = unsafe { GetForegroundWindow() };
    let thread = if window.is_null() {
        0
    } else {
        // SAFETY: a stale window handle is answered with 0; the process id is not asked for.
        unsafe { GetWindowThreadProcessId(window, ptr::null_mut()) }
    };
    // SAFETY: any thread id is accepted; 0 is the calling thread.
    let mut layout: HKL = unsafe { GetKeyboardLayout(thread) };
    if layout.is_null() {
        // SAFETY: as above.
        layout = unsafe { GetKeyboardLayout(0) };
    }
    (0x41_u16..=0x5A)
        .filter_map(|vk| {
            let state = [0_u8; 256];
            let mut units = [0_u16; 8];
            // SAFETY: `state` is the 256-byte array the call reads and `units` has room for
            // the count passed. Flags 0b101: the system's keyboard state is not changed.
            let count = unsafe {
                ToUnicodeEx(
                    u32::from(vk),
                    0,
                    state.as_ptr(),
                    units.as_mut_ptr(),
                    8,
                    0b101,
                    layout,
                )
            };
            (count == 1).then(|| (vk, String::from_utf16_lossy(&units[..1]).to_lowercase()))
        })
        .collect()
}

/// What a listener reported, without sequence numbers, printable: the test data is nobody's
/// typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seen {
    /// A character, lower case (the test machine's Caps Lock may be on).
    Char(String),
    Backspace,
    Boundary(BoundaryKind),
    Started(u64),
    Finished(u64),
    Hotkey(u32),
    Overflow,
}

/// `event` in printable form. `None` for a mouse click and for `Composing`: they depend on the
/// person and the layout at the test machine.
pub(crate) fn printable(event: &ListenerEvent) -> Option<Seen> {
    Some(match event {
        ListenerEvent::Char { ch, .. } => Seen::Char(ch.expose().to_lowercase().to_string()),
        ListenerEvent::Backspace { .. } => Seen::Backspace,
        ListenerEvent::Boundary {
            kind: BoundaryKind::Click,
            ..
        }
        | ListenerEvent::Composing { .. } => return None,
        ListenerEvent::Boundary { kind, .. } => Seen::Boundary(*kind),
        ListenerEvent::InjectionStarted { expansion, .. } => Seen::Started(expansion.raw()),
        ListenerEvent::InjectionFinished { expansion, .. } => Seen::Finished(expansion.raw()),
        ListenerEvent::Hotkey { id, .. } => Seen::Hotkey(id.0),
        ListenerEvent::Overflow => Seen::Overflow,
    })
}

/// The events waiting in `events`, in order, printable.
pub(crate) fn seen(events: &Receiver<ListenerEvent>) -> Vec<Seen> {
    events
        .try_iter()
        .filter_map(|event| printable(&event))
        .collect()
}
