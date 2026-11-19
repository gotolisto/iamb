//! The injector's system calls: `SendInput` batches, the state of the modifier keys, and the
//! paste through the clipboard.

use iamb_platform::{
    ClipboardContent, ClipboardSnapshot, FocusToken, KeyTarget, PlatformError, Result,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
};

use super::plan::{Key, KeyEvent, MODIFIERS, is_extended, paste_chord, release_held};
use super::run::Desk;
use super::strategy::{PasteKeys, Strategy};
use crate::clipboard::paste::{self, PasteHost};
use crate::clipboard::{restore_now, sequence, snapshot_now, write_now};

// The markers need all 64 bits of the extra-info field; Iamb supports 64-bit Windows only.
const _: () = assert!(usize::BITS == 64);

/// An `INPUT` that carries nothing.
const ZERO: INPUT = INPUT {
    r#type: 0,
    Anonymous: INPUT_0 {
        ki: KEYBDINPUT {
            wVk: 0,
            wScan: 0,
            dwFlags: 0,
            time: 0,
            dwExtraInfo: 0,
        },
    },
};

/// The extra-info value of a marker.
#[allow(clippy::cast_possible_truncation)] // 64-bit only, asserted above.
const fn extra(marker: iamb_platform::Marker) -> usize {
    marker.encode() as usize
}

/// The `INPUT` of a Unicode unit. Makes no system call: it is used under the hand-off's lock.
const fn unicode_input(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: extra(iamb_platform::Marker::Output),
            },
        },
    }
}

/// The `INPUT` of an event, with the scan code of its virtual key.
fn input(event: &KeyEvent) -> INPUT {
    let up = if event.up { KEYEVENTF_KEYUP } else { 0 };
    let (vk, scan, flags) = match event.key {
        Key::Unicode(unit) => (0, unit, KEYEVENTF_UNICODE | up),
        Key::Vk(vk) => {
            // Applications that read scan codes (remote-desktop clients among them) need it.
            // SAFETY: plain values; an unmapped key is answered with 0.
            let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) };
            let extended = if is_extended(vk) {
                KEYEVENTF_EXTENDEDKEY
            } else {
                0
            };
            (vk, u16::try_from(scan).unwrap_or(0), extended | up)
        }
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: extra(event.marker),
            },
        },
    }
}

/// The inputs of one `SendInput` call. Overwritten with zeros when dropped: they may hold
/// text, the retyped characters among it.
pub(super) struct Inputs(Vec<INPUT>);

impl Drop for Inputs {
    fn drop(&mut self) {
        self.0.fill(ZERO);
        std::hint::black_box(&mut self.0);
    }
}

/// Sends `inputs` in one call. Fewer taken than sent means the system refused them: a window
/// of a higher privilege level is in front (UIPI), or another desktop is (a lock screen, a
/// secure desktop).
fn send_inputs(inputs: &Inputs) -> Result<()> {
    if inputs.0.is_empty() {
        return Ok(());
    }
    let count = u32::try_from(inputs.0.len()).map_err(|_| PlatformError::Invalid {
        context: "too many key events for one call",
    })?;
    let size = i32::try_from(size_of::<INPUT>()).map_err(|_| PlatformError::Unsupported)?;
    // SAFETY: `inputs` holds `count` structures of the size passed, alive for the call.
    let sent = unsafe { SendInput(count, inputs.0.as_ptr(), size) };
    if sent == count {
        Ok(())
    } else {
        Err(PlatformError::InjectionRefused)
    }
}

/// The modifier keys down right now.
fn held_modifiers() -> Vec<u16> {
    MODIFIERS
        .iter()
        .copied()
        // SAFETY: any key code is accepted; the most significant bit of the answer says
        // "down".
        .filter(|vk| unsafe { GetAsyncKeyState(i32::from(*vk)) } < 0)
        .collect()
}

/// Sends events in one call, led by the release of held modifiers.
fn send_events(events: &[KeyEvent]) -> Result<()> {
    let mut all = release_held(&held_modifiers());
    all.extend_from_slice(events);
    let inputs = Inputs(all.iter().map(input).collect());
    send_inputs(&inputs)
}

/// The system, for the protocol in `run`.
pub(super) struct SystemDesk;

impl Desk for SystemDesk {
    type Batch = Inputs;
    type Focus = Option<(u64, u32, u64)>;

    fn focus(&mut self) -> Self::Focus {
        crate::focus::keyboard_focus()
    }

    fn target_of(focus: &Self::Focus) -> Option<KeyTarget> {
        // The foreground window and its process: the two numbers the listener stamps keys
        // with (`listener::os::key_target`) and the watcher puts in its token.
        focus.map(|(window, pid, _)| KeyTarget::window(FocusToken::from_raw(pid, window)))
    }

    fn batch(&mut self, capacity: usize) -> Inputs {
        Inputs(Vec::with_capacity(capacity))
    }

    fn extend(&mut self, batch: &mut Inputs, events: &[KeyEvent]) {
        batch.0.extend(events.iter().map(input));
    }

    fn extend_chars(batch: &mut Inputs, chars: &[char]) {
        for &ch in chars {
            let mut units = [0; 2];
            for &unit in ch.encode_utf16(&mut units).iter() {
                batch.0.push(unicode_input(unit, false));
                batch.0.push(unicode_input(unit, true));
            }
            std::hint::black_box(&mut units).fill(0);
        }
    }

    fn send(&mut self, batch: Inputs) -> Result<()> {
        send_inputs(&batch)
    }

    fn held_modifiers(&mut self) -> Vec<u16> {
        held_modifiers()
    }

    fn sleep(&mut self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
    }

    fn paste(&mut self, content: &ClipboardContent, strategy: &Strategy) -> Result<()> {
        let mut host = SystemPaste {
            keys: strategy.paste_keys,
        };
        paste::paste(&mut host, content, strategy.paste_wait_ms).into_result()
    }
}

/// The clipboard and the paste shortcut, for the paste in `clipboard::paste`.
struct SystemPaste {
    keys: PasteKeys,
}

impl PasteHost for SystemPaste {
    type Saved = ClipboardSnapshot;

    fn snapshot(&mut self) -> Result<ClipboardSnapshot> {
        snapshot_now()
    }

    fn is_complete(&self, saved: &ClipboardSnapshot) -> bool {
        saved.is_complete()
    }

    fn write_transient(&mut self, content: &ClipboardContent) -> Result<u32> {
        write_now(content, true)
    }

    fn press_paste(&mut self) -> Result<()> {
        send_events(&paste_chord(self.keys))
    }

    fn wait(&mut self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
    }

    fn sequence(&mut self) -> u32 {
        sequence()
    }

    fn restore(&mut self, saved: &ClipboardSnapshot) -> Result<()> {
        restore_now(saved)
    }
}
