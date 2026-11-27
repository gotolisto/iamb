//! Which hotkeys are wanted and which are registered with the system, free of any system
//! call.
//!
//! The service names a hotkey by its [`HotkeyId`] and may register it at any time. The system
//! holds a registration only while the listener runs, because a chord the system swallows
//! while nobody listens would simply vanish. [`Table`] keeps both sides:
//!
//! - an entry per id with its chord, wanted from `register` until `unregister`;
//! - for each entry, the system's handle while the registration is live.
//!
//! [`Table::activate`] (the listener started) registers every entry with the system,
//! [`Table::deactivate`] (it stops) releases them and keeps the entries, so they are applied
//! again on the next start.
//!
//! The system calls are behind [`Backend`]. They must run on the main thread, so the table
//! offers two doors: [`try_record`](Table::try_record) and [`try_forget`](Table::try_forget)
//! need no backend and tell the caller when one is needed; the other methods take one.

use std::collections::BTreeMap;

use iamb_platform::{Chord, HotkeyId, Modifiers, PlatformError, Result};

use crate::listener::hotkey_code;

/// The modifier bits of the system hotkey API (`cmdKey`, `shiftKey`, `optionKey`,
/// `controlKey`).
mod system_modifiers {
    pub(super) const COMMAND: u32 = 1 << 8;
    pub(super) const SHIFT: u32 = 1 << 9;
    pub(super) const OPTION: u32 = 1 << 11;
    pub(super) const CONTROL: u32 = 1 << 12;
}

/// A chord as the system hotkey API takes it: a virtual key code and a modifier mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Combo {
    /// The virtual key code.
    pub(crate) key_code: u32,
    /// The modifier mask.
    pub(crate) modifiers: u32,
}

impl Combo {
    /// The system's form of `chord`. `Meta` is Command and `Alt` is Option.
    ///
    /// # Errors
    ///
    /// [`Invalid`](PlatformError::Invalid) for a key a Mac keyboard does not have (F21 to
    /// F24).
    pub(crate) fn from_chord(chord: Chord) -> Result<Self> {
        let key_code = hotkey_code(chord.key()).ok_or(PlatformError::Invalid {
            context: "a Mac keyboard has no such key",
        })?;
        let held = chord.modifiers();
        let mut modifiers = 0;
        for (modifier, bit) in [
            (Modifiers::META, system_modifiers::COMMAND),
            (Modifiers::SHIFT, system_modifiers::SHIFT),
            (Modifiers::ALT, system_modifiers::OPTION),
            (Modifiers::CTRL, system_modifiers::CONTROL),
        ] {
            if held.contains(modifier) {
                modifiers |= bit;
            }
        }
        Ok(Self {
            key_code: u32::from(key_code),
            modifiers,
        })
    }
}

/// The system's hotkey registration.
pub(crate) trait Backend {
    /// What the system gives back for a registration, needed to release it.
    type Handle;

    /// Registers `combo`; a press is then reported as `id`.
    ///
    /// # Errors
    ///
    /// [`HotkeyUnavailable`](PlatformError::HotkeyUnavailable) when the chord is taken;
    /// [`Os`](PlatformError::Os) for any other refusal.
    fn register(&mut self, id: HotkeyId, combo: Combo) -> Result<Self::Handle>;

    /// Releases a registration.
    fn unregister(&mut self, handle: Self::Handle);
}

struct Entry<H> {
    combo: Combo,
    /// The system's handle while the registration is live.
    live: Option<H>,
}

/// The hotkeys of one platform object.
pub(crate) struct Table<H> {
    entries: BTreeMap<HotkeyId, Entry<H>>,
    /// True while the listener runs: registrations are live.
    active: bool,
}

impl<H> Default for Table<H> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            active: false,
        }
    }
}

impl<H> Table<H> {
    /// Whether `chord` could be registered under `id` as far as this table can tell.
    ///
    /// # Errors
    ///
    /// [`Invalid`](PlatformError::Invalid) when the system cannot express the chord;
    /// [`HotkeyUnavailable`](PlatformError::HotkeyUnavailable) when another id holds it.
    pub(crate) fn check(&self, id: HotkeyId, chord: Chord) -> Result<()> {
        self.admit(id, chord).map(|_| ())
    }

