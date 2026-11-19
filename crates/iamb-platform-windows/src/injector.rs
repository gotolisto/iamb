//! The injector (architecture §4.2, §4.6, §5; ADR-0019; the protocol in the trait docs of
//! [`Injector`](iamb_platform::Injector)).
//!
//! # Parts
//!
//! | Module | Runs on | Does |
//! | --- | --- | --- |
//! | `strategy` | every system | the per-application strategy table and the user's overrides |
//! | `plan` | every system | steps to key events and batches: UTF-16, line breaks, type or paste, pacing, held modifiers |
//! | `run` | every system | one expansion, start marker to end marker, against a small `Desk` trait |
//! | `os` | Windows | `SendInput`, the modifier keys, the paste through the clipboard |
//!
//! The first four make no system call and are tested on the Linux build machine.
//!
//! # How an expansion is sent
//!
//! The start marker is the release of an unassigned key, marked
//! [`InjectionStart`](iamb_platform::Marker::InjectionStart) in its extra-info field, sent on
//! its own. Once the hand-off has sealed, the deletion (the expansion's own plus the `k`
//! characters typed after the trigger), the text, the key presses, the caret moves, the `k`
//! characters again and the end marker go to `SendInput` in **one call**, which the system
//! inserts into the input stream as a whole: a real key cannot land in the middle. Only a
//! `Wait` step, a paste and a strategy's pacing split the output into several calls. Every
//! output event carries [`Output`](iamb_platform::Marker::Output).
//!
//! Text goes as `KEYEVENTF_UNICODE` events of its UTF-16 code units (a surrogate pair is two
//! units), so accents, emoji sequences, flags and CJK arrive as written, independent of the
//! keyboard layout. Line breaks press Enter, or Shift+Enter where the strategy table says
//! Enter sends a message. Text longer than the strategy's threshold (200 characters by
//! default) is pasted: snapshot of the clipboard, transient write, Ctrl+V, a wait for the
//! application to read it (150 ms by default), restore. The restore runs on every path, a
//! failed paste included (`clipboard::paste`).
//!
//! A modifier key the user still holds when a batch goes out (the Shift of a `:` delimiter)
//! is released at the front of the batch, so Backspace, Left and Enter are not taken as
//! chords; it is not pressed again afterwards, which could leave it stuck.
//!
//! # User overrides
//!
//! The service passes the `injection_overrides` setting to
//! [`Injector::configure`](iamb_platform::Injector::configure). The format is the one both
//! systems share (`iamb_platform::injection_settings`, ADR-0025); `strategy` documents the
//! fields only Windows has.
//!
//! # Failures
//!
//! A window of a higher privilege level than Iamb's (UIPI), or another desktop (lock screen,
//! secure desktop), makes `SendInput` take fewer events than it was given. The outcome is
//! [`Failed`](iamb_platform::InjectionOutcome::Failed) with
//! [`InjectionRefused`](iamb_platform::PlatformError::InjectionRefused); once the start
//! marker went out, no end marker follows and the service answers with
//! `InjectionCancelled` (`run` explains why). Nothing panics.
//!
//! # Limits
//!
//! - **The focus between batches** (`run`). The guard compares the foreground window, its
//!   process and the control the system names as focused. A window that draws its own fields
//!   (a browser engine) has one control for all of them, so a move between its fields after
//!   the start marker is not seen; the service's cancel before the start marker is the only
//!   guard there. Inside one batch nothing is compared: the system delivers it whole.
//! - **A dead key pressed between the trigger and the start marker.** It produces no
//!   character, so the hand-off does not count it, and the application holds it pending: the
//!   first Backspace of the deletion is spent cancelling it, and one character of the
//!   abbreviation stays. Only the listener sees a pending dead key; the injector cannot ask
//!   the target without disturbing its keyboard state. The listener handles it: when the
//!   start marker arrives while a dead key waits or may wait, it reports a boundary just
//!   before `InjectionStarted`, the hand-off cancels, and nothing is sent.
//! - **Backspace in some applications deletes a whole grapheme cluster**, others one UTF-16
//!   unit. The engine counts characters; abbreviations are typed text, so this matters only
//!   for characters typed after the trigger that are outside the Basic Multilingual Plane.
//! - **Pacing gives up atomicity.** With a key delay (remote-desktop clients), real keys can
//!   land between Iamb's keys; the final call still carries the last key, the retyped
//!   characters and the end marker together.
//! - **The clipboard is restored after a fixed wait**, not when the application has read it:
//!   the system does not say when that happens. A slow application can be given a longer
//!   `paste_wait_ms`.
//!
//! # Privacy (architecture §8)
//!
//! The retyped characters are copied into the input batch under the hand-off's lock and the
//! batch is overwritten with zeros after the call; every batch is. Nothing here logs; errors
//! carry no text; the `Debug` of a key event shows no character.

