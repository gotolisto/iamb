//! The key listener: a listen-only event tap on its own thread (architecture §3.1, §4.1, §5;
//! ADR-0010, ADR-0011, ADR-0021; the trait docs of
//! [`KeyListener`](iamb_platform::KeyListener)).
//!
//! # Map
//!
//! What the listener decides is plain Rust, built and tested on every system. Only the code
//! that calls macOS is behind `cfg(target_os = "macos")`.
//!
//! | Module | Built on | Holds |
//! | --- | --- | --- |
//! | [`keycodes`] | every system | virtual key codes; which keys never type text |
//! | [`modifiers`] | every system | the modifiers of an event; why Command and Control chords are never text |
//! | [`translate`] | every system | the keyboard-layout seam, the listener's own dead-key state, the text / Backspace / boundary rules |
//! | [`inputs`] | every system | the seam for "which layout, and is an input method selected"; what `Composing` means here |
//! | [`recovery`] | every system | the state machine for a tap the system disabled |
//! | [`selftest`] | every system | the rendezvous between `self_test` and the callback |
//! | [`reply`] | every system | the answer of a job handed to another thread, with a deadline |
//! | [`stats`] | every system | the callback's duration histogram and counters |
//! | [`core`] | every system | what the callback does with one event; markers; why other software's synthetic events are typing |
//! | [`target`] | every system | where a key went: the event's target process, the frontmost application as last noted, or no stamp |
//! | `sys` | macOS | every hand-written `extern` declaration of this crate |
//! | `main_thread` | macOS | running work on the main thread |
//! | `layout` | macOS | the system's keyboard layout, read on the main thread, translated with `UCKeyTranslate` |
//! | `tap` | macOS | the event tap, its callback and its thread |
//! | `mac` | macOS | [`KeyListener`](iamb_platform::KeyListener) for `Platform` |
//!
//! # What the worker's main thread must do
//!
//! The system's header asks an application to call the keyboard-layout functions on the main
//! thread, and hotkey presses are delivered through the application's event loop there. The
//! listener thread never calls those functions; it asks the main thread (`main_thread`). So:
//!
//! - **The worker's main thread must run its run loop** (for hotkeys: the application event
//!   loop). `start` called from another thread waits a moment for the main thread to read the
//!   keyboard layout and fails with an [`Os`](iamb_platform::PlatformError::Os) error naming
//!   the main run loop when it does not answer. `start` called on the main thread itself needs
//!   nothing.
//! - Under the standard test harness the main thread is busy running the harness, so tests
//!   that need a keyboard layout load it in a static constructor, which runs on the main
//!   thread before the harness starts (`mac_tests`).
//!
//! # Start, stop, self-test
//!
//! - `start` refuses a second start with `AlreadyRunning`, and refuses to create a tap
//!   without Input Monitoring and Accessibility (`PermissionDenied(kind)`): a tap created
//!   without them would see nothing, and creating it would make the system prompt uninvited.
//! - `stop` removes the tap, ends the thread and drops the sink; stopping a stopped listener
//!   succeeds. Sequence numbers continue across a restart.
//! - `self_test` posts a marked key release and waits for the tap to see it. A tap found
//!   disabled is enabled and tested once more. `NotRunning` when stopped,
//!   `PermissionDenied(kind)` when a permission was revoked while the listener ran, `Timeout`
//!   otherwise. While secure keyboard entry is on (a password field has the focus) the system
//!   shows no keyboard event to any tap; a test that fails then counts as passed, because the
//!   tap is not at fault and reinstalling it would change nothing.
//!
//! # Privacy
//!
//! Typed characters pass through the callback on their way to the sink and are not kept: the
//! listener's only state derived from typing is the 32-bit dead-key state of
//! [`translate::Translator`]. Types that hold a key or a character have no `Debug`; errors
//! carry static text only; the counters in [`stats`] are numbers. Nothing in this module
//! writes a log.

mod core;
mod inputs;
mod keycodes;
mod modifiers;
mod recovery;
mod reply;
mod selftest;
mod stats;
pub(crate) mod target;
mod translate;

#[cfg(test)]
mod testing;

