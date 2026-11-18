//! Which keys are down, tracked from the hook stream (architecture §4.1).
//!
//! `GetKeyState` answers for the calling thread's own input queue, and the listener thread
//! receives no keyboard messages, so its answer is stale. The listener therefore keeps its own
//! picture of the keyboard: every key event the hook sees sets or clears one bit, and Caps Lock
//! flips a toggle when it goes down.
//!
//! The picture holds every key, not only Shift, Ctrl and Alt, because a layout may use any key
//! as a modifier (the right Ctrl key of the Canadian Multilingual layout selects a third
//! level, for example), and the translation looks at whichever keys the layout names.
//!
//! # Keeping the picture true
//!
//! A hook does not see everything, and not everything it sees happens:
//!
//! - **A release it never saw.** While a window of a higher privilege level or the secure
//!   desktop (the lock screen, a consent prompt) has the keyboard, the hook is not called.
//!   Ctrl+Shift+Esc opens the task manager and the releases of Ctrl and Shift go by unseen;
//!   without a correction every later key would look like a Ctrl+Shift chord.
//!   [`resync`](KeyTracker::resync) compares the picture with the physical keyboard
//!   (`GetAsyncKeyState`) when the previous key event is at least [`RESYNC_GAP_MS`] old, which
//!   every such excursion is.
//! - **A press that never happened.** A key-remapping tool whose hook comes after the
//!   listener's swallows a key the listener has already seen (Caps Lock turned into Ctrl is
//!   the common case). [`verify`](KeyTracker::verify) asks, at the next event, whether a
//!   modifier or Caps Lock that went down really is down, and takes the press back if not.
//!
//! Both ask about a key only after the event that pressed it has been through every hook (the
//! physical state of the key of the event in progress is not yet updated, and the listener
//! never asks about it). `GetAsyncKeyState` also clears a "pressed since the last call" bit that
//! a few old programs poll; the two rules above keep the calls to one per modifier press and
//! to keys whose press is at least a second old.
//!
//! The Caps Lock toggle is read once, when the hook is installed. No call reports it for
//! another thread afterwards; if it is changed where the hook cannot see (on the secure
//! desktop, or by a setting that makes Shift turn Caps Lock off), the picture is wrong until
//! the hook is installed again.

use std::fmt;

use super::keys::{
    VK_CAPITAL, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};

/// The previous key event must be this old before the picture is compared with the physical
/// keyboard. Longer than any pause inside a chord; shorter than any trip to a window the hook
/// cannot see.
pub(crate) const RESYNC_GAP_MS: u64 = 1_000;

/// The modifier keys every layout uses: both Shifts, Ctrls, Alts and Windows keys.
const MODIFIERS: [u32; 8] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

/// A set of virtual keys.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct KeySet([u64; 4]);

impl KeySet {
    const fn slot(vk: u32) -> (usize, u64) {
        ((vk as usize >> 6) & 3, 1 << (vk & 63))
    }

    const fn contains(&self, vk: u32) -> bool {
        let (word, bit) = Self::slot(vk);
        vk <= 0xFF && self.0[word] & bit != 0
    }

    const fn set(&mut self, vk: u32, on: bool) {
        if vk > 0xFF {
            return;
        }
        let (word, bit) = Self::slot(vk);
        if on {
            self.0[word] |= bit;
        } else {
            self.0[word] &= !bit;
        }
    }

    fn iter(self) -> impl Iterator<Item = u32> {
        (0..=0xFF).filter(move |vk| self.contains(*vk))
    }
}

/// The keys that are down and the Caps Lock toggle: what a translation depends on.
///
/// It says which keys are held at one moment, so its `Debug` shows the modifiers only.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct KeyState {
    down: KeySet,
    caps: bool,
}

impl KeyState {
    /// A keyboard with nothing held but `keys`, and Caps Lock off.
    pub(crate) const fn holding(keys: &[u32]) -> Self {
        let mut down = KeySet([0; 4]);
        let mut index = 0;
        while index < keys.len() {
            down.set(keys[index], true);
            index += 1;
        }
        Self { down, caps: false }
    }

    /// True while `vk` is down.
    pub(crate) const fn is_down(&self, vk: u32) -> bool {
        self.down.contains(vk)
    }

