//! From a key press to what it typed: the keyboard-layout seam, the listener's own dead-key
//! state, and the rules that decide between text, Backspace and a boundary.
//!
//! # The rules
//!
//! 1. Command or Control held: a shortcut, never text ([`modifiers`](super::modifiers) says
//!    why). The press is `Boundary(Shortcut)` whatever the key.
//! 2. A key that never types (Return, Tab, Escape, delete, the arrows, the navigation and
//!    function keys) is reported by name, without asking the layout.
//! 3. Any other key is given to the layout with the Shift, Caps Lock and Option state of the
//!    event. If the layout answers with text, each character is a `Char`: Option combinations
//!    are characters (architecture §4.1). If it answers with nothing because the key is a dead
//!    key, nothing is reported yet: the accent arrives combined with the next key. Otherwise
//!    the press is a boundary: `Shortcut` when Option was held, `Other` when not.
//!
//! # Dead keys
//!
//! The layout is a pure function of the key, the modifiers and a 32-bit dead-key state. The
//! [`Translator`] owns that state, so translating a key here never touches the state the
//! focused application keeps for the same keyboard (architecture §4.1). The state is the only
//! thing derived from the user's typing that the listener keeps between two events; it says at
//! most which dead key is pending.
//!
//! The listener's state follows the application's as closely as the event stream allows:
//!
//! - The pending accent is dropped when the application would end its own composition: a
//!   shortcut, a named key, a mouse click, an injection, a change of layout or input method.
//! - Delete after a dead key removes the pending accent in the application and nothing else,
//!   so nothing is reported.
//! - Return or Tab after a dead key is reported as a plain boundary, not as the delimiter it
//!   normally is: what the application did with the accent is not known, so the engine must
//!   not match on that press.
//!
//! # Modified named keys
//!
//! Option+delete deletes a word and Option+Return or Option+Tab do something the application
//! chooses. None of them is the single deletion or the delimiter the plain key is, so with
//! Option they are `Boundary(Shortcut)`. Shift does not change a named key.
//!
//! # Limits
//!
//! - The press-and-hold accent menu replaces the held letter with the chosen accented one.
//!   The event stream shows the letter and then the digit that picked the accent; the
//!   listener reports both as typed.
//! - What a non-Cocoa application does with a pending dead key on a named key is its own
//!   choice; the rules above follow the system text input.

use iamb_platform::core_types::{BoundaryKind, KeyName, Sensitive};

use super::keycodes::{KeyClass, classify};
use super::modifiers::Modifiers;

/// The most UTF-16 units one key press may yield. A dead key followed by a key it does not
/// combine with yields two characters; layouts with multi-character keys stay well below
/// this.
pub(crate) const MAX_UNITS: usize = 16;

/// How the key was pressed, as the layout wants to know it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyAction {
    /// The key went down.
    Down,
    /// The key is held and repeating.
    Repeat,
}

impl KeyAction {
    /// The `keyAction` argument of `UCKeyTranslate`: `kUCKeyActionDown` or
    /// `kUCKeyActionAutoKey`.
    pub(crate) const fn code(self) -> u16 {
        match self {
            Self::Down => 0,
            Self::Repeat => 2,
        }
    }
}

/// One key press as a keyboard layout sees it. No `Debug`: it names a key the user pressed.
pub(crate) struct KeyPress {
    /// The virtual key code.
    pub(crate) code: u16,
    /// Down or repeating.
    pub(crate) action: KeyAction,
    /// The modifier state ([`Modifiers::layout_state`]).
    pub(crate) modifiers: u32,
    /// The keyboard type the event came from; 0 when it is not known.
    pub(crate) keyboard_type: u32,
}

/// A keyboard layout: a pure function from a key press and a dead-key state to text.
///
/// This is the contract of `UCKeyTranslate`: the text of the press is written to `out` and
/// its length in UTF-16 units returned; `dead_state` is read and replaced. A dead key writes
/// nothing and leaves a non-zero state; the next press writes the combined character, or the
/// accent followed by its own character. `None` means the layout could not translate the
/// press at all.
pub(crate) trait KeyLayout: Send + Sync {
    /// Translates one press.
    fn translate(
        &self,
        press: &KeyPress,
        dead_state: &mut u32,
        out: &mut [u16; MAX_UNITS],
    ) -> Option<usize>;
}

