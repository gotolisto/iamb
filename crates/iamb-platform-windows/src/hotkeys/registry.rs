//! The chords Iamb holds, and how a chord is said to the system's hotkey call.
//!
//! This part makes no system call and is tested on every system.

use iamb_platform::core_types::KeyName;
use iamb_platform::{Chord, HotkeyId, HotkeyKey, Modifiers, PlatformError};

use crate::listener::HotkeyBinding;

// `RegisterHotKey` modifier flags (`winuser.h`).
pub(crate) const MOD_ALT: u32 = 0x0001;
pub(crate) const MOD_CONTROL: u32 = 0x0002;
pub(crate) const MOD_SHIFT: u32 = 0x0004;
pub(crate) const MOD_WIN: u32 = 0x0008;
/// Holding the chord does not fire the hotkey again and again.
pub(crate) const MOD_NOREPEAT: u32 = 0x4000;

/// The system accepts hotkey ids from 0 to this for a hotkey that belongs to a thread.
const LAST_OS_ID: i32 = 0xBFFF;

/// The virtual key of a chord's key. Letters and digits have the code of their character;
/// both are independent of the layout's shifted symbols.
pub(crate) fn virtual_key(key: HotkeyKey) -> u32 {
    match key {
        HotkeyKey::Letter(letter) => u32::from(letter.to_ascii_uppercase()),
        HotkeyKey::Digit(digit) => 0x30 + u32::from(digit),
        HotkeyKey::Function(number) => 0x6F + u32::from(number),
        HotkeyKey::Named(key) => match key {
            KeyName::Backspace => 0x08,
            KeyName::Tab => 0x09,
            KeyName::Enter => 0x0D,
            KeyName::Escape => 0x1B,
            KeyName::Space => 0x20,
            KeyName::PageUp => 0x21,
            KeyName::PageDown => 0x22,
            KeyName::End => 0x23,
            KeyName::Home => 0x24,
            KeyName::Left => 0x25,
            KeyName::Up => 0x26,
            KeyName::Right => 0x27,
            KeyName::Down => 0x28,
            KeyName::Delete => 0x2E,
        },
    }
}

/// The modifier flags of a chord for `RegisterHotKey`, with [`MOD_NOREPEAT`].
pub(crate) fn modifier_flags(modifiers: Modifiers) -> u32 {
    let mut flags = MOD_NOREPEAT;
    for (modifier, flag) in [
        (Modifiers::CTRL, MOD_CONTROL),
        (Modifiers::ALT, MOD_ALT),
        (Modifiers::SHIFT, MOD_SHIFT),
        (Modifiers::META, MOD_WIN),
    ] {
        if modifiers.contains(modifier) {
            flags |= flag;
        }
    }
    flags
}

/// One hotkey as the listener thread registers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Registration {
    /// The id the system knows it by.
    pub(crate) os_id: i32,
    /// The id the service knows it by.
    pub(crate) id: HotkeyId,
    /// The chord.
    pub(crate) chord: Chord,
}

impl Registration {
    /// The chord's modifier flags for `RegisterHotKey`.
    pub(crate) fn modifier_flags(&self) -> u32 {
        modifier_flags(self.chord.modifiers())
    }

    /// The chord's virtual key.
    pub(crate) fn virtual_key(&self) -> u32 {
        virtual_key(self.chord.key())
    }

    /// The chord as the hook recognises it in the key stream.
    pub(crate) fn binding(&self) -> HotkeyBinding {
        HotkeyBinding {
            modifiers: self.chord.modifiers(),
            vk: self.virtual_key(),
        }
    }
}

/// What registering a chord under an id takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The id already holds this chord.
    Unchanged,
    /// Register `new` with the system; when that worked, unregister `replaces` (the id's
    /// previous chord, if the system holds it) and [`commit`](Registry::commit).
    Register {
        /// The registration to make.
        new: Registration,
        /// The system id of the registration it replaces.
        replaces: Option<i32>,
    },
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    registration: Registration,
    /// True while the system holds the registration for the running listener thread.
    active: bool,
}

/// The chords registered through [`Hotkeys`](iamb_platform::Hotkeys), by id. They outlive the
/// listener thread: a restart registers them again.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    entries: Vec<Entry>,
    last_os_id: i32,
}

