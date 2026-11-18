//! The system calls of the listener and of the hotkeys, each behind a small safe function.
//!
//! Apart from the layout files (`layouts.rs`) and the two hook procedures' reads of the
//! event they are called with (`thread.rs`), every `unsafe` block of the listener is here.
//! None of these functions waits for another thread.

use std::ptr;

use iamb_platform::{Marker, PlatformError, Result};
use windows_sys::Win32::Foundation::{
    ERROR_HOTKEY_ALREADY_REGISTERED, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, GetKeyboardLayoutList, HKL, INPUT, INPUT_0,
    INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, RegisterHotKey, SendInput, ToUnicodeEx,
    UnregisterHotKey,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SendMessageCallbackW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_APP, WM_HOTKEY, WM_IME_CONTROL, WM_QUIT, WM_USER,
};

use super::deadkey::{KeyQuery, MAX_UNITS, Translation};
use super::keys::{VK_CAPITAL, VK_CONTROL, VK_MENU, VK_SHIFT};

/// Thread message: commands are waiting in the listener thread's channel.
pub(super) const WM_WAKE: u32 = WM_APP + 1;
/// Thread message the listener thread posts to itself: read the dead-key table of the layout
/// whose handle is the message's first parameter.
pub(super) const WM_LAYOUT: u32 = WM_APP + 2;
/// Ends the listener thread's message loop.
pub(super) const WM_STOP: u32 = WM_QUIT;

/// `WM_IME_CONTROL` sub-command: is the input method open?
const IMC_GETOPENSTATUS: WPARAM = 0x0005;

/// `ToUnicodeEx` flags. Bit 2: the keyboard state is not changed (Windows 10 version 1607 and
/// later), so the application's dead keys are left alone. Bit 0: "a menu is active", in which
/// mode Alt+numeric-keypad entry is not handled, so a translation can never add a digit to
/// the character code the user is entering that way.
const TRANSLATE_FLAGS: u32 = 0b101;

/// In a keyboard-state array: the key is down.
const KEY_DOWN: u8 = 0x80;
/// In a keyboard-state array: the key is toggled on.
const KEY_TOGGLED: u8 = 0x01;

/// The signature of a hook procedure.
pub(super) type HookProcedure = unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT;

/// The signature of the procedure that receives an input method's answer.
pub(super) type AnswerProcedure = unsafe extern "system" fn(HWND, u32, usize, LRESULT);

/// The calling thread's last error code.
pub(crate) fn last_error() -> i64 {
    // SAFETY: the call takes no arguments and reads a value of the calling thread.
    i64::from(unsafe { GetLastError() })
}

/// The error for a failed call named `context`, with the calling thread's last error code.
pub(crate) fn os_error(context: &'static str) -> PlatformError {
    PlatformError::os(last_error(), context)
}

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

/// Which low-level hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HookKind {
    /// `WH_KEYBOARD_LL`.
    Keyboard,
    /// `WH_MOUSE_LL`.
    Mouse,
}

/// An installed low-level hook. Removed when dropped; [`remove`](Self::remove) also says
/// whether the system agreed.
#[derive(Debug)]
pub(super) struct Hook(HHOOK);

impl Hook {
    /// Installs a low-level hook whose procedure is called on the calling thread, while that
    /// thread retrieves messages.
    pub(super) fn install(kind: HookKind, procedure: HookProcedure) -> Result<Self> {
        let id = match kind {
            HookKind::Keyboard => WH_KEYBOARD_LL,
            HookKind::Mouse => WH_MOUSE_LL,
        };
        // SAFETY: a null name asks for the module of the running executable; the call keeps
        // no pointer.
        let module = unsafe { GetModuleHandleW(ptr::null()) };
        // SAFETY: `procedure` is a function of this executable with the signature of a hook
        // procedure and lives as long as the process; thread id 0 is what a low-level hook
        // requires.
        let hook = unsafe { SetWindowsHookExW(id, Some(procedure), module, 0) };
        if hook.is_null() {
            Err(os_error("SetWindowsHookExW"))
        } else {
            Ok(Self(hook))
        }
    }

    fn unhook(&mut self) -> Result<()> {
        let hook = std::mem::replace(&mut self.0, ptr::null_mut());
        if hook.is_null() {
            return Ok(());
        }
        // SAFETY: `hook` came from `SetWindowsHookExW` and is passed here once: the field is
        // null from now on. A handle the system has already removed is refused, not misused.
        if unsafe { UnhookWindowsHookEx(hook) } == 0 {
            Err(os_error("UnhookWindowsHookEx"))
        } else {
            Ok(())
        }
    }

    /// Removes the hook.
    pub(super) fn remove(mut self) -> Result<()> {
        self.unhook()
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // Nothing can be done about a refusal here.
        let _ = self.unhook();
    }
}