#[cfg(target_os = "macos")]
mod layout;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub(crate) mod main_thread;
#[cfg(target_os = "macos")]
pub(crate) mod sys;
#[cfg(target_os = "macos")]
mod tap;

#[cfg(all(test, target_os = "macos"))]
mod mac_tests;

use iamb_platform::{PermissionKind, PlatformError, Result};

pub(crate) use keycodes::hotkey_code;
#[cfg(target_os = "macos")]
pub(crate) use mac::State;
#[cfg(target_os = "macos")]
pub(crate) use tap::{Request, Shared};

/// The gate every start goes through: one listener at a time, and no tap without both
/// permissions.
///
/// `slot` holds the running listener. `missing` is the permission that is not granted, if
/// any. `launch` creates the tap; it is not called when the gate refuses, so a refused start
/// has no side effect.
pub(crate) fn start_gate<L>(
    slot: &mut Option<L>,
    missing: Option<PermissionKind>,
    launch: impl FnOnce() -> Result<L>,
) -> Result<()> {
    if slot.is_some() {
        return Err(PlatformError::AlreadyRunning);
    }
    if let Some(kind) = missing {
        return Err(PlatformError::PermissionDenied(kind));
    }
    *slot = Some(launch()?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::policy::missing_for_listener;
    use crate::permissions::policy::testing::FakeProbe;

    fn never() -> Result<u8> {
        panic!("a refused start must not create a tap");
    }

    #[test]
    fn a_granted_start_launches_once() {
        let mut slot = None;
        let granted = FakeProbe::new(true, true);
        assert_eq!(
            start_gate(&mut slot, missing_for_listener(&granted), || Ok(7)),
            Ok(())
        );
        assert_eq!(slot, Some(7));
        assert_eq!(
            start_gate(&mut slot, missing_for_listener(&granted), never),
            Err(PlatformError::AlreadyRunning)
        );
        assert_eq!(slot, Some(7));
    }

    #[test]
    fn a_missing_permission_refuses_the_start_and_creates_no_tap() {
        for (accessibility, input_monitoring, kind) in [
            (false, false, PermissionKind::InputMonitoring),
            (true, false, PermissionKind::InputMonitoring),
            (false, true, PermissionKind::Accessibility),
        ] {
            let mut slot = None;
            let probe = FakeProbe::new(accessibility, input_monitoring);
            assert_eq!(
                start_gate(&mut slot, missing_for_listener(&probe), never),
                Err(PlatformError::PermissionDenied(kind))
            );
            assert_eq!(slot, None);
            // The refused start asked for nothing and opened nothing.
            assert!(probe.calls().is_empty());
        }
    }

    #[test]
    fn a_permission_revoked_between_two_starts_refuses_the_second() {
        let probe = FakeProbe::new(true, true);
        let mut slot = None;
        assert_eq!(
            start_gate(&mut slot, missing_for_listener(&probe), || Ok(1)),
            Ok(())
        );
        // The worker stops the listener; meanwhile the user revokes Input Monitoring.
        slot = None;
        probe.input_monitoring.set(false);
        assert_eq!(
            start_gate(&mut slot, missing_for_listener(&probe), never),
            Err(PlatformError::PermissionDenied(
                PermissionKind::InputMonitoring
            ))
        );
        // Granted again: the next start works.
        probe.input_monitoring.set(true);
        assert_eq!(
            start_gate(&mut slot, missing_for_listener(&probe), || Ok(2)),
            Ok(())
        );
        assert_eq!(slot, Some(2));
    }

    #[test]
    fn already_running_is_reported_before_a_missing_permission() {
        let mut slot = Some(1);
        assert_eq!(
            start_gate(&mut slot, Some(PermissionKind::Accessibility), never),
            Err(PlatformError::AlreadyRunning)
        );
    }

    #[test]
    fn a_failed_launch_leaves_the_listener_stopped() {
        let mut slot: Option<u8> = None;
        let failure = PlatformError::os(0, "CGEventTapCreate");
        assert_eq!(
            start_gate(&mut slot, None, || Err(failure.clone())),
            Err(failure)
        );
        assert_eq!(slot, None);
        assert_eq!(start_gate(&mut slot, None, || Ok(3)), Ok(()));
    }
}