impl Registry {
    fn next_os_id(&mut self) -> i32 {
        loop {
            self.last_os_id = if self.last_os_id >= LAST_OS_ID {
                1
            } else {
                self.last_os_id + 1
            };
            let candidate = self.last_os_id;
            if !self
                .entries
                .iter()
                .any(|e| e.registration.os_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// What it takes to register `chord` under `id`. `running` says whether the listener
    /// thread runs (an id whose chord the system refused when the thread started is tried
    /// again).
    ///
    /// # Errors
    ///
    /// [`PlatformError::HotkeyUnavailable`] when another id holds the chord.
    pub(crate) fn plan(
        &mut self,
        id: HotkeyId,
        chord: Chord,
        running: bool,
    ) -> Result<Plan, PlatformError> {
        if self
            .entries
            .iter()
            .any(|e| e.registration.id != id && e.registration.chord == chord)
        {
            return Err(PlatformError::HotkeyUnavailable);
        }
        let existing = self
            .entries
            .iter()
            .find(|e| e.registration.id == id)
            .copied();
        if let Some(entry) = existing
            && entry.registration.chord == chord
            && (entry.active || !running)
        {
            return Ok(Plan::Unchanged);
        }
        let new = Registration {
            os_id: self.next_os_id(),
            id,
            chord,
        };
        Ok(Plan::Register {
            new,
            replaces: existing.filter(|e| e.active).map(|e| e.registration.os_id),
        })
    }

    /// Records that `new` is the registration of its id now. `active` says whether the system
    /// holds it.
    pub(crate) fn commit(&mut self, new: Registration, active: bool) {
        self.entries.retain(|e| e.registration.id != new.id);
        self.entries.push(Entry {
            registration: new,
            active,
        });
    }

    /// Forgets `id`. Returns its system id when the system holds the registration.
    pub(crate) fn remove(&mut self, id: HotkeyId) -> Option<i32> {
        let position = self.entries.iter().position(|e| e.registration.id == id)?;
        let entry = self.entries.remove(position);
        entry.active.then_some(entry.registration.os_id)
    }

    /// Every registration, for a listener thread that starts.
    pub(crate) fn registrations(&self) -> Vec<Registration> {
        self.entries.iter().map(|e| e.registration).collect()
    }

    /// The listener thread started and the system refused the registrations `refused` (by
    /// system id): every other one is held now.
    pub(crate) fn started(&mut self, refused: &[i32]) {
        for entry in &mut self.entries {
            entry.active = !refused.contains(&entry.registration.os_id);
        }
    }

    /// The listener thread ended: the system holds nothing any more.
    pub(crate) fn stopped(&mut self) {
        for entry in &mut self.entries {
            entry.active = false;
        }
    }

    /// The ids whose chord the system holds.
    #[cfg(test)]
    pub(crate) fn active(&self) -> Vec<HotkeyId> {
        self.entries
            .iter()
            .filter(|e| e.active)
            .map(|e| e.registration.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("a valid chord")
    }

    fn register(registry: &mut Registry, id: u32, text: &str, running: bool) -> Registration {
        match registry.plan(HotkeyId(id), chord(text), running) {
            Ok(Plan::Register { new, .. }) => {
                registry.commit(new, running);
                new
            }
            other => panic!("expected a registration, got {other:?}"),
        }
    }

    #[test]
    fn chords_in_the_systems_terms() {
        let palette = Registration {
            os_id: 1,
            id: HotkeyId(1),
            chord: chord("Ctrl+Shift+Space"),
        };
        assert_eq!(
            palette.modifier_flags(),
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT
        );
        assert_eq!(palette.virtual_key(), 0x20);
        assert_eq!(
            palette.binding(),
            HotkeyBinding {
                modifiers: Modifiers::CTRL.with(Modifiers::SHIFT),
                vk: 0x20
            }
        );
        assert_eq!(
            modifier_flags(chord("Alt+Win+K").modifiers()),
            MOD_ALT | MOD_WIN | MOD_NOREPEAT
        );
        assert_eq!(modifier_flags(Modifiers::NONE), MOD_NOREPEAT);
    }

    #[test]
    fn every_key_of_a_chord_has_its_virtual_key() {
        assert_eq!(virtual_key(HotkeyKey::Letter('A')), 0x41);
        assert_eq!(virtual_key(HotkeyKey::Letter('z')), 0x5A);
        assert_eq!(virtual_key(HotkeyKey::Digit(0)), 0x30);
        assert_eq!(virtual_key(HotkeyKey::Digit(9)), 0x39);
        assert_eq!(virtual_key(HotkeyKey::Function(1)), 0x70);
        assert_eq!(virtual_key(HotkeyKey::Function(12)), 0x7B);
        assert_eq!(virtual_key(HotkeyKey::Function(24)), 0x87);
        // The named keys agree with the listener's classification of virtual keys.
        for key in KeyName::ALL {
            let vk = virtual_key(HotkeyKey::Named(key));
            assert_eq!(
                crate::listener::keys::classify(vk),
                crate::listener::keys::KeyClass::Named(key),
                "{key:?}"
            );
        }
    }

    #[test]
    fn registering_before_the_listener_runs_is_kept_for_its_start() {
        let mut registry = Registry::default();
        let palette = register(&mut registry, 1, "Ctrl+Shift+Space", false);
        let other = register(&mut registry, 2, "Alt+F9", false);
        assert_ne!(palette.os_id, other.os_id);
        assert!(registry.active().is_empty());
        assert_eq!(registry.registrations(), [palette, other]);
        // Asking for the same again changes nothing.
        assert_eq!(
            registry.plan(HotkeyId(1), chord("ctrl+shift+space"), false),
            Ok(Plan::Unchanged)
        );

        // The thread starts; the system refuses the second chord.
        registry.started(&[other.os_id]);
        assert_eq!(registry.active(), [HotkeyId(1)]);
        // The refused one is tried again when it is registered again.
        assert!(matches!(
            registry.plan(HotkeyId(2), chord("Alt+F9"), true),
            Ok(Plan::Register { replaces: None, .. })
        ));
        assert_eq!(
            registry.plan(HotkeyId(1), chord("Ctrl+Shift+Space"), true),
            Ok(Plan::Unchanged)
        );

        registry.stopped();
        assert!(registry.active().is_empty());
        assert_eq!(registry.registrations().len(), 2, "kept for the next start");
    }

    #[test]
    fn a_chord_another_id_holds_is_unavailable() {
        let mut registry = Registry::default();
        register(&mut registry, 1, "Ctrl+Shift+Space", true);
        assert_eq!(
            registry.plan(HotkeyId(2), chord("Ctrl+Shift+Space"), true),
            Err(PlatformError::HotkeyUnavailable)
        );
        assert_eq!(registry.registrations().len(), 1);
    }

    #[test]
    fn registering_an_id_again_replaces_its_chord() {
        let mut registry = Registry::default();
        let old = register(&mut registry, 1, "Ctrl+Shift+Space", true);
        let Ok(Plan::Register { new, replaces }) =
            registry.plan(HotkeyId(1), chord("Ctrl+Alt+Meta+K"), true)
        else {
            panic!("expected a registration");
        };
        assert_eq!(replaces, Some(old.os_id));
        assert_ne!(
            new.os_id, old.os_id,
            "the old one stays until the new one works"
        );
        // The system refuses the new chord: nothing was committed, the old one stays.
        assert_eq!(registry.registrations(), [old]);
        // It accepts it.
        registry.commit(new, true);
        assert_eq!(registry.registrations(), [new]);
        assert_eq!(registry.active(), [HotkeyId(1)]);
        // The old chord is free for another id again.
        assert!(
            registry
                .plan(HotkeyId(2), chord("Ctrl+Shift+Space"), true)
                .is_ok()
        );
    }

    #[test]
    fn removing_an_id() {
        let mut registry = Registry::default();
        let held = register(&mut registry, 1, "Ctrl+Shift+Space", true);
        register(&mut registry, 2, "Alt+F9", false);
        assert_eq!(registry.remove(HotkeyId(1)), Some(held.os_id));
        assert_eq!(
            registry.remove(HotkeyId(2)),
            None,
            "the system did not hold it"
        );
        assert_eq!(
            registry.remove(HotkeyId(3)),
            None,
            "unknown ids are not an error"
        );
        assert!(registry.registrations().is_empty());
    }

    #[test]
    fn system_ids_stay_in_range_and_are_not_reused_while_held() {
        let mut registry = Registry {
            last_os_id: LAST_OS_ID - 1,
            ..Registry::default()
        };
        let last = register(&mut registry, 1, "Alt+F1", true);
        assert_eq!(last.os_id, LAST_OS_ID);
        let first = register(&mut registry, 2, "Alt+F2", true);
        assert_eq!(first.os_id, 1, "wraps around");
        registry.last_os_id = LAST_OS_ID - 1;
        let next = register(&mut registry, 3, "Alt+F3", true);
        assert_eq!(next.os_id, 2, "skips the ids that are held");
    }
}