/// What a key press did besides typing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEffect {
    /// Nothing: the press typed text, or was a dead key.
    None,
    /// A key with a name; the listener reports it with
    /// [`ListenerEvent::named_key`](iamb_platform::ListenerEvent::named_key).
    Named(KeyName),
    /// A press without text.
    Boundary(BoundaryKind),
}

/// What one key press means: the characters it typed, then at most one key effect.
///
/// No `Debug` and no public field: it holds typed characters.
pub(crate) struct KeyOutput {
    text: [char; MAX_UNITS],
    len: usize,
    effect: KeyEffect,
}

impl KeyOutput {
    const fn effect(effect: KeyEffect) -> Self {
        Self {
            text: ['\0'; MAX_UNITS],
            len: 0,
            effect,
        }
    }

    const fn nothing() -> Self {
        Self::effect(KeyEffect::None)
    }

    const fn boundary(kind: BoundaryKind) -> Self {
        Self::effect(KeyEffect::Boundary(kind))
    }

    fn push(&mut self, ch: char) {
        if let Some(slot) = self.text.get_mut(self.len) {
            *slot = ch;
            self.len += 1;
        }
    }

    /// The characters the press typed, in order.
    pub(crate) fn text(&self) -> impl Iterator<Item = Sensitive<char>> + '_ {
        self.text.iter().take(self.len).copied().map(Sensitive::new)
    }

    /// What the press did after its text.
    pub(crate) const fn key_effect(&self) -> KeyEffect {
        self.effect
    }
}

/// True when a `UCKeyTranslate` dead-key state holds a waiting dead key. Only the low word
/// counts: a key that ends a dead key leaves a non-zero high word behind (0x1, then E, gives
/// 0x10000 on the system's layouts), which is passed back unchanged but waits for nothing.
pub(crate) const fn dead_key_waits(state: u32) -> bool {
    state & 0xFFFF != 0
}

/// The listener's translation state: its own dead-key state and nothing else.
#[derive(Debug, Default)]
pub(crate) struct Translator {
    dead_state: u32,
}

impl Drop for Translator {
    /// The listener stopped: the dead-key state, which says which accent waits, is
    /// overwritten (architecture §8 rule 1; the 2.8 review's finding 9).
    fn drop(&mut self) {
        self.dead_state = 0;
        std::hint::black_box(&mut self.dead_state);
    }
}

impl Translator {
    /// A translator with no dead key pending.
    pub(crate) const fn new() -> Self {
        Self { dead_state: 0 }
    }

    /// True while a dead key waits for the key it combines with.
    pub(crate) const fn dead_key_pending(&self) -> bool {
        dead_key_waits(self.dead_state)
    }

    /// Forgets a pending dead key: the application's own composition has ended.
    pub(crate) const fn reset(&mut self) {
        self.dead_state = 0;
    }

    /// Translates a key press. `layout` is `None` when no keyboard layout applies (none is
    /// known yet, or an input method owns the keys); nothing is then reported as text.
    pub(crate) fn key_down(
        &mut self,
        code: u16,
        modifiers: Modifiers,
        repeat: bool,
        keyboard_type: u32,
        layout: Option<&dyn KeyLayout>,
    ) -> KeyOutput {
        let class = classify(code);
        if class == KeyClass::Modifier {
            // Modifiers arrive as flag changes. A key press with a modifier's code types
            // nothing and ends nothing.
            return KeyOutput::nothing();
        }
        if modifiers.is_command_chord() {
            self.reset();
            return KeyOutput::boundary(BoundaryKind::Shortcut);
        }
        match class {
            KeyClass::Named(name) => {
                let pending = self.dead_key_pending();
                self.reset();
                KeyOutput::effect(named_effect(name, modifiers, pending))
            }
            KeyClass::Layout => {
                let action = if repeat {
                    KeyAction::Repeat
                } else {
                    KeyAction::Down
                };
                let press = KeyPress {
                    code,
                    action,
                    modifiers: modifiers.layout_state(),
                    keyboard_type,
                };
                self.layout_key(&press, modifiers, layout)
            }
            KeyClass::Function | KeyClass::Modifier => {
                self.reset();
                KeyOutput::boundary(without_text(modifiers))
            }
        }
    }