// The parts that run on every system are only called from the Windows-only parts; on other
// systems they exist for their tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod plan;
mod run;
mod strategy;

#[cfg(windows)]
mod os;

#[cfg(windows)]
pub(crate) use windows::State;

#[cfg(windows)]
mod windows {
    use std::sync::{Mutex, PoisonError, RwLock};

    use iamb_platform::{
        InjectionOutcome, InjectionRequest, Injector, Result, START_MARKER_TIMEOUT, TypedHandoff,
    };

    use super::os::SystemDesk;
    use super::run;
    use super::strategy::StrategyTable;
    use crate::Platform;

    /// The injector's state: the strategy table with the user's overrides, and the lock that
    /// keeps one expansion in flight.
    #[derive(Debug, Default)]
    pub(crate) struct State {
        table: RwLock<StrategyTable>,
        in_flight: Mutex<()>,
    }

    impl Injector for Platform {
        /// Replaces the user's per-application injection overrides with `settings_json`, in
        /// the shared format of `iamb_platform::injection_settings` read as its Windows
        /// dialect (an empty string clears them). An expansion being sent keeps the strategy
        /// it started with.
        ///
        /// # Errors
        ///
        /// [`Invalid`](iamb_platform::PlatformError::Invalid) when the settings are not valid
        /// (malformed JSON, an unknown field or value, a macOS-only field or line break, a
        /// value out of range); the overrides in force stay as they were. The error names
        /// what was wrong and quotes nothing from the settings.
        fn configure(&self, settings_json: &str) -> Result<()> {
            let table = StrategyTable::from_settings(settings_json)?;
            *self
                .injector
                .table
                .write()
                .unwrap_or_else(PoisonError::into_inner) = table;
            Ok(())
        }

        fn inject(&self, request: &InjectionRequest, typed: &TypedHandoff) -> InjectionOutcome {
            let _one_at_a_time = self
                .injector
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let strategy = self
                .injector
                .table
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .resolve(
                    request.app.as_ref().map(|app| app.0.as_str()),
                    &request.overrides,
                );
            run::inject(
                &mut SystemDesk,
                request,
                typed,
                &strategy,
                START_MARKER_TIMEOUT,
            )
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::PoisonError;

        use iamb_platform::{InjectionOverrides, Injector, PlatformError};

        use super::super::strategy::Typing;
        use crate::Platform;

        fn typing_for(platform: &Platform, app: &str) -> Typing {
            platform
                .injector
                .table
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .resolve(Some(app), &InjectionOverrides::default())
                .typing
        }

        #[test]
        fn overrides_are_validated_and_kept() {
            let platform = Platform::new();
            assert_eq!(
                platform.configure(r#"{"apps": {"x.exe": {"method": "keys"}}}"#),
                Ok(())
            );
            assert_eq!(typing_for(&platform, "x.exe"), Typing::Keys);
            assert!(matches!(
                platform.configure("{"),
                Err(PlatformError::Invalid { .. })
            ));
            // The old Windows shape and macOS-only fields are refused; the settings stay.
            for refused in [
                r#"{"apps": {"x.exe": {"typing": "paste"}}}"#,
                r#"{"apps": {"x.exe": {"method": "paste", "chunk_units": 2}}}"#,
            ] {
                assert!(matches!(
                    platform.configure(refused),
                    Err(PlatformError::Invalid { .. })
                ));
            }
            assert_eq!(typing_for(&platform, "x.exe"), Typing::Keys);
            assert_eq!(platform.configure(""), Ok(()));
            assert_eq!(typing_for(&platform, "x.exe"), Typing::Auto);
        }
    }
}
