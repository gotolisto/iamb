//! Virtual keys, hook flags and messages as plain numbers, and what kind of key a virtual key
//! is.
//!
//! The values are written out here, not taken from the Windows bindings, so that the rest of
//! the listener's logic compiles and is tested on every system. On Windows a test compares
//! them with the bindings.

use iamb_platform::core_types::KeyName;

// Virtual-key codes (`winuser.h`).
pub(crate) const VK_BACK: u32 = 0x08;
pub(crate) const VK_TAB: u32 = 0x09;
pub(crate) const VK_RETURN: u32 = 0x0D;
pub(crate) const VK_SHIFT: u32 = 0x10;
pub(crate) const VK_CONTROL: u32 = 0x11;
pub(crate) const VK_MENU: u32 = 0x12;
pub(crate) const VK_CAPITAL: u32 = 0x14;
/// `VK_KANA`, `VK_HANGUL`: the first of the input-method mode keys.
pub(crate) const VK_KANA: u32 = 0x15;
pub(crate) const VK_ESCAPE: u32 = 0x1B;
/// `VK_MODECHANGE`: the last of the input-method mode keys below Space.
pub(crate) const VK_MODECHANGE: u32 = 0x1F;
pub(crate) const VK_SPACE: u32 = 0x20;
pub(crate) const VK_PRIOR: u32 = 0x21;
pub(crate) const VK_NEXT: u32 = 0x22;
pub(crate) const VK_END: u32 = 0x23;
pub(crate) const VK_HOME: u32 = 0x24;
pub(crate) const VK_LEFT: u32 = 0x25;
pub(crate) const VK_UP: u32 = 0x26;
pub(crate) const VK_RIGHT: u32 = 0x27;
pub(crate) const VK_DOWN: u32 = 0x28;
pub(crate) const VK_DELETE: u32 = 0x2E;
pub(crate) const VK_LWIN: u32 = 0x5B;
pub(crate) const VK_RWIN: u32 = 0x5C;
#[cfg(test)]
pub(crate) const VK_F1: u32 = 0x70;
#[cfg(test)]
pub(crate) const VK_F10: u32 = 0x79;
pub(crate) const VK_NUMLOCK: u32 = 0x90;
pub(crate) const VK_SCROLL: u32 = 0x91;
pub(crate) const VK_LSHIFT: u32 = 0xA0;
pub(crate) const VK_RSHIFT: u32 = 0xA1;
pub(crate) const VK_LCONTROL: u32 = 0xA2;
pub(crate) const VK_RCONTROL: u32 = 0xA3;
pub(crate) const VK_LMENU: u32 = 0xA4;
pub(crate) const VK_RMENU: u32 = 0xA5;
pub(crate) const VK_PROCESSKEY: u32 = 0xE5;
pub(crate) const VK_PACKET: u32 = 0xE7;
/// An unassigned virtual key: no keyboard has it and no application reacts to it. The
/// self-test's no-op event is a release of this key.
pub(crate) const VK_UNASSIGNED: u32 = 0xE8;
/// `VK_DBE_ALPHANUMERIC` to `VK_DBE_NOROMAN`: the mode keys of Japanese keyboards.
pub(crate) const VK_DBE_FIRST: u32 = 0xF0;
pub(crate) const VK_DBE_LAST: u32 = 0xF6;
/// Not a key: drivers use it for events that belong to no key.
pub(crate) const VK_NONE: u32 = 0xFF;

// `KBDLLHOOKSTRUCT.flags`.
pub(crate) const LLKHF_INJECTED: u32 = 0x10;
pub(crate) const LLKHF_UP: u32 = 0x80;

// `MSLLHOOKSTRUCT.flags`.
pub(crate) const LLMHF_INJECTED: u32 = 0x01;

// The message a low-level hook is called with.
#[cfg(test)]
pub(crate) const WM_KEYDOWN: u32 = 0x0100;
#[cfg(test)]
pub(crate) const WM_KEYUP: u32 = 0x0101;
pub(crate) const WM_SYSKEYDOWN: u32 = 0x0104;
#[cfg(test)]
pub(crate) const WM_SYSKEYUP: u32 = 0x0105;
pub(crate) const WM_LBUTTONDOWN: u32 = 0x0201;
pub(crate) const WM_RBUTTONDOWN: u32 = 0x0204;
pub(crate) const WM_MBUTTONDOWN: u32 = 0x0207;
pub(crate) const WM_XBUTTONDOWN: u32 = 0x020B;

/// What kind of key a virtual key is, before the layout is asked what it types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyClass {
    /// Shift, Ctrl, Alt, the Windows key, or a lock key: tracked, never reported.
    State,
    /// A key that switches or drives an input method (the mode keys, `VK_PROCESSKEY`).
    InputMethod,
    /// `VK_PACKET`: a character another program injected; the scan code is a UTF-16 unit.
    Packet,
    /// Not a key at all (`0`, `VK_NONE`).
    NoKey,
    /// A key the template language names; reported with
    /// [`ListenerEvent::named_key`](iamb_platform::ListenerEvent::named_key) when it types
    /// nothing.
    Named(KeyName),
    /// Any other key: a character key, a function key, a media key.
    Other,
}

