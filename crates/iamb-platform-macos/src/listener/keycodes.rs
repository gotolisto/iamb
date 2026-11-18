//! macOS virtual key codes and what kind of key each one is.
//!
//! A key code names a position on the keyboard, not a character: `ANSI_A` is the key that
//! types `a` on a US keyboard and `q` on a French one. Which character a key types is the
//! layout's business ([`translate`](super::translate)); this module only knows the keys that
//! mean the same thing on every layout (Return, the arrows, the function keys, the modifiers).
//!
//! The values are the `kVK_*` constants of `HIToolbox/Events.h`.

use iamb_platform::HotkeyKey;
use iamb_platform::core_types::KeyName;

/// The `kVK_*` constants this crate uses.
pub(crate) mod vk {
    pub(crate) const ANSI_A: u16 = 0x00;
    pub(crate) const ANSI_S: u16 = 0x01;
    pub(crate) const ANSI_D: u16 = 0x02;
    pub(crate) const ANSI_F: u16 = 0x03;
    pub(crate) const ANSI_H: u16 = 0x04;
    pub(crate) const ANSI_G: u16 = 0x05;
    pub(crate) const ANSI_Z: u16 = 0x06;
    pub(crate) const ANSI_X: u16 = 0x07;
    pub(crate) const ANSI_C: u16 = 0x08;
    pub(crate) const ANSI_V: u16 = 0x09;
    pub(crate) const ANSI_B: u16 = 0x0B;
    pub(crate) const ANSI_Q: u16 = 0x0C;
    pub(crate) const ANSI_W: u16 = 0x0D;
    pub(crate) const ANSI_E: u16 = 0x0E;
    pub(crate) const ANSI_R: u16 = 0x0F;
    pub(crate) const ANSI_Y: u16 = 0x10;
    pub(crate) const ANSI_T: u16 = 0x11;
    pub(crate) const ANSI_1: u16 = 0x12;
    pub(crate) const ANSI_2: u16 = 0x13;
    pub(crate) const ANSI_3: u16 = 0x14;
    pub(crate) const ANSI_4: u16 = 0x15;
    pub(crate) const ANSI_6: u16 = 0x16;
    pub(crate) const ANSI_5: u16 = 0x17;
    pub(crate) const ANSI_9: u16 = 0x19;
    pub(crate) const ANSI_7: u16 = 0x1A;
    pub(crate) const ANSI_8: u16 = 0x1C;
    pub(crate) const ANSI_0: u16 = 0x1D;
    pub(crate) const ANSI_O: u16 = 0x1F;
    pub(crate) const ANSI_U: u16 = 0x20;
    pub(crate) const ANSI_I: u16 = 0x22;
    pub(crate) const ANSI_P: u16 = 0x23;
    pub(crate) const RETURN: u16 = 0x24;
    pub(crate) const ANSI_L: u16 = 0x25;
    pub(crate) const ANSI_J: u16 = 0x26;
    pub(crate) const ANSI_K: u16 = 0x28;
    pub(crate) const ANSI_N: u16 = 0x2D;
    pub(crate) const ANSI_M: u16 = 0x2E;
    pub(crate) const TAB: u16 = 0x30;
    pub(crate) const SPACE: u16 = 0x31;
    /// The key labelled "delete" on a Mac keyboard: it deletes backwards.
    pub(crate) const DELETE: u16 = 0x33;
    pub(crate) const ESCAPE: u16 = 0x35;
    pub(crate) const RIGHT_COMMAND: u16 = 0x36;
    pub(crate) const COMMAND: u16 = 0x37;
    pub(crate) const SHIFT: u16 = 0x38;
    pub(crate) const CAPS_LOCK: u16 = 0x39;
    pub(crate) const OPTION: u16 = 0x3A;
    pub(crate) const CONTROL: u16 = 0x3B;
    pub(crate) const RIGHT_SHIFT: u16 = 0x3C;
    pub(crate) const RIGHT_OPTION: u16 = 0x3D;
    pub(crate) const RIGHT_CONTROL: u16 = 0x3E;
    pub(crate) const FUNCTION: u16 = 0x3F;
    pub(crate) const F17: u16 = 0x40;
    pub(crate) const KEYPAD_CLEAR: u16 = 0x47;
    pub(crate) const KEYPAD_ENTER: u16 = 0x4C;
    pub(crate) const F18: u16 = 0x4F;
    pub(crate) const F19: u16 = 0x50;
    pub(crate) const F20: u16 = 0x5A;
    pub(crate) const F5: u16 = 0x60;
    pub(crate) const F6: u16 = 0x61;
    pub(crate) const F7: u16 = 0x62;
    pub(crate) const F3: u16 = 0x63;
    pub(crate) const F8: u16 = 0x64;
    pub(crate) const F9: u16 = 0x65;
    pub(crate) const F11: u16 = 0x67;
    pub(crate) const F13: u16 = 0x69;
    pub(crate) const F16: u16 = 0x6A;
    pub(crate) const F14: u16 = 0x6B;
    pub(crate) const F10: u16 = 0x6D;
    pub(crate) const F12: u16 = 0x6F;
    pub(crate) const F15: u16 = 0x71;
    pub(crate) const HELP: u16 = 0x72;
    pub(crate) const HOME: u16 = 0x73;
    pub(crate) const PAGE_UP: u16 = 0x74;
    pub(crate) const FORWARD_DELETE: u16 = 0x75;
    pub(crate) const F4: u16 = 0x76;
    pub(crate) const END: u16 = 0x77;
    pub(crate) const F2: u16 = 0x78;
    pub(crate) const PAGE_DOWN: u16 = 0x79;
    pub(crate) const F1: u16 = 0x7A;
    pub(crate) const LEFT_ARROW: u16 = 0x7B;
    pub(crate) const RIGHT_ARROW: u16 = 0x7C;
    pub(crate) const DOWN_ARROW: u16 = 0x7D;
    pub(crate) const UP_ARROW: u16 = 0x7E;
}