    fn layout_key(
        &mut self,
        press: &KeyPress,
        modifiers: Modifiers,
        layout: Option<&dyn KeyLayout>,
    ) -> KeyOutput {
        let mut units = [0_u16; MAX_UNITS];
        let translated =
            layout.and_then(|layout| layout.translate(press, &mut self.dead_state, &mut units));
        let Some(len) = translated else {
            self.reset();
            return KeyOutput::boundary(without_text(modifiers));
        };
        let units = units.get(..len).unwrap_or(&units);

        let mut output = KeyOutput::nothing();
        let mut ends_in_text = false;
        for decoded in char::decode_utf16(units.iter().copied()) {
            ends_in_text = match decoded {
                Ok(ch) if is_text(ch) => {
                    output.push(ch);
                    true
                }
                _ => false,
            };
        }
        if units.is_empty() {
            if !self.dead_key_pending() {
                output.effect = KeyEffect::Boundary(without_text(modifiers));
            }
        } else if !ends_in_text {
            // The key's own output was a control code: it is a key without text, possibly
            // after the accent of a dead key that it ended.
            self.reset();
            output.effect = KeyEffect::Boundary(without_text(modifiers));
        }
        output
    }
}

/// The boundary kind of a press without text: a chord when Option was held.
const fn without_text(modifiers: Modifiers) -> BoundaryKind {
    if modifiers.option() {
        BoundaryKind::Shortcut
    } else {
        BoundaryKind::Other
    }
}

/// What a named key did (see the module docs for the modified and dead-key cases).
const fn named_effect(name: KeyName, modifiers: Modifiers, dead_key_pending: bool) -> KeyEffect {
    match name {
        KeyName::Backspace | KeyName::Enter | KeyName::Tab if modifiers.option() => {
            KeyEffect::Boundary(BoundaryKind::Shortcut)
        }
        KeyName::Backspace if dead_key_pending => KeyEffect::None,
        KeyName::Enter | KeyName::Tab if dead_key_pending => {
            KeyEffect::Boundary(BoundaryKind::Other)
        }
        _ => KeyEffect::Named(name),
    }
}

/// True for a character that is text in a document: not a control code, and not one of the
/// private-use code points the system assigns to function keys (U+F700 to U+F7FF).
fn is_text(ch: char) -> bool {
    !ch.is_control() && !('\u{F700}'..='\u{F7FF}').contains(&ch)
}

#[cfg(test)]
mod tests {
    use super::super::keycodes::vk;
    use super::super::modifiers::{
        FLAG_CAPS_LOCK, FLAG_COMMAND, FLAG_CONTROL, FLAG_OPTION, FLAG_SHIFT,
    };
    use super::super::testing::{FakeLayout, typed};
    use super::*;

    const PLAIN: Modifiers = Modifiers::NONE;

    fn mods(flags: u64) -> Modifiers {
        Modifiers::from_event_flags(flags)
    }

    /// Presses a key on `layout` and returns the text and the effect.
    fn press(
        translator: &mut Translator,
        layout: &FakeLayout,
        code: u16,
        modifiers: Modifiers,
    ) -> (String, KeyEffect) {
        let output = translator.key_down(code, modifiers, false, 0, Some(layout));
        (typed(&output), output.key_effect())
    }

    fn text(s: &str) -> (String, KeyEffect) {
        (s.to_owned(), KeyEffect::None)
    }

    fn only(effect: KeyEffect) -> (String, KeyEffect) {
        (String::new(), effect)
    }

    fn boundary(kind: BoundaryKind) -> (String, KeyEffect) {
        only(KeyEffect::Boundary(kind))
    }