    /// The combo of `chord`, unless another id of this table holds it.
    fn admit(&self, id: HotkeyId, chord: Chord) -> Result<Combo> {
        let combo = Combo::from_chord(chord)?;
        let taken = self
            .entries
            .iter()
            .any(|(other, entry)| *other != id && entry.combo == combo);
        if taken {
            return Err(PlatformError::HotkeyUnavailable);
        }
        Ok(combo)
    }

    /// Records `chord` for `id` if that needs no system call: the listener is stopped, or
    /// the same chord is live already. `Ok(false)` means the caller must come back with a
    /// backend ([`register`](Self::register)).
    ///
    /// # Errors
    ///
    /// As [`register`](Self::register), for the problems that show without the system.
    pub(crate) fn try_record(&mut self, id: HotkeyId, chord: Chord) -> Result<bool> {
        let combo = self.admit(id, chord)?;
        if self.active {
            let live_already = self
                .entries
                .get(&id)
                .is_some_and(|entry| entry.combo == combo && entry.live.is_some());
            return Ok(live_already);
        }
        self.entries.insert(id, Entry { combo, live: None });
        Ok(true)
    }

    /// Registers `chord` under `id`, replacing the id's previous chord. While the listener
    /// is stopped this only records the wish.
    ///
    /// # Errors
    ///
    /// [`Invalid`](PlatformError::Invalid) when the system cannot express the chord;
    /// [`HotkeyUnavailable`](PlatformError::HotkeyUnavailable) when another id or another
    /// application holds it; [`Os`](PlatformError::Os) for any other refusal. On an error the
    /// id keeps what it had.
    pub(crate) fn register<B: Backend<Handle = H>>(
        &mut self,
        id: HotkeyId,
        chord: Chord,
        backend: &mut B,
    ) -> Result<()> {
        if self.try_record(id, chord)? {
            return Ok(());
        }
        let combo = self.admit(id, chord)?;
        // The new chord first: if the system refuses it, the old one stays live.
        let handle = backend.register(id, combo)?;
        let replaced = self.entries.insert(
            id,
            Entry {
                combo,
                live: Some(handle),
            },
        );
        if let Some(old) = replaced.and_then(|entry| entry.live) {
            backend.unregister(old);
        }
        Ok(())
    }

    /// Forgets `id` if that needs no system call: it is unknown, or not live. `false` means
    /// the caller must come back with a backend ([`unregister`](Self::unregister)).
    pub(crate) fn try_forget(&mut self, id: HotkeyId) -> bool {
        if self
            .entries
            .get(&id)
            .is_some_and(|entry| entry.live.is_some())
        {
            return false;
        }
        self.entries.remove(&id);
        true
    }

    /// Removes the registration of `id`. An unknown id is not an error.
    pub(crate) fn unregister<B: Backend<Handle = H>>(&mut self, id: HotkeyId, backend: &mut B) {
        if let Some(handle) = self.entries.remove(&id).and_then(|entry| entry.live) {
            backend.unregister(handle);
        }
    }

    /// The listener started: registers every wanted chord with the system. A chord the
    /// system refuses stays wanted and is tried again at the next start, or when the service
    /// registers it again.
    pub(crate) fn activate<B: Backend<Handle = H>>(&mut self, backend: &mut B) {
        self.active = true;
        for (id, entry) in &mut self.entries {
            if entry.live.is_none() {
                entry.live = backend.register(*id, entry.combo).ok();
            }
        }
    }

    /// The listener stops: releases every registration and keeps the wishes.
    pub(crate) fn deactivate<B: Backend<Handle = H>>(&mut self, backend: &mut B) {
        self.active = false;
        for entry in self.entries.values_mut() {
            if let Some(handle) = entry.live.take() {
                backend.unregister(handle);
            }
        }
    }