/// The function keys in order: `FUNCTION_KEYS[0]` is F1. A Mac keyboard has no F21 to F24.
const FUNCTION_KEYS: [u16; 20] = [
    vk::F1,
    vk::F2,
    vk::F3,
    vk::F4,
    vk::F5,
    vk::F6,
    vk::F7,
    vk::F8,
    vk::F9,
    vk::F10,
    vk::F11,
    vk::F12,
    vk::F13,
    vk::F14,
    vk::F15,
    vk::F16,
    vk::F17,
    vk::F18,
    vk::F19,
    vk::F20,
];

/// The letter keys at their positions on the US (ANSI) keyboard: `LETTER_KEYS[0]` is A.
const LETTER_KEYS: [u16; 26] = [
    vk::ANSI_A,
    vk::ANSI_B,
    vk::ANSI_C,
    vk::ANSI_D,
    vk::ANSI_E,
    vk::ANSI_F,
    vk::ANSI_G,
    vk::ANSI_H,
    vk::ANSI_I,
    vk::ANSI_J,
    vk::ANSI_K,
    vk::ANSI_L,
    vk::ANSI_M,
    vk::ANSI_N,
    vk::ANSI_O,
    vk::ANSI_P,
    vk::ANSI_Q,
    vk::ANSI_R,
    vk::ANSI_S,
    vk::ANSI_T,
    vk::ANSI_U,
    vk::ANSI_V,
    vk::ANSI_W,
    vk::ANSI_X,
    vk::ANSI_Y,
    vk::ANSI_Z,
];

/// The digit keys of the main row: `DIGIT_KEYS[0]` is 0.
const DIGIT_KEYS: [u16; 10] = [
    vk::ANSI_0,
    vk::ANSI_1,
    vk::ANSI_2,
    vk::ANSI_3,
    vk::ANSI_4,
    vk::ANSI_5,
    vk::ANSI_6,
    vk::ANSI_7,
    vk::ANSI_8,
    vk::ANSI_9,
];

/// What a key is before any keyboard layout is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyClass {
    /// A key with the same meaning on every layout that never types text: Return, Tab, Escape,
    /// delete, forward delete, the arrows and the navigation keys.
    Named(KeyName),
    /// A function key, Help or the keypad's Clear: never text, and no name of its own in the
    /// engine's vocabulary.
    Function,
    /// Shift, Control, Option, Command, Caps Lock or Fn. These arrive as flag changes, not as
    /// key presses.
    Modifier,
    /// Any other key, the space bar included: the layout decides what it types.
    Layout,
}

