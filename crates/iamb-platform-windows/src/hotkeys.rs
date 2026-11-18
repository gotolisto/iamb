//! Global hotkeys (architecture §4.5, §5; the trait docs of
//! [`Hotkeys`](iamb_platform::Hotkeys)).
//!
//! A hotkey is registered with the system's hotkey call, never taken from the keyboard hook,
//! so the listener stays observe-only (ADR-0010). The call ties a hotkey to the thread that
//! makes it: that thread's message queue receives the press. Iamb's hotkeys therefore belong
//! to the listener thread, which delivers a press as
//! [`ListenerEvent::Hotkey`](iamb_platform::ListenerEvent::Hotkey) through the listener's
//! sink, with the next sequence number.
//!
//! - `register` and `unregister` hand the work to the listener thread and wait for its
//!   answer. While the listener is stopped, `register` only checks that the chord is free (by
//!   registering and releasing it on the calling thread) and keeps it; `start` registers
//!   everything that is kept, and so does every restart.
//! - A chord that another program holds, or another id of Iamb's, is
//!   [`HotkeyUnavailable`](iamb_platform::PlatformError::HotkeyUnavailable). A chord that was
//!   free when it was kept and is taken when the listener starts cannot be reported by
//!   `start`; it stays kept, and registering it again reports the refusal.
//! - Holding a chord down fires it once.
//! - The key of a registered chord never reaches the application in front, so the listener
//!   does not report it as a key either (`listener/pipeline.rs`).
//!
//! `registry` holds the part that makes no system call and is tested on every system.

// The registry is only used by the Windows-only code; on other systems it exists for its
// tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod registry;

#[cfg(windows)]
pub(crate) use registry::Registration;

#[cfg(windows)]
pub(crate) use windows::State;

#[cfg(windows)]
mod windows {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use iamb_platform::{Chord, HotkeyId, Hotkeys, Result};

    use super::registry::{Plan, Registration, Registry};
    use crate::Platform;
    use crate::listener::os;

    /// The registered chords. Locked before the listener's own state, by `start` and `stop`
    /// too, so that a registration and a restart cannot overtake each other.
    #[derive(Debug, Default)]
    pub(crate) struct State {
        pub(crate) registry: Mutex<Registry>,
    }

    impl State {
        fn registry(&self) -> MutexGuard<'_, Registry> {
            // A panic while the registry was locked left whole entries behind.
            self.registry.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// Checks that the system would accept `registration`, on the calling thread, and leaves
    /// nothing registered.
    fn check_free(registration: Registration) -> Result<()> {
        os::register_hotkey(
            registration.os_id,
            registration.modifier_flags(),
            registration.virtual_key(),
        )?;
        os::unregister_hotkey(registration.os_id)
    }

    impl Hotkeys for Platform {
        fn register(&self, id: HotkeyId, chord: Chord) -> Result<()> {
            let mut registry = self.hotkeys.registry();
            let link = self.listener.link();
            match registry.plan(id, chord, link.is_some())? {
                Plan::Unchanged => Ok(()),
                Plan::Register { new, replaces } => {
                    match &link {
                        Some(link) => {
                            link.register(new)?;
                            if let Some(old) = replaces {
                                // The new chord works; a refusal to let the old one go
                                // leaves a hotkey that fires for an id nobody knows, which
                                // the listener thread ignores.
                                let _ = link.unregister(old);
                            }
                        }
                        None => check_free(new)?,
                    }
                    registry.commit(new, link.is_some());
                    Ok(())
                }
            }
        }

        fn unregister(&self, id: HotkeyId) -> Result<()> {
            let mut registry = self.hotkeys.registry();
            let Some(os_id) = registry.remove(id) else {
                return Ok(());
            };
            match self.listener.link() {
                Some(link) => link.unregister(os_id),
                // The thread ended and its hotkeys with it.
                None => Ok(()),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use iamb_platform::PlatformError;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse as kam;

        use super::super::registry;
        use super::*;

        #[test]
        fn the_modifier_flags_written_out_for_every_system_are_the_systems_own() {
            assert_eq!(registry::MOD_ALT, kam::MOD_ALT);
            assert_eq!(registry::MOD_CONTROL, kam::MOD_CONTROL);
            assert_eq!(registry::MOD_SHIFT, kam::MOD_SHIFT);
            assert_eq!(registry::MOD_WIN, kam::MOD_WIN);
            assert_eq!(registry::MOD_NOREPEAT, kam::MOD_NOREPEAT);
        }

        #[test]
        fn a_chord_registered_while_stopped_is_kept_and_unknown_ids_are_no_error() {
            let platform = Platform::new();
            // A chord nobody uses. The check on the calling thread needs no input events, but
            // it needs a session that has hotkeys at all.
            let chord = Chord::parse("Ctrl+Alt+Shift+F21").expect("a valid chord");
            if let Err(PlatformError::Os { code, context }) = platform.register(HotkeyId(1), chord)
            {
                assert!(
                    std::env::var_os("IAMB_REQUIRE_DESKTOP").is_none(),
                    "IAMB_REQUIRE_DESKTOP is set, but {context} failed with code {code}"
                );
                eprintln!(
                    "SKIPPED: this session has no hotkeys ({context} failed with code {code})"
                );
                return;
            }
            assert_eq!(platform.register(HotkeyId(1), chord), Ok(()));
            assert_eq!(platform.register(HotkeyId(1), chord), Ok(()), "unchanged");
            assert_eq!(
                platform.register(HotkeyId(2), chord),
                Err(PlatformError::HotkeyUnavailable),
                "another id holds it"
            );
            assert_eq!(platform.unregister(HotkeyId(1)), Ok(()));
            assert_eq!(platform.unregister(HotkeyId(1)), Ok(()));
            assert_eq!(platform.unregister(HotkeyId(7)), Ok(()));
            assert_eq!(platform.register(HotkeyId(2), chord), Ok(()), "free again");
        }
    }
}