    /// Either Shift key.
    pub(crate) const fn shift(&self) -> bool {
        self.is_down(VK_LSHIFT) || self.is_down(VK_RSHIFT)
    }

    /// Either Ctrl key.
    pub(crate) const fn ctrl(&self) -> bool {
        self.is_down(VK_LCONTROL) || self.is_down(VK_RCONTROL)
    }

    /// Either Alt key.
    pub(crate) const fn alt(&self) -> bool {
        self.is_down(VK_LMENU) || self.is_down(VK_RMENU)
    }

    /// Either Windows key.
    pub(crate) const fn win(&self) -> bool {
        self.is_down(VK_LWIN) || self.is_down(VK_RWIN)
    }

    /// The Caps Lock toggle.
    pub(crate) const fn caps(&self) -> bool {
        self.caps
    }

    /// The keys that are down.
    pub(crate) fn held(&self) -> impl Iterator<Item = u32> + use<> {
        self.down.iter()
    }

    /// True when the held modifiers let a key type: none, Shift, or Ctrl and Alt together
    /// (which is what `AltGr` is). Ctrl alone, Alt alone and the Windows key make a command.
    pub(crate) const fn types_text(&self) -> bool {
        !self.win() && self.ctrl() == self.alt()
    }

    /// True while Ctrl, Alt or a Windows key is held.
    pub(crate) const fn chord(&self) -> bool {
        self.ctrl() || self.alt() || self.win()
    }
}

impl fmt::Debug for KeyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyState")
            .field("shift", &self.shift())
            .field("ctrl", &self.ctrl())
            .field("alt", &self.alt())
            .field("win", &self.win())
            .field("caps", &self.caps)
            .finish_non_exhaustive()
    }
}

/// The listener's picture of the keyboard.
#[derive(Default)]
pub(crate) struct KeyTracker {
    state: KeyState,
    /// Modifiers that went down and have not been checked against the physical keyboard.
    unverified: KeySet,
    /// The Caps Lock toggle flipped and the press has not been checked.
    caps_unverified: bool,
}

impl fmt::Debug for KeyTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyTracker")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Shift, Ctrl and Alt without a side count as the left key.
const fn canonical(vk: u32) -> u32 {
    match vk {
        VK_SHIFT => VK_LSHIFT,
        VK_CONTROL => VK_LCONTROL,
        VK_MENU => VK_LMENU,
        other => other,
    }
}

fn is_modifier(vk: u32) -> bool {
    MODIFIERS.contains(&vk)
}

impl KeyTracker {
    /// The picture as it stands.
    pub(crate) const fn state(&self) -> &KeyState {
        &self.state
    }

    /// Starts from the physical keyboard: the modifiers `physically_down` reports and the
    /// Caps Lock toggle `caps`. Every other key counts as up.
    pub(crate) fn seed(&mut self, mut physically_down: impl FnMut(u32) -> bool, caps: bool) {
        *self = Self::default();
        for vk in MODIFIERS {
            self.state.down.set(vk, physically_down(vk));
        }
        self.state.caps = caps;
    }

    /// Records one key event of the hook stream. A repeated key-down changes nothing.
    pub(crate) fn event(&mut self, vk: u32, down: bool) {
        let vk = canonical(vk);
        let was_down = self.state.is_down(vk);
        self.state.down.set(vk, down);
        let pressed = down && !was_down;
        if vk == VK_CAPITAL && pressed {
            self.state.caps = !self.state.caps;
            self.caps_unverified = true;
        }
        if is_modifier(vk) {
            self.unverified
                .set(vk, pressed || (down && self.unverified.contains(vk)));
        }
    }

    /// Takes back the modifier and Caps Lock presses that never reached the system: a hook
    /// after the listener's swallowed them. Call it for an event that is not nested in
    /// another (see the module documentation), before [`event`](Self::event).
    pub(crate) fn verify(&mut self, mut physically_down: impl FnMut(u32) -> bool) {
        for vk in MODIFIERS {
            if self.unverified.contains(vk) {
                self.unverified.set(vk, false);
                if !physically_down(vk) {
                    self.state.down.set(vk, false);
                }
            }
        }
        if self.caps_unverified {
            self.caps_unverified = false;
            if !physically_down(VK_CAPITAL) {
                self.state.caps = !self.state.caps;
                self.state.down.set(VK_CAPITAL, false);
            }
        }
    }