    /// How many registrations are live.
    pub(crate) fn live_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.live.is_some())
            .count()
    }

    /// The chord wanted for `id`, in the system's form.
    #[cfg(test)]
    pub(crate) fn wanted(&self, id: HotkeyId) -> Option<Combo> {
        self.entries.get(&id).map(|entry| entry.combo)
    }

    /// Whether the registration of `id` is live.
    #[cfg(test)]
    pub(crate) fn is_live(&self, id: HotkeyId) -> bool {
        self.entries
            .get(&id)
            .is_some_and(|entry| entry.live.is_some())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use iamb_platform::HotkeyKey;
    use iamb_platform::core_types::KeyName;

    use super::*;

    /// A system that hands out numbered handles and can be told that a chord is taken.
    #[derive(Default)]
    struct FakeSystem {
        next: u32,
        live: BTreeMap<u32, (HotkeyId, Combo)>,
        taken_elsewhere: BTreeSet<(u32, u32)>,
        refuse_with: Option<PlatformError>,
        log: Vec<String>,
    }

    impl FakeSystem {
        fn live_ids(&self) -> Vec<u32> {
            self.live.values().map(|(id, _)| id.0).collect()
        }
    }

    impl Backend for FakeSystem {
        type Handle = u32;

        fn register(&mut self, id: HotkeyId, combo: Combo) -> Result<u32> {
            if let Some(error) = &self.refuse_with {
                return Err(error.clone());
            }
            let held = self.live.values().any(|(_, live)| *live == combo);
            if held
                || self
                    .taken_elsewhere
                    .contains(&(combo.key_code, combo.modifiers))
            {
                self.log.push(format!("refuse {}", id.0));
                return Err(PlatformError::HotkeyUnavailable);
            }
            self.next += 1;
            self.live.insert(self.next, (id, combo));
            self.log.push(format!("register {}", id.0));
            Ok(self.next)
        }

        fn unregister(&mut self, handle: u32) {
            let (id, _) = self.live.remove(&handle).expect("a live handle");
            self.log.push(format!("unregister {}", id.0));
        }
    }

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("a valid chord")
    }

    fn combo(text: &str) -> Combo {
        Combo::from_chord(chord(text)).expect("a chord a Mac can express")
    }

    const PALETTE: HotkeyId = HotkeyId(1);
    const PAUSE: HotkeyId = HotkeyId(2);

    #[test]
    fn chords_map_to_key_codes_and_the_systems_modifier_bits() {
        let combo = Combo::from_chord(chord("Ctrl+Shift+Space")).expect("expressible");
        assert_eq!(combo.key_code, 49);
        assert_eq!(combo.modifiers, (1 << 12) | (1 << 9));
        // Meta is Command, Alt is Option.
        let combo = Combo::from_chord(chord("Meta+Alt+L")).expect("expressible");
        assert_eq!(combo.key_code, 37);
        assert_eq!(combo.modifiers, (1 << 8) | (1 << 11));
        // A function key may stand alone.
        let combo = Combo::from_chord(chord("F5")).expect("expressible");
        assert_eq!((combo.key_code, combo.modifiers), (96, 0));
        let all = Combo::from_chord(chord("Ctrl+Alt+Shift+Meta+1")).expect("expressible");
        assert_eq!(all.modifiers, 256 | 512 | 2048 | 4096);
        assert_eq!(all.key_code, 18);
    }

    #[test]
    fn a_key_a_mac_does_not_have_is_invalid() {
        for number in 21..=24 {
            let chord = Chord::new(Modifiers::NONE, HotkeyKey::Function(number)).expect("valid");
            assert!(matches!(
                Combo::from_chord(chord),
                Err(PlatformError::Invalid { .. })
            ));
            let mut table = Table::<u32>::default();
            assert!(matches!(
                table.try_record(PALETTE, chord),
                Err(PlatformError::Invalid { .. })
            ));
        }
        let f20 = Chord::new(Modifiers::NONE, HotkeyKey::Function(20)).expect("valid");
        assert!(Combo::from_chord(f20).is_ok());
        let named =
            Chord::new(Modifiers::CTRL, HotkeyKey::Named(KeyName::PageDown)).expect("valid");
        assert_eq!(Combo::from_chord(named).expect("expressible").key_code, 121);
    }

    #[test]
    fn registrations_made_while_stopped_are_applied_at_start_and_again_after_a_restart() {
        let mut system = FakeSystem::default();
        let mut table = Table::default();
        assert_eq!(table.try_record(PALETTE, chord("Ctrl+Space")), Ok(true));
        assert_eq!(table.try_record(PAUSE, chord("Ctrl+Shift+P")), Ok(true));
        assert!(
            system.log.is_empty(),
            "nothing is held while nobody listens"
        );
        assert_eq!(table.live_count(), 0);

        table.activate(&mut system);
        assert_eq!(system.live_ids(), [1, 2]);
        assert_eq!(table.live_count(), 2);

        table.deactivate(&mut system);
        assert!(system.live.is_empty());
        assert_eq!(table.live_count(), 0);
        assert_eq!(table.wanted(PALETTE), Some(combo("Ctrl+Space")));

        table.activate(&mut system);
        assert_eq!(system.live_ids(), [1, 2]);
        assert_eq!(
            system.log,
            [
                "register 1",
                "register 2",
                "unregister 1",
                "unregister 2",
                "register 1",
                "register 2"
            ]
        );
    }

    #[test]
    fn a_registration_while_running_goes_to_the_system_at_once() {
        let mut system = FakeSystem::default();
        let mut table = Table::default();
        table.activate(&mut system);
        // Without a backend the table says it needs one and changes nothing.
        assert_eq!(table.try_record(PALETTE, chord("Ctrl+Space")), Ok(false));
        assert_eq!(table.wanted(PALETTE), None);

        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );
        assert!(table.is_live(PALETTE));
        assert_eq!(system.live_ids(), [1]);
        // The same chord again is nothing to do, with or without a backend.
        assert_eq!(table.try_record(PALETTE, chord("ctrl+space")), Ok(true));
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );
        assert_eq!(system.log, ["register 1"]);
    }

    #[test]
    fn a_chord_held_by_another_id_is_unavailable() {
        let mut system = FakeSystem::default();
        let mut table = Table::default();
        assert_eq!(table.check(PALETTE, chord("Ctrl+Space")), Ok(()));
        assert_eq!(table.try_record(PALETTE, chord("Ctrl+Space")), Ok(true));
        // Checking changes nothing and gives the answer `register` would.
        assert_eq!(table.check(PALETTE, chord("Ctrl+Space")), Ok(()));
        assert_eq!(
            table.check(PAUSE, chord("Ctrl+Space")),
            Err(PlatformError::HotkeyUnavailable)
        );
        assert_eq!(table.wanted(PAUSE), None);
        assert_eq!(
            table.try_record(PAUSE, chord("Ctrl+Space")),
            Err(PlatformError::HotkeyUnavailable)
        );
        table.activate(&mut system);
        assert_eq!(
            table.register(PAUSE, chord("Ctrl+Space"), &mut system),
            Err(PlatformError::HotkeyUnavailable)
        );
        assert_eq!(table.wanted(PAUSE), None);
        // The system was not even asked.
        assert_eq!(system.log, ["register 1"]);
    }

    #[test]
    fn a_chord_held_by_another_application_is_unavailable() {
        let mut system = FakeSystem::default();
        let taken = Combo::from_chord(chord("Meta+Space")).expect("expressible");
        system
            .taken_elsewhere
            .insert((taken.key_code, taken.modifiers));
        let mut table = Table::default();
        table.activate(&mut system);
        assert_eq!(
            table.register(PALETTE, chord("Meta+Space"), &mut system),
            Err(PlatformError::HotkeyUnavailable)
        );
        assert_eq!(table.wanted(PALETTE), None);
        assert_eq!(table.live_count(), 0);
        // Another chord is fine.
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );
    }

    #[test]
    fn registering_an_id_again_replaces_its_chord() {
        let mut system = FakeSystem::default();
        let mut table = Table::default();
        table.activate(&mut system);
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Shift+K"), &mut system),
            Ok(())
        );
        assert_eq!(table.wanted(PALETTE), Some(combo("Ctrl+Shift+K")));
        assert_eq!(system.live.len(), 1);
        // New first, then the old one is released.
        assert_eq!(system.log, ["register 1", "register 1", "unregister 1"]);
        // The old chord is free for another id.
        assert_eq!(
            table.register(PAUSE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );

        // While stopped, replacing is only a note.
        table.deactivate(&mut system);
        assert_eq!(table.try_record(PALETTE, chord("F6")), Ok(true));
        assert_eq!(table.wanted(PALETTE), Some(combo("F6")));
    }

    #[test]
    fn a_refused_replacement_keeps_the_old_chord_live() {
        let mut system = FakeSystem::default();
        let taken = Combo::from_chord(chord("Meta+Space")).expect("expressible");
        system
            .taken_elsewhere
            .insert((taken.key_code, taken.modifiers));
        let mut table = Table::default();
        table.activate(&mut system);
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );
        assert_eq!(
            table.register(PALETTE, chord("Meta+Space"), &mut system),
            Err(PlatformError::HotkeyUnavailable)
        );
        assert_eq!(table.wanted(PALETTE), Some(combo("Ctrl+Space")));
        assert!(table.is_live(PALETTE));
        assert_eq!(system.live_ids(), [1]);

        system.refuse_with = Some(PlatformError::os(-50, "RegisterEventHotKey"));
        assert_eq!(
            table.register(PALETTE, chord("F7"), &mut system),
            Err(PlatformError::os(-50, "RegisterEventHotKey"))
        );
        assert!(table.is_live(PALETTE));
    }

    #[test]
    fn unregistering_releases_and_forgets() {
        let mut system = FakeSystem::default();
        let mut table = Table::default();
        // Unknown ids are fine, with or without a backend.
        assert!(table.try_forget(PALETTE));
        table.unregister(PALETTE, &mut system);

        table.activate(&mut system);
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Ok(())
        );
        // A live registration needs the system.
        assert!(!table.try_forget(PALETTE));
        assert!(table.is_live(PALETTE));
        table.unregister(PALETTE, &mut system);
        assert_eq!(table.wanted(PALETTE), None);
        assert!(system.live.is_empty());
        assert_eq!(system.log, ["register 1", "unregister 1"]);

        // While stopped, forgetting needs nothing.
        table.deactivate(&mut system);
        assert_eq!(table.try_record(PAUSE, chord("F8")), Ok(true));
        assert!(table.try_forget(PAUSE));
        table.activate(&mut system);
        assert!(system.live.is_empty());
    }

    #[test]
    fn a_chord_refused_at_start_stays_wanted_and_is_tried_again() {
        let mut system = FakeSystem::default();
        let taken = Combo::from_chord(chord("Ctrl+Space")).expect("expressible");
        system
            .taken_elsewhere
            .insert((taken.key_code, taken.modifiers));
        let mut table = Table::default();
        assert_eq!(table.try_record(PALETTE, chord("Ctrl+Space")), Ok(true));
        assert_eq!(table.try_record(PAUSE, chord("F9")), Ok(true));

        table.activate(&mut system);
        assert!(!table.is_live(PALETTE));
        assert!(table.is_live(PAUSE));
        assert_eq!(table.wanted(PALETTE), Some(combo("Ctrl+Space")));
        // Registering it again while running asks the system again, and reports the refusal.
        assert_eq!(table.try_record(PALETTE, chord("Ctrl+Space")), Ok(false));
        assert_eq!(
            table.register(PALETTE, chord("Ctrl+Space"), &mut system),
            Err(PlatformError::HotkeyUnavailable)
        );
        // The wish survives the refusal.
        assert_eq!(table.wanted(PALETTE), Some(combo("Ctrl+Space")));

        // The other application lets go: the next start gets it.
        system.taken_elsewhere.clear();
        table.deactivate(&mut system);
        table.activate(&mut system);
        assert!(table.is_live(PALETTE));
        assert_eq!(table.live_count(), 2);
    }
}