/// Hands a hook call to the hooks installed before this one and returns their verdict.
pub(super) fn call_next_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the arguments are the ones the system called this thread's hook procedure
    // with, unchanged; the hook handle is ignored by the system and may be null.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// The event a low-level keyboard hook was called with.
///
/// # Safety
///
/// `lparam` must be the third argument of a `WH_KEYBOARD_LL` hook call whose code is
/// `HC_ACTION`, and that call must not have returned yet.
pub(super) unsafe fn keyboard_event(lparam: LPARAM) -> KBDLLHOOKSTRUCT {
    // SAFETY: by the caller's contract `lparam` is the address of a `KBDLLHOOKSTRUCT` that the
    // system keeps valid for the duration of the hook call.
    unsafe { ptr::read(lparam as *const KBDLLHOOKSTRUCT) }
}

/// The event a low-level mouse hook was called with.
///
/// # Safety
///
/// `lparam` must be the third argument of a `WH_MOUSE_LL` hook call whose code is
/// `HC_ACTION`, and that call must not have returned yet.
pub(super) unsafe fn mouse_event(lparam: LPARAM) -> MSLLHOOKSTRUCT {
    // SAFETY: by the caller's contract `lparam` is the address of an `MSLLHOOKSTRUCT` that the
    // system keeps valid for the duration of the hook call.
    unsafe { ptr::read(lparam as *const MSLLHOOKSTRUCT) }
}

/// A message of the listener thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Message {
    /// The loop must end.
    Stop,
    /// Retrieving a message failed, with this error code.
    Failed(i64),
    /// The hotkey with this system id was pressed.
    Hotkey(i32),
    /// Commands are waiting.
    Wake,
    /// Read the dead-key table of this layout.
    Layout(usize),
    /// Anything else.
    Other,
}

/// Waits for the next message of the calling thread. Hook procedures and input-method answers
/// are called from inside this wait.
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
        // The listener creates no window, but the system may give the thread one of its own.
        // SAFETY: `message` was filled in by `GetMessageW` just now.
        unsafe { DispatchMessageW(&raw const message) };
        return Message::Other;
    }
    match message.message {
        WM_HOTKEY => Message::Hotkey(i32::try_from(message.wParam).unwrap_or(-1)),
        WM_WAKE => Message::Wake,
        WM_LAYOUT => Message::Layout(message.wParam),
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

/// The window in front.
#[derive(Debug, Clone, Copy)]
pub(super) struct Front {
    /// Its thread; 0 when there is no window.
    pub(super) thread: u32,
    /// The keyboard layout of that thread (of the calling thread when there is no window);
    /// 0 when the system does not say.
    pub(super) layout: usize,
}

/// The window in front, its thread and that thread's keyboard layout.
pub(super) fn foreground() -> Front {
    // SAFETY: the call takes no arguments.
    let window = unsafe { GetForegroundWindow() };
    let thread = if window.is_null() {
        0
    } else {
        // SAFETY: a window handle that is no longer valid is answered with 0; the pointer for
        // the process id is optional and null.
        unsafe { GetWindowThreadProcessId(window, ptr::null_mut()) }
    };
    Front {
        thread,
        layout: layout_of_thread(thread),
    }
}

/// The keyboard layout handle of `thread`; of the calling thread for 0.
pub(super) fn layout_of_thread(thread: u32) -> usize {
    // SAFETY: any thread id is accepted; an unknown one is answered with null.
    unsafe { GetKeyboardLayout(thread) as usize }
}

/// The keyboard layouts loaded for the user.
pub(super) fn keyboard_layouts() -> Vec<usize> {
    const MOST: usize = 64;
    let mut handles: [HKL; MOST] = [ptr::null_mut(); MOST];
    // SAFETY: the buffer holds `MOST` handles and that is the count passed.
    let count = unsafe { GetKeyboardLayoutList(64, handles.as_mut_ptr()) };
    let count = usize::try_from(count).unwrap_or(0).min(MOST);
    handles[..count]
        .iter()
        .map(|handle| *handle as usize)
        .collect()
}

/// What `key` produces on `layout`, in the mode that leaves the system's keyboard state as it
/// is (see [`TRANSLATE_FLAGS`]).
pub(super) fn translate(layout: usize, key: &KeyQuery) -> Translation {
    let mut state = [0_u8; 256];
    for vk in key.keys.held() {
        if let Some(slot) = state.get_mut(vk as usize) {
            *slot = KEY_DOWN;
        }
    }
    // Layouts name the modifiers without a side.
    for (held, vk) in [
        (key.keys.shift(), VK_SHIFT),
        (key.keys.ctrl(), VK_CONTROL),
        (key.keys.alt(), VK_MENU),
    ] {
        if held {
            state[vk as usize] = KEY_DOWN;
        }
    }
    if key.keys.caps() {
        state[VK_CAPITAL as usize] |= KEY_TOGGLED;
    }
    let mut units = [0_u16; MAX_UNITS];
    // SAFETY: `state` is the 256-byte array the call reads; `units` has room for the number
    // of units passed; the layout handle is looked up by the system, never dereferenced, and
    // an unknown one falls back to the calling thread's layout.
    let count = unsafe {
        ToUnicodeEx(
            key.vk,
            // Bit 15 would mean "key up"; only key presses are translated.
            key.scan & 0x7FFF,
            state.as_ptr(),
            units.as_mut_ptr(),
            // A count of 0 (never the case) would make the call write nothing.
            i32::try_from(MAX_UNITS).unwrap_or(0),
            TRANSLATE_FLAGS,
            layout as HKL,
        )
    };
    match usize::try_from(count) {
        Ok(0) => Translation::None,
        Ok(count) => Translation::text(&units[..count.min(MAX_UNITS)]),
        // Negative: a dead key, with its character in the buffer.
        Err(_) => Translation::Dead(units[0]),
    }
}

/// Whether the key `vk` is physically down. Not valid for the key of a hook call in progress.
pub(super) fn physically_down(vk: u32) -> bool {
    let Ok(vk) = i32::try_from(vk) else {
        return false;
    };
    // SAFETY: any key code is accepted.
    let state = unsafe { GetAsyncKeyState(vk) };
    // The most significant bit says "down".
    state < 0
}

/// Whether Caps Lock is on.
///
/// The toggle is only readable through the calling thread's own input state, which is a copy
/// of the system's when the thread is created and goes stale on a thread that receives no
/// keyboard messages. So the question is asked on a thread created for it.
pub(super) fn caps_lock_on() -> bool {
    std::thread::spawn(|| {
        // SAFETY: any key code is accepted. The least significant bit of the answer is the
        // toggle.
        let state = unsafe { GetKeyState(VK_CAPITAL.cast_signed()) };
        state & 1 != 0
    })
    .join()
    .unwrap_or(false)
}

/// Posts one key release of `vk` through the system's input stream, marked with `marker` in
/// its extra-info field.
pub(crate) fn send_marked_key_up(vk: u32, marker: Marker) -> Result<()> {
    // The marker needs all 64 bits; Iamb supports 64-bit Windows only.
    let extra = usize::try_from(marker.encode()).map_err(|_| PlatformError::Unsupported)?;
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: u16::try_from(vk).map_err(|_| PlatformError::Invalid {
                    context: "virtual key out of range",
                })?,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: extra,
            },
        },
    };
    let size = i32::try_from(size_of::<INPUT>()).map_err(|_| PlatformError::Unsupported)?;
    // SAFETY: one `INPUT` is passed, with its count and its size.
    let sent = unsafe { SendInput(1, &raw const input, size) };
    if sent == 1 {
        Ok(())
    } else {
        Err(os_error("SendInput"))
    }
}