/// Classifies a key code.
pub(crate) const fn classify(code: u16) -> KeyClass {
    let name = match code {
        vk::RETURN | vk::KEYPAD_ENTER => KeyName::Enter,
        vk::TAB => KeyName::Tab,
        vk::ESCAPE => KeyName::Escape,
        vk::DELETE => KeyName::Backspace,
        vk::FORWARD_DELETE => KeyName::Delete,
        vk::UP_ARROW => KeyName::Up,
        vk::DOWN_ARROW => KeyName::Down,
        vk::LEFT_ARROW => KeyName::Left,
        vk::RIGHT_ARROW => KeyName::Right,
        vk::HOME => KeyName::Home,
        vk::END => KeyName::End,
        vk::PAGE_UP => KeyName::PageUp,
        vk::PAGE_DOWN => KeyName::PageDown,
        vk::COMMAND
        | vk::RIGHT_COMMAND
        | vk::SHIFT
        | vk::RIGHT_SHIFT
        | vk::OPTION
        | vk::RIGHT_OPTION
        | vk::CONTROL
        | vk::RIGHT_CONTROL
        | vk::CAPS_LOCK
        | vk::FUNCTION => return KeyClass::Modifier,
        vk::HELP | vk::KEYPAD_CLEAR => return KeyClass::Function,
        _ => {
            return if is_function_key(code) {
                KeyClass::Function
            } else {
                KeyClass::Layout
            };
        }
    };
    KeyClass::Named(name)
}

const fn is_function_key(code: u16) -> bool {
    let mut i = 0;
    while i < FUNCTION_KEYS.len() {
        if FUNCTION_KEYS[i] == code {
            return true;
        }
        i += 1;
    }
    false
}

