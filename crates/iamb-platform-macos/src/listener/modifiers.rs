//! The modifier keys held during a key press, and which of them can select a character.
//!
//! # Where the state comes from
//!
//! Every key event carries the modifier flags that applied when the key was pressed, and the
//! application that receives the event decides with those same flags. The listener therefore
//! reads the flags of each event and keeps no modifier state of its own between events: there
//! is nothing that could drift after a missed key-up, a disabled tap or a synthetic event.
//!
//! # Which modifiers select characters
//!
//! On a Mac keyboard layout, Shift, Caps Lock and Option select the character a key types:
//! Option+L is `@` on the German layout and Option+E starts an acute accent on the US one. A
//! key pressed with them is text whenever the layout says so (architecture §4.1).
//!
//! Command and Control do not select characters; they turn the key into a command:
//!
//! - With Command held the layout still names a character (the one the key types without
//!   Command), but applications run a key equivalent with it and insert nothing.
//! - With Control held the layout names a control code for a letter and repeats the plain
//!   character for most other keys. Applications hand the press to their key bindings first,
//!   and the system takes many of these chords for itself (Control+Space switches the input
//!   source, Control+digit switches desktops), so the plain character is not typed.
//!
//! Reporting such a press as a character would put a character into the engine's buffer that
//! is not in the document; the next expansion would then delete one character too many. So a
//! press with Command or Control is a shortcut and never text. The cost of the opposite
//! mistake (the application did insert a character) is only a cleared buffer.

/// `kCGEventFlagMaskAlphaShift`: Caps Lock is on.
pub(crate) const FLAG_CAPS_LOCK: u64 = 1 << 16;
/// `kCGEventFlagMaskShift`.
pub(crate) const FLAG_SHIFT: u64 = 1 << 17;
/// `kCGEventFlagMaskControl`.
pub(crate) const FLAG_CONTROL: u64 = 1 << 18;
/// `kCGEventFlagMaskAlternate`: Option.
pub(crate) const FLAG_OPTION: u64 = 1 << 19;
/// `kCGEventFlagMaskCommand`.
pub(crate) const FLAG_COMMAND: u64 = 1 << 20;

/// The `modifierKeyState` bits of `UCKeyTranslate`: the high byte of a classic event's
/// modifiers (`shiftKey >> 8` and so on).
pub(crate) mod layout_state {
    pub(crate) const SHIFT: u32 = 1 << 1;
    pub(crate) const CAPS_LOCK: u32 = 1 << 2;
    pub(crate) const OPTION: u32 = 1 << 3;
}

/// The modifiers of one key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Modifiers {
    flags: u64,
}

impl Modifiers {
    /// No modifier held.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self { flags: 0 };

    /// The modifiers in the flags of a key event. Other flag bits (numeric pad, Fn, the
    /// device-dependent bits) are ignored.
    pub(crate) const fn from_event_flags(flags: u64) -> Self {
        Self {
            flags: flags
                & (FLAG_CAPS_LOCK | FLAG_SHIFT | FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND),
        }
    }

    const fn has(self, flag: u64) -> bool {
        self.flags & flag != 0
    }

    /// Shift is held.
    #[cfg(test)]
    pub(crate) const fn shift(self) -> bool {
        self.has(FLAG_SHIFT)
    }

    /// Option is held.
    pub(crate) const fn option(self) -> bool {
        self.has(FLAG_OPTION)
    }

    /// Command or Control is held: the press is a command, never text (see the module docs).
    pub(crate) const fn is_command_chord(self) -> bool {
        self.has(FLAG_COMMAND | FLAG_CONTROL)
    }

    /// The modifier state to translate the key with: only the modifiers that select
    /// characters.
    pub(crate) const fn layout_state(self) -> u32 {
        let mut state = 0;
        if self.has(FLAG_SHIFT) {
            state |= layout_state::SHIFT;
        }
        if self.has(FLAG_CAPS_LOCK) {
            state |= layout_state::CAPS_LOCK;
        }
        if self.has(FLAG_OPTION) {
            state |= layout_state::OPTION;
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_modifier_bits() {
        assert_eq!(Modifiers::from_event_flags(0), Modifiers::NONE);
        // What a key event carries when nothing is held: the non-coalesced bit and
        // device-dependent low bits.
        assert_eq!(Modifiers::from_event_flags(0x100), Modifiers::NONE);
        // Numeric pad and Fn do not select characters and are not chords.
        let arrows = Modifiers::from_event_flags((1 << 21) | (1 << 23));
        assert_eq!(arrows, Modifiers::NONE);
        assert!(!arrows.is_command_chord());

        let shift = Modifiers::from_event_flags(FLAG_SHIFT | 0x102);
        assert!(shift.shift() && !shift.option() && !shift.is_command_chord());
        let option = Modifiers::from_event_flags(FLAG_OPTION);
        assert!(option.option() && !option.shift() && !option.is_command_chord());
    }

    #[test]
    fn command_and_control_make_a_chord_and_option_does_not() {
        assert!(Modifiers::from_event_flags(FLAG_COMMAND).is_command_chord());
        assert!(Modifiers::from_event_flags(FLAG_CONTROL).is_command_chord());
        assert!(Modifiers::from_event_flags(FLAG_CONTROL | FLAG_OPTION).is_command_chord());
        assert!(Modifiers::from_event_flags(FLAG_COMMAND | FLAG_SHIFT).is_command_chord());
        for flags in [
            0,
            FLAG_SHIFT,
            FLAG_OPTION,
            FLAG_CAPS_LOCK,
            FLAG_SHIFT | FLAG_OPTION,
        ] {
            assert!(!Modifiers::from_event_flags(flags).is_command_chord());
        }
    }

    #[test]
    fn the_layout_sees_shift_caps_lock_and_option_only() {
        assert_eq!(Modifiers::NONE.layout_state(), 0);
        assert_eq!(Modifiers::from_event_flags(FLAG_SHIFT).layout_state(), 2);
        assert_eq!(
            Modifiers::from_event_flags(FLAG_CAPS_LOCK).layout_state(),
            4
        );
        assert_eq!(Modifiers::from_event_flags(FLAG_OPTION).layout_state(), 8);
        assert_eq!(
            Modifiers::from_event_flags(FLAG_SHIFT | FLAG_OPTION | FLAG_CAPS_LOCK).layout_state(),
            14
        );
        // Command (1) and Control (16) never reach the layout.
        assert_eq!(
            Modifiers::from_event_flags(FLAG_COMMAND | FLAG_CONTROL | FLAG_SHIFT).layout_state(),
            2
        );
    }
}