    /// Makes the picture agree with the physical keyboard: keys that are no longer down are
    /// released, and modifiers that are down are pressed. `current` is the key of the event
    /// in progress, whose physical state is not yet updated; it is left alone. Call it for an
    /// event that is not nested in another, before [`event`](Self::event).
    pub(crate) fn resync(&mut self, current: u32, mut physically_down: impl FnMut(u32) -> bool) {
        let current = canonical(current);
        // First the presses that never happened, the Caps Lock toggle among them.
        self.verify(&mut physically_down);
        for vk in self.state.held() {
            if vk != current && !physically_down(vk) {
                self.state.down.set(vk, false);
            }
        }
        for vk in MODIFIERS {
            if vk != current && !self.state.is_down(vk) && physically_down(vk) {
                self.state.down.set(vk, true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::keys::{VK_ESCAPE, VK_SPACE};
    use super::*;

    const VK_A: u32 = 0x41;
    const VK_OEM_8: u32 = 0xDF;

    /// A physical keyboard for `verify` and `resync`, which counts how often it is asked.
    struct Physical {
        down: Vec<u32>,
        asked: Vec<u32>,
    }

    impl Physical {
        fn holding(down: &[u32]) -> Self {
            Self {
                down: down.to_vec(),
                asked: Vec::new(),
            }
        }

        fn ask(&mut self) -> impl FnMut(u32) -> bool + '_ {
            |vk| {
                self.asked.push(vk);
                self.down.contains(&vk)
            }
        }
    }

    #[test]
    fn modifiers_follow_the_stream_by_side() {
        let mut keys = KeyTracker::default();
        assert!(!keys.state().shift() && !keys.state().chord());
        keys.event(VK_LSHIFT, true);
        keys.event(VK_RSHIFT, true);
        assert!(keys.state().shift());
        keys.event(VK_LSHIFT, false);
        assert!(keys.state().shift(), "the right Shift is still down");
        keys.event(VK_RSHIFT, false);
        assert!(!keys.state().shift());

        keys.event(VK_RCONTROL, true);
        assert!(keys.state().ctrl() && keys.state().chord() && !keys.state().types_text());
        keys.event(VK_RCONTROL, false);
        keys.event(VK_LMENU, true);
        assert!(keys.state().alt() && !keys.state().types_text());
        keys.event(VK_LMENU, false);
        keys.event(VK_RWIN, true);
        assert!(keys.state().win() && !keys.state().types_text());
        keys.event(VK_RWIN, false);
        assert!(keys.state().types_text() && !keys.state().chord());
    }

    #[test]
    fn altgr_is_ctrl_and_alt_together_and_types() {
        let mut keys = KeyTracker::default();
        // What Windows sends for the AltGr key: a left Ctrl, then the right Alt.
        keys.event(VK_LCONTROL, true);
        assert!(!keys.state().types_text());
        keys.event(VK_RMENU, true);
        assert!(keys.state().ctrl() && keys.state().alt());
        assert!(keys.state().types_text(), "AltGr is a character modifier");
        assert!(keys.state().chord());
        keys.event(VK_LSHIFT, true);
        assert!(keys.state().types_text(), "and so is Shift+AltGr");
        keys.event(VK_LWIN, true);
        assert!(!keys.state().types_text(), "the Windows key never types");
    }

    #[test]
    fn keys_without_a_side_count_as_the_left_one() {
        let mut keys = KeyTracker::default();
        keys.event(VK_SHIFT, true);
        keys.event(VK_CONTROL, true);
        keys.event(VK_MENU, true);
        assert!(keys.state().is_down(VK_LSHIFT));
        assert!(keys.state().is_down(VK_LCONTROL));
        assert!(keys.state().is_down(VK_LMENU));
        keys.event(VK_LSHIFT, false);
        keys.event(VK_CONTROL, false);
        keys.event(VK_MENU, false);
        assert_eq!(*keys.state(), KeyState::default());
    }

    #[test]
    fn caps_lock_flips_on_the_press_and_not_on_repeats() {
        let mut keys = KeyTracker::default();
        keys.event(VK_CAPITAL, true);
        assert!(keys.state().caps());
        keys.event(VK_CAPITAL, true);
        keys.event(VK_CAPITAL, true);
        assert!(keys.state().caps(), "auto-repeat does not toggle");
        keys.event(VK_CAPITAL, false);
        assert!(keys.state().caps());
        keys.event(VK_CAPITAL, true);
        keys.event(VK_CAPITAL, false);
        assert!(!keys.state().caps());
    }

    #[test]
    fn every_key_is_tracked_because_a_layout_may_use_any_as_a_modifier() {
        let mut keys = KeyTracker::default();
        keys.event(VK_OEM_8, true);
        keys.event(VK_A, true);
        assert!(keys.state().is_down(VK_OEM_8) && keys.state().is_down(VK_A));
        assert_eq!(keys.state().held().collect::<Vec<_>>(), [VK_A, VK_OEM_8]);
        keys.event(VK_A, false);
        keys.event(VK_OEM_8, false);
        assert_eq!(keys.state().held().count(), 0);
        // Out-of-range codes are ignored.
        keys.event(0x1FF, true);
        keys.event(0x141, true);
        assert_eq!(keys.state().held().count(), 0);
        assert!(!keys.state().is_down(0x141));
    }

    #[test]
    fn seed_reads_the_modifiers_and_the_toggle() {
        let mut keys = KeyTracker::default();
        keys.event(VK_A, true);
        let mut physical = Physical::holding(&[VK_RSHIFT, VK_A]);
        keys.seed(physical.ask(), true);
        assert!(keys.state().shift() && keys.state().caps());
        assert!(!keys.state().is_down(VK_A), "only modifiers are read");
        assert_eq!(physical.asked, MODIFIERS);
    }

    #[test]
    fn a_swallowed_modifier_press_is_taken_back_at_the_next_event() {
        let mut keys = KeyTracker::default();
        keys.event(VK_LMENU, true);
        assert!(keys.state().alt());
        // A remapping tool swallowed the Alt: the system never saw it go down.
        let mut physical = Physical::holding(&[]);
        keys.verify(physical.ask());
        assert!(!keys.state().alt());
        assert_eq!(physical.asked, [VK_LMENU], "one question per press");

        // Nothing left to check.
        let mut physical = Physical::holding(&[]);
        keys.verify(physical.ask());
        assert!(physical.asked.is_empty());
    }

    #[test]
    fn a_real_modifier_press_is_checked_once_and_kept() {
        let mut keys = KeyTracker::default();
        keys.event(VK_LSHIFT, true);
        let mut physical = Physical::holding(&[VK_LSHIFT]);
        keys.verify(physical.ask());
        keys.verify(physical.ask());
        assert!(keys.state().shift());
        assert_eq!(physical.asked, [VK_LSHIFT]);
        // Its auto-repeat is not a new press.
        keys.event(VK_LSHIFT, true);
        keys.verify(physical.ask());
        assert_eq!(physical.asked, [VK_LSHIFT]);
        // A release needs no check.
        keys.event(VK_LSHIFT, false);
        keys.event(VK_LSHIFT, true);
        keys.event(VK_LSHIFT, false);
        keys.verify(physical.ask());
        assert_eq!(physical.asked, [VK_LSHIFT]);
        assert!(!keys.state().shift());
    }

    #[test]
    fn a_swallowed_caps_lock_does_not_flip_the_toggle() {
        // Caps Lock remapped to Ctrl by a tool whose hook comes after the listener's.
        let mut keys = KeyTracker::default();
        keys.event(VK_CAPITAL, true);
        keys.event(VK_LCONTROL, true); // injected by the tool, nested: no check yet
        assert!(keys.state().caps());
        // The next event of its own: neither Caps Lock nor... but Ctrl is really down.
        let mut physical = Physical::holding(&[VK_LCONTROL]);
        keys.verify(physical.ask());
        assert!(!keys.state().caps(), "the press never happened");
        assert!(!keys.state().is_down(VK_CAPITAL));
        assert!(keys.state().ctrl());
        // The physical release of Caps Lock changes nothing more.
        keys.event(VK_CAPITAL, false);
        keys.event(VK_LCONTROL, false);
        assert_eq!(*keys.state(), KeyState::default());

        // Held and repeating: every repeat looks like a press and is taken back again.
        keys.event(VK_CAPITAL, true);
        for _ in 0..3 {
            keys.verify(Physical::holding(&[]).ask());
            assert!(!keys.state().caps());
            keys.event(VK_CAPITAL, true);
        }
        keys.verify(Physical::holding(&[]).ask());
        keys.event(VK_CAPITAL, false);
        assert_eq!(*keys.state(), KeyState::default());
    }

    #[test]
    fn a_real_caps_lock_press_is_kept() {
        let mut keys = KeyTracker::default();
        keys.event(VK_CAPITAL, true);
        // Checked at its own release: the key is still physically down then.
        keys.verify(Physical::holding(&[VK_CAPITAL]).ask());
        keys.event(VK_CAPITAL, false);
        assert!(keys.state().caps());
        let mut physical = Physical::holding(&[]);
        keys.verify(physical.ask());
        assert!(physical.asked.is_empty());
        assert!(keys.state().caps());
    }

    #[test]
    fn resync_releases_what_is_no_longer_held() {
        // Ctrl+Shift+Esc opened the task manager; the releases went by unseen.
        let mut keys = KeyTracker::default();
        keys.event(VK_LCONTROL, true);
        keys.event(VK_LSHIFT, true);
        keys.verify(Physical::holding(&[VK_LCONTROL, VK_LSHIFT]).ask());
        keys.event(VK_ESCAPE, true);
        assert!(keys.state().ctrl() && keys.state().shift());

        // Seconds later the next key arrives: the letter A.
        keys.resync(VK_A, Physical::holding(&[]).ask());
        keys.event(VK_A, true);
        assert!(!keys.state().ctrl() && !keys.state().shift());
        assert!(!keys.state().is_down(VK_ESCAPE));
        assert!(keys.state().is_down(VK_A));
        assert!(keys.state().types_text());
    }

    #[test]
    fn resync_presses_modifiers_whose_press_went_unseen() {
        let mut keys = KeyTracker::default();
        let mut physical = Physical::holding(&[VK_RSHIFT, VK_SPACE]);
        keys.resync(VK_A, physical.ask());
        assert!(keys.state().shift());
        assert!(
            !keys.state().is_down(VK_SPACE),
            "only modifiers are pressed: other keys are not asked about"
        );
        assert_eq!(physical.asked, MODIFIERS);
    }

    #[test]
    fn resync_leaves_the_key_of_the_event_in_progress_alone() {
        // The physical state of the key being pressed is not updated yet.
        let mut keys = KeyTracker::default();
        keys.event(VK_LSHIFT, true);
        keys.verify(Physical::holding(&[VK_LSHIFT]).ask());
        let mut physical = Physical::holding(&[]);
        keys.resync(VK_SHIFT, physical.ask());
        assert!(keys.state().shift(), "the repeating Shift is not released");
        assert!(!physical.asked.contains(&VK_LSHIFT));

        // And a modifier going down after a pause is not pressed twice or asked about.
        let mut keys = KeyTracker::default();
        let mut physical = Physical::holding(&[]);
        keys.resync(VK_RMENU, physical.ask());
        assert!(!physical.asked.contains(&VK_RMENU));
        keys.event(VK_RMENU, true);
        assert!(keys.state().alt());
    }

    #[test]
    fn resync_also_takes_back_a_swallowed_caps_lock() {
        let mut keys = KeyTracker::default();
        keys.event(VK_CAPITAL, true);
        keys.resync(VK_A, Physical::holding(&[]).ask());
        assert!(!keys.state().caps());
    }

    #[test]
    fn debug_shows_modifiers_and_never_other_keys() {
        let mut keys = KeyTracker::default();
        keys.event(VK_A, true);
        keys.event(VK_LSHIFT, true);
        let shown = format!("{:?}", keys.state());
        assert_eq!(
            shown,
            "KeyState { shift: true, ctrl: false, alt: false, win: false, caps: false, .. }"
        );
        assert!(format!("{keys:?}").contains("shift: true"));
        let altgr = KeyState::holding(&[VK_RSHIFT, VK_LCONTROL, VK_RMENU]);
        assert!(altgr.shift() && altgr.ctrl() && altgr.alt() && altgr.types_text());
        assert!(!altgr.caps() && !altgr.win());
        assert_eq!(altgr.held().count(), 3);
        assert_eq!(KeyState::holding(&[]), KeyState::default());
    }
}