/// The kind of `vk`.
pub(crate) const fn classify(vk: u32) -> KeyClass {
    match vk {
        0 | VK_NONE => KeyClass::NoKey,
        VK_SHIFT
        | VK_CONTROL
        | VK_MENU
        | VK_CAPITAL
        | VK_LWIN
        | VK_RWIN
        | VK_NUMLOCK
        | VK_SCROLL
        | VK_LSHIFT..=VK_RMENU => KeyClass::State,
        VK_KANA..VK_ESCAPE | 0x1C..=VK_MODECHANGE | VK_PROCESSKEY | VK_DBE_FIRST..=VK_DBE_LAST => {
            KeyClass::InputMethod
        }
        VK_PACKET => KeyClass::Packet,
        VK_RETURN => KeyClass::Named(KeyName::Enter),
        VK_TAB => KeyClass::Named(KeyName::Tab),
        VK_ESCAPE => KeyClass::Named(KeyName::Escape),
        VK_BACK => KeyClass::Named(KeyName::Backspace),
        VK_DELETE => KeyClass::Named(KeyName::Delete),
        VK_SPACE => KeyClass::Named(KeyName::Space),
        VK_UP => KeyClass::Named(KeyName::Up),
        VK_DOWN => KeyClass::Named(KeyName::Down),
        VK_LEFT => KeyClass::Named(KeyName::Left),
        VK_RIGHT => KeyClass::Named(KeyName::Right),
        VK_HOME => KeyClass::Named(KeyName::Home),
        VK_END => KeyClass::Named(KeyName::End),
        VK_PRIOR => KeyClass::Named(KeyName::PageUp),
        VK_NEXT => KeyClass::Named(KeyName::PageDown),
        _ => KeyClass::Other,
    }
}

/// True for the messages of a mouse button going down.
pub(crate) const fn is_button_down(message: u32) -> bool {
    matches!(
        message,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_keys_are_never_keys_of_their_own() {
        for vk in [
            VK_SHIFT,
            VK_CONTROL,
            VK_MENU,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_LMENU,
            VK_RMENU,
            VK_LWIN,
            VK_RWIN,
            VK_CAPITAL,
            VK_NUMLOCK,
            VK_SCROLL,
        ] {
            assert_eq!(classify(vk), KeyClass::State, "{vk:#x}");
        }
    }

    #[test]
    fn every_named_key_of_the_template_language_has_a_virtual_key() {
        let named: Vec<KeyName> = (0..=0xFF)
            .filter_map(|vk| match classify(vk) {
                KeyClass::Named(key) => Some(key),
                _ => None,
            })
            .collect();
        assert_eq!(named.len(), KeyName::ALL.len(), "one virtual key each");
        for key in KeyName::ALL {
            assert!(named.contains(&key), "{key:?}");
        }
        assert_eq!(classify(VK_RETURN), KeyClass::Named(KeyName::Enter));
        assert_eq!(classify(VK_BACK), KeyClass::Named(KeyName::Backspace));
        assert_eq!(classify(VK_PRIOR), KeyClass::Named(KeyName::PageUp));
    }

    #[test]
    fn input_method_keys_packets_and_non_keys() {
        // Kana/Hangul, IME on, Junja, Final, Hanja/Kanji, IME off.
        for vk in 0x15..=0x1A {
            assert_eq!(classify(vk), KeyClass::InputMethod, "{vk:#x}");
        }
        // Convert, non-convert, accept, mode change.
        for vk in 0x1C..=0x1F {
            assert_eq!(classify(vk), KeyClass::InputMethod, "{vk:#x}");
        }
        for vk in VK_DBE_FIRST..=VK_DBE_LAST {
            assert_eq!(classify(vk), KeyClass::InputMethod, "{vk:#x}");
        }
        assert_eq!(classify(VK_PROCESSKEY), KeyClass::InputMethod);
        assert_eq!(classify(VK_ESCAPE), KeyClass::Named(KeyName::Escape));
        assert_eq!(classify(VK_PACKET), KeyClass::Packet);
        assert_eq!(classify(0), KeyClass::NoKey);
        assert_eq!(classify(VK_NONE), KeyClass::NoKey);
    }

    #[test]
    fn everything_else_is_an_ordinary_key() {
        for vk in [
            0x30,
            0x41,
            0x5A,
            0x60,
            VK_F1,
            VK_F10,
            0x87,
            0xBA,
            0xDF,
            0xE2,
            VK_UNASSIGNED,
        ] {
            assert_eq!(classify(vk), KeyClass::Other, "{vk:#x}");
        }
    }

    #[test]
    fn button_presses_but_not_releases_or_moves() {
        for message in [
            WM_LBUTTONDOWN,
            WM_RBUTTONDOWN,
            WM_MBUTTONDOWN,
            WM_XBUTTONDOWN,
        ] {
            assert!(is_button_down(message));
        }
        // Mouse move, left up, wheel.
        for message in [0x0200, 0x0202, 0x020A, WM_KEYDOWN] {
            assert!(!is_button_down(message));
        }
    }
}