/// Registers a hotkey for the calling thread: its presses arrive as [`Message::Hotkey`] in
/// that thread's queue.
pub(crate) fn register_hotkey(os_id: i32, modifier_flags: u32, vk: u32) -> Result<()> {
    // SAFETY: a null window ties the hotkey to the calling thread; the rest are plain values.
    if unsafe { RegisterHotKey(ptr::null_mut(), os_id, modifier_flags, vk) } != 0 {
        return Ok(());
    }
    // SAFETY: the call takes no arguments and reads a value of the calling thread.
    match unsafe { GetLastError() } {
        ERROR_HOTKEY_ALREADY_REGISTERED => Err(PlatformError::HotkeyUnavailable),
        code => Err(PlatformError::os(i64::from(code), "RegisterHotKey")),
    }
}

/// Removes a hotkey the calling thread registered.
pub(crate) fn unregister_hotkey(os_id: i32) -> Result<()> {
    // SAFETY: a null window means "a hotkey of the calling thread"; an id that is not
    // registered is refused.
    if unsafe { UnregisterHotKey(ptr::null_mut(), os_id) } == 0 {
        Err(os_error("UnregisterHotKey"))
    } else {
        Ok(())
    }
}

/// Asks the input method of the window in front whether it is open. The call does not wait:
/// `answer` is called on the calling thread, from inside [`next_message`], with `id` and a
/// result that is not zero for "open". False when the question could not be sent.
pub(super) fn ask_input_method(id: u32, answer: AnswerProcedure) -> bool {
    // SAFETY: the call takes no arguments.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return false;
    }
    // SAFETY: a window handle that is no longer valid is answered with null.
    let input_method = unsafe { ImmGetDefaultIMEWnd(window) };
    if input_method.is_null() {
        return false;
    }
    // SAFETY: the message carries no pointer; `answer` is a function of this executable with
    // the callback's signature and lives as long as the process. The window belongs to
    // another thread (the listener thread has none), so the call returns at once.
    let sent = unsafe {
        SendMessageCallbackW(
            input_method,
            WM_IME_CONTROL,
            IMC_GETOPENSTATUS,
            0,
            Some(answer),
            id as usize,
        )
    };
    sent != 0
}