    #[test]
    fn plain_and_shifted_keys_are_text() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        assert_eq!(press(&mut t, &us, vk::ANSI_A, PLAIN), text("a"));
        assert_eq!(press(&mut t, &us, vk::ANSI_A, mods(FLAG_SHIFT)), text("A"));
        assert_eq!(
            press(&mut t, &us, vk::ANSI_A, mods(FLAG_CAPS_LOCK)),
            text("A")
        );
        assert_eq!(press(&mut t, &us, vk::ANSI_1, PLAIN), text("1"));
        assert_eq!(press(&mut t, &us, vk::ANSI_1, mods(FLAG_SHIFT)), text("!"));
        assert_eq!(press(&mut t, &us, vk::SPACE, PLAIN), text(" "));
    }

    #[test]
    fn option_combinations_that_yield_text_are_characters() {
        let german = FakeLayout::german();
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &german, vk::ANSI_L, mods(FLAG_OPTION)),
            text("@")
        );
        assert_eq!(
            press(&mut t, &german, vk::ANSI_7, mods(FLAG_OPTION | FLAG_SHIFT)),
            text("\\")
        );
        let us = FakeLayout::us();
        assert_eq!(
            press(&mut t, &us, vk::SPACE, mods(FLAG_OPTION)),
            text("\u{a0}")
        );
        // The Apple logo is a private-use character and still text.
        assert_eq!(
            press(&mut t, &us, vk::ANSI_K, mods(FLAG_OPTION | FLAG_SHIFT)),
            text("\u{f8ff}")
        );
    }

    #[test]
    fn option_without_text_is_a_shortcut() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        // The fake US layout gives Option+Z no character.
        assert_eq!(
            press(&mut t, &us, vk::ANSI_Z, mods(FLAG_OPTION)),
            boundary(BoundaryKind::Shortcut)
        );
        assert_eq!(
            press(&mut t, &us, vk::F5, mods(FLAG_OPTION)),
            boundary(BoundaryKind::Shortcut)
        );
        // A key without text and without a chord modifier is just a key.
        assert_eq!(
            press(&mut t, &us, vk::F5, PLAIN),
            boundary(BoundaryKind::Other)
        );
        assert_eq!(
            press(&mut t, &us, vk::F5, mods(FLAG_SHIFT)),
            boundary(BoundaryKind::Other)
        );
        assert_eq!(
            press(&mut t, &us, 0x66, PLAIN),
            boundary(BoundaryKind::Other)
        );
    }

    #[test]
    fn command_and_control_chords_are_never_text() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        for flags in [
            FLAG_COMMAND,
            FLAG_CONTROL,
            FLAG_COMMAND | FLAG_SHIFT,
            FLAG_CONTROL | FLAG_OPTION,
            FLAG_COMMAND | FLAG_OPTION,
        ] {
            for code in [
                vk::ANSI_C,
                vk::ANSI_1,
                vk::SPACE,
                vk::RETURN,
                vk::TAB,
                vk::DELETE,
                vk::F5,
            ] {
                assert_eq!(
                    press(&mut t, &us, code, mods(flags)),
                    boundary(BoundaryKind::Shortcut)
                );
            }
        }
        // The layout was not even asked: Command+C would have been "c".
        assert_eq!(us.calls(), 0);
    }

    #[test]
    fn named_keys_are_reported_by_name_without_the_layout() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        for (code, name) in [
            (vk::RETURN, KeyName::Enter),
            (vk::KEYPAD_ENTER, KeyName::Enter),
            (vk::TAB, KeyName::Tab),
            (vk::ESCAPE, KeyName::Escape),
            (vk::DELETE, KeyName::Backspace),
            (vk::FORWARD_DELETE, KeyName::Delete),
            (vk::LEFT_ARROW, KeyName::Left),
            (vk::PAGE_DOWN, KeyName::PageDown),
        ] {
            assert_eq!(
                press(&mut t, &us, code, PLAIN),
                only(KeyEffect::Named(name))
            );
            assert_eq!(
                press(&mut t, &us, code, mods(FLAG_SHIFT)),
                only(KeyEffect::Named(name))
            );
        }
        assert_eq!(us.calls(), 0);
        // Named keys need no layout at all.
        let output = t.key_down(vk::RETURN, PLAIN, false, 0, None);
        assert_eq!(output.key_effect(), KeyEffect::Named(KeyName::Enter));
    }

    #[test]
    fn option_turns_delete_return_and_tab_into_shortcuts() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        for code in [vk::DELETE, vk::RETURN, vk::TAB] {
            assert_eq!(
                press(&mut t, &us, code, mods(FLAG_OPTION)),
                boundary(BoundaryKind::Shortcut)
            );
        }
        // Option+arrow moves by word: still an arrow for the engine.
        assert_eq!(
            press(&mut t, &us, vk::LEFT_ARROW, mods(FLAG_OPTION)),
            only(KeyEffect::Named(KeyName::Left))
        );
    }

    #[test]
    fn a_dead_key_combines_with_the_next_key() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        assert_eq!(press(&mut t, &spanish, vk::ANSI_C, PLAIN), text("c"));
        // The dead key types nothing yet and is not a boundary.
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert!(t.dead_key_pending());
        assert_eq!(press(&mut t, &spanish, vk::ANSI_E, PLAIN), text("é"));
        assert!(!t.dead_key_pending());
        // Shift applies to the letter.
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(
            press(&mut t, &spanish, vk::ANSI_E, mods(FLAG_SHIFT)),
            text("É")
        );

        let french = FakeLayout::french();
        assert_eq!(
            press(&mut t, &french, FakeLayout::FRENCH_CIRCUMFLEX, PLAIN),
            text("")
        );
        assert_eq!(press(&mut t, &french, vk::ANSI_E, PLAIN), text("ê"));

        // Option+E on the US layout is a dead key too.
        let us = FakeLayout::us();
        assert_eq!(press(&mut t, &us, vk::ANSI_E, mods(FLAG_OPTION)), text(""));
        assert_eq!(press(&mut t, &us, vk::ANSI_E, PLAIN), text("é"));
    }

    #[test]
    fn a_dead_key_before_a_key_it_cannot_combine_with_types_both() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(press(&mut t, &spanish, vk::ANSI_X, PLAIN), text("´x"));
        assert!(!t.dead_key_pending());
        // Space after a dead key types the accent alone.
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(press(&mut t, &spanish, vk::SPACE, PLAIN), text("´"));
    }

    #[test]
    fn the_dead_key_state_is_the_translators_own() {
        let spanish = FakeLayout::spanish();
        let mut mine = Translator::new();
        let mut theirs = Translator::new();
        assert_eq!(
            press(&mut mine, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        // Another holder of state for the same layout is not affected, and does not affect.
        assert_eq!(press(&mut theirs, &spanish, vk::ANSI_E, PLAIN), text("e"));
        assert_eq!(press(&mut mine, &spanish, vk::ANSI_E, PLAIN), text("é"));
    }

    #[test]
    fn what_ends_the_applications_composition_drops_the_pending_accent() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        let dead = |t: &mut Translator| {
            assert_eq!(
                press(t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
                text("")
            );
            assert!(t.dead_key_pending());
        };

        dead(&mut t);
        assert_eq!(
            press(&mut t, &spanish, vk::ANSI_V, mods(FLAG_COMMAND)),
            boundary(BoundaryKind::Shortcut)
        );
        assert_eq!(press(&mut t, &spanish, vk::ANSI_E, PLAIN), text("e"));

        dead(&mut t);
        assert_eq!(
            press(&mut t, &spanish, vk::LEFT_ARROW, PLAIN),
            only(KeyEffect::Named(KeyName::Left))
        );
        assert_eq!(press(&mut t, &spanish, vk::ANSI_E, PLAIN), text("e"));

        dead(&mut t);
        assert_eq!(
            press(&mut t, &spanish, vk::ESCAPE, PLAIN),
            only(KeyEffect::Named(KeyName::Escape))
        );
        assert!(!t.dead_key_pending());

        dead(&mut t);
        assert_eq!(
            press(&mut t, &spanish, vk::F5, PLAIN),
            boundary(BoundaryKind::Other)
        );
        assert!(!t.dead_key_pending());

        dead(&mut t);
        t.reset();
        assert_eq!(press(&mut t, &spanish, vk::ANSI_E, PLAIN), text("e"));
    }

    #[test]
    fn delete_after_a_dead_key_removes_only_the_accent() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        // Nothing is reported: no character was typed and none was deleted.
        assert_eq!(
            press(&mut t, &spanish, vk::DELETE, PLAIN),
            only(KeyEffect::None)
        );
        assert!(!t.dead_key_pending());
        // The next delete is an ordinary one.
        assert_eq!(
            press(&mut t, &spanish, vk::DELETE, PLAIN),
            only(KeyEffect::Named(KeyName::Backspace))
        );
        // Option+delete is a shortcut even with an accent pending.
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(
            press(&mut t, &spanish, vk::DELETE, mods(FLAG_OPTION)),
            boundary(BoundaryKind::Shortcut)
        );
    }

    #[test]
    fn return_and_tab_after_a_dead_key_are_not_delimiters() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        for code in [vk::RETURN, vk::TAB] {
            assert_eq!(
                press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
                text("")
            );
            assert_eq!(
                press(&mut t, &spanish, code, PLAIN),
                boundary(BoundaryKind::Other)
            );
            assert!(!t.dead_key_pending());
        }
        assert_eq!(
            press(&mut t, &spanish, vk::RETURN, PLAIN),
            only(KeyEffect::Named(KeyName::Enter))
        );
    }

    #[test]
    fn a_modifier_key_code_is_not_a_key_press() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(
            press(&mut t, &spanish, vk::SHIFT, mods(FLAG_SHIFT)),
            only(KeyEffect::None)
        );
        // Pressing Shift between the accent and the letter is how É is typed.
        assert_eq!(
            press(&mut t, &spanish, vk::ANSI_E, mods(FLAG_SHIFT)),
            text("É")
        );
    }

    #[test]
    fn without_a_layout_nothing_is_text() {
        let mut t = Translator::new();
        for code in [vk::ANSI_A, vk::SPACE, vk::ANSI_1] {
            let output = t.key_down(code, PLAIN, false, 0, None);
            assert_eq!(typed(&output), "");
            assert_eq!(
                output.key_effect(),
                KeyEffect::Boundary(BoundaryKind::Other)
            );
        }
        let output = t.key_down(vk::ANSI_A, mods(FLAG_OPTION), false, 0, None);
        assert_eq!(
            output.key_effect(),
            KeyEffect::Boundary(BoundaryKind::Shortcut)
        );
        // Delete is still delete.
        let output = t.key_down(vk::DELETE, PLAIN, false, 0, None);
        assert_eq!(output.key_effect(), KeyEffect::Named(KeyName::Backspace));
    }

    #[test]
    fn a_layout_that_fails_gives_a_boundary_and_no_pending_state() {
        let spanish = FakeLayout::spanish();
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::FAILING_KEY, PLAIN),
            boundary(BoundaryKind::Other)
        );
        assert!(!t.dead_key_pending());
    }

    #[test]
    fn control_codes_from_the_layout_are_not_text() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        // A key whose only output is a control code is a key without text.
        assert_eq!(
            press(&mut t, &us, FakeLayout::CONTROL_CODE_KEY, PLAIN),
            boundary(BoundaryKind::Other)
        );
        // After a dead key the accent is typed, then the key is a boundary.
        let spanish = FakeLayout::spanish();
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::SPANISH_ACUTE, PLAIN),
            text("")
        );
        assert_eq!(
            press(&mut t, &spanish, FakeLayout::CONTROL_CODE_KEY, PLAIN),
            ("´".to_owned(), KeyEffect::Boundary(BoundaryKind::Other))
        );
        assert!(!t.dead_key_pending());
        assert!(!is_text('\u{8}') && !is_text('\r') && !is_text('\u{7f}') && !is_text('\u{9b}'));
        assert!(!is_text('\u{f700}') && !is_text('\u{f747}'));
        assert!(is_text(' ') && is_text('\u{a0}') && is_text('€') && is_text('\u{f8ff}'));
    }

    #[test]
    fn characters_outside_the_basic_plane_are_one_character() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        assert_eq!(press(&mut t, &us, FakeLayout::EMOJI_KEY, PLAIN), text("😀"));
        assert_eq!(
            press(&mut t, &us, FakeLayout::LONE_SURROGATE_KEY, PLAIN),
            boundary(BoundaryKind::Other)
        );
    }

    #[test]
    fn a_repeating_key_is_translated_as_a_repeat() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        let output = t.key_down(vk::ANSI_A, PLAIN, true, 44, Some(&us));
        assert_eq!(typed(&output), "a");
        assert_eq!(us.last_action(), Some(KeyAction::Repeat.code()));
        assert_eq!(us.last_keyboard_type(), Some(44));
        let _ = t.key_down(vk::ANSI_A, PLAIN, false, 0, Some(&us));
        assert_eq!(us.last_action(), Some(KeyAction::Down.code()));
        assert_eq!((KeyAction::Down.code(), KeyAction::Repeat.code()), (0, 2));
    }

    #[test]
    fn a_layout_cannot_overrun_the_output() {
        let us = FakeLayout::us();
        let mut t = Translator::new();
        // The fake claims more units than fit; the translator reads what there is.
        let output = t.key_down(FakeLayout::OVERLONG_KEY, PLAIN, false, 0, Some(&us));
        assert_eq!(typed(&output).chars().count(), MAX_UNITS);
    }
}