/// The key code the system hotkey API wants for the key of a chord; `None` when a Mac keyboard
/// has no such key (F21 to F24).
///
/// Letters and digits are their positions on the US keyboard, which is how macOS itself
/// identifies the key of a shortcut. On a layout that moves letters (AZERTY, QWERTZ, Dvorak)
/// the chord therefore sits on the key in that position, not on the key that types the
/// letter. Recorded as a known limit of this task.
pub(crate) fn hotkey_code(key: HotkeyKey) -> Option<u16> {
    match key {
        HotkeyKey::Letter(letter) => {
            let index = u32::from(letter.to_ascii_uppercase()).checked_sub(u32::from('A'))?;
            LETTER_KEYS.get(usize::try_from(index).ok()?).copied()
        }
        HotkeyKey::Digit(digit) => DIGIT_KEYS.get(usize::from(digit)).copied(),
        HotkeyKey::Function(number) => FUNCTION_KEYS
            .get(usize::from(number).checked_sub(1)?)
            .copied(),
        HotkeyKey::Named(name) => Some(match name {
            KeyName::Enter => vk::RETURN,
            KeyName::Tab => vk::TAB,
            KeyName::Escape => vk::ESCAPE,
            KeyName::Backspace => vk::DELETE,
            KeyName::Delete => vk::FORWARD_DELETE,
            KeyName::Space => vk::SPACE,
            KeyName::Up => vk::UP_ARROW,
            KeyName::Down => vk::DOWN_ARROW,
            KeyName::Left => vk::LEFT_ARROW,
            KeyName::Right => vk::RIGHT_ARROW,
            KeyName::Home => vk::HOME,
            KeyName::End => vk::END,
            KeyName::PageUp => vk::PAGE_UP,
            KeyName::PageDown => vk::PAGE_DOWN,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn named_keys_mean_the_same_on_every_layout() {
        assert_eq!(classify(vk::RETURN), KeyClass::Named(KeyName::Enter));
        assert_eq!(classify(vk::KEYPAD_ENTER), KeyClass::Named(KeyName::Enter));
        assert_eq!(classify(vk::TAB), KeyClass::Named(KeyName::Tab));
        assert_eq!(classify(vk::ESCAPE), KeyClass::Named(KeyName::Escape));
        // The Mac "delete" key deletes backwards; forward delete is the other one.
        assert_eq!(classify(vk::DELETE), KeyClass::Named(KeyName::Backspace));
        assert_eq!(
            classify(vk::FORWARD_DELETE),
            KeyClass::Named(KeyName::Delete)
        );
        for (code, name) in [
            (vk::UP_ARROW, KeyName::Up),
            (vk::DOWN_ARROW, KeyName::Down),
            (vk::LEFT_ARROW, KeyName::Left),
            (vk::RIGHT_ARROW, KeyName::Right),
            (vk::HOME, KeyName::Home),
            (vk::END, KeyName::End),
            (vk::PAGE_UP, KeyName::PageUp),
            (vk::PAGE_DOWN, KeyName::PageDown),
        ] {
            assert_eq!(classify(code), KeyClass::Named(name));
        }
    }

    #[test]
    fn the_layout_decides_for_everything_else() {
        // The space bar is the layout's too: Option+Space types a no-break space.
        for code in [vk::ANSI_A, vk::ANSI_L, vk::ANSI_0, vk::SPACE, 0x0A, 0x32] {
            assert_eq!(classify(code), KeyClass::Layout);
        }
        // A code no keyboard sends is not a reason to fail.
        assert_eq!(classify(0x1FF), KeyClass::Layout);
        assert_eq!(classify(u16::MAX), KeyClass::Layout);
    }

    #[test]
    fn function_keys_and_modifiers_never_type() {
        for code in FUNCTION_KEYS {
            assert_eq!(classify(code), KeyClass::Function);
        }
        assert_eq!(classify(vk::HELP), KeyClass::Function);
        assert_eq!(classify(vk::KEYPAD_CLEAR), KeyClass::Function);
        for code in [
            vk::COMMAND,
            vk::RIGHT_COMMAND,
            vk::SHIFT,
            vk::RIGHT_SHIFT,
            vk::OPTION,
            vk::RIGHT_OPTION,
            vk::CONTROL,
            vk::RIGHT_CONTROL,
            vk::CAPS_LOCK,
            vk::FUNCTION,
        ] {
            assert_eq!(classify(code), KeyClass::Modifier);
        }
    }

    #[test]
    fn every_hotkey_key_has_its_own_code() {
        let mut seen = BTreeSet::new();
        for letter in 'A'..='Z' {
            let code = hotkey_code(HotkeyKey::Letter(letter)).expect("a letter key");
            assert_eq!(
                hotkey_code(HotkeyKey::Letter(letter.to_ascii_lowercase())),
                Some(code)
            );
            assert!(seen.insert(code), "two keys share a code");
        }
        for digit in 0..=9 {
            assert!(seen.insert(hotkey_code(HotkeyKey::Digit(digit)).expect("a digit key")));
        }
        for number in 1..=20 {
            assert!(seen.insert(hotkey_code(HotkeyKey::Function(number)).expect("a function key")));
        }
        for name in KeyName::ALL {
            assert!(seen.insert(hotkey_code(HotkeyKey::Named(name)).expect("a named key")));
        }
        assert_eq!(seen.len(), 26 + 10 + 20 + 14);
        assert!(seen.iter().all(|code| *code < 0x80));
    }

    #[test]
    fn keys_a_mac_keyboard_lacks_have_no_code() {
        for number in [0, 21, 22, 23, 24, 25, u8::MAX] {
            assert_eq!(hotkey_code(HotkeyKey::Function(number)), None);
        }
        assert_eq!(hotkey_code(HotkeyKey::Digit(10)), None);
        assert_eq!(hotkey_code(HotkeyKey::Letter('1')), None);
        assert_eq!(hotkey_code(HotkeyKey::Letter('é')), None);
    }

    #[test]
    fn a_few_positions_are_what_the_system_headers_say() {
        assert_eq!(hotkey_code(HotkeyKey::Letter('A')), Some(0));
        assert_eq!(hotkey_code(HotkeyKey::Letter('L')), Some(37));
        assert_eq!(hotkey_code(HotkeyKey::Letter('Z')), Some(6));
        assert_eq!(hotkey_code(HotkeyKey::Digit(1)), Some(18));
        assert_eq!(hotkey_code(HotkeyKey::Function(1)), Some(122));
        assert_eq!(hotkey_code(HotkeyKey::Function(12)), Some(111));
        assert_eq!(hotkey_code(HotkeyKey::Named(KeyName::Space)), Some(49));
        assert_eq!(hotkey_code(HotkeyKey::Named(KeyName::Enter)), Some(36));
    }
}
