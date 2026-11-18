//! Hotkey registration through the public API while the listener is stopped: the wishes are
//! kept, conflicts and impossible chords are refused, and no system call is made. Runs on
//! every Mac, with or without permissions.
//!
//! Delivery of a real hotkey press needs a running listener (Input Monitoring and
//! Accessibility) and a main thread that runs the application event loop; see
//! `listener::mac_tests::with_a_tap` in the crate, which skips with its reason where either
//! is missing.

#![cfg(target_os = "macos")]

use iamb_platform::{Chord, HotkeyId, Hotkeys, PlatformError};
use iamb_platform_macos::Platform;

fn chord(text: &str) -> Chord {
    Chord::parse(text).expect("a valid chord")
}

#[test]
fn registrations_made_before_start_are_kept() {
    let platform = Platform::new();
    assert_eq!(
        platform.register(HotkeyId(1), chord("Ctrl+Shift+Space")),
        Ok(())
    );
    assert_eq!(platform.register(HotkeyId(2), chord("Meta+Alt+K")), Ok(()));
    // The same chord again is not an error, in any spelling.
    assert_eq!(
        platform.register(HotkeyId(1), chord("shift+control+space")),
        Ok(())
    );
}

#[test]
fn a_chord_held_by_another_id_is_unavailable_until_it_is_released() {
    let platform = Platform::new();
    assert_eq!(platform.register(HotkeyId(1), chord("Ctrl+Alt+P")), Ok(()));
    assert_eq!(
        platform.register(HotkeyId(2), chord("Ctrl+Alt+P")),
        Err(PlatformError::HotkeyUnavailable)
    );
    // Replacing the first id's chord frees the old one.
    assert_eq!(platform.register(HotkeyId(1), chord("Ctrl+Alt+O")), Ok(()));
    assert_eq!(platform.register(HotkeyId(2), chord("Ctrl+Alt+P")), Ok(()));
    // Unregistering frees it too, and an unknown id is not an error.
    assert_eq!(platform.unregister(HotkeyId(2)), Ok(()));
    assert_eq!(platform.unregister(HotkeyId(2)), Ok(()));
    assert_eq!(platform.unregister(HotkeyId(77)), Ok(()));
    assert_eq!(platform.register(HotkeyId(3), chord("Ctrl+Alt+P")), Ok(()));
}

#[test]
fn two_platform_objects_do_not_share_their_wishes() {
    let one = Platform::new();
    let other = Platform::new();
    assert_eq!(one.register(HotkeyId(1), chord("Ctrl+Alt+J")), Ok(()));
    // Nothing is registered with the system while the listeners are stopped, so the second
    // object's wish is only recorded.
    assert_eq!(other.register(HotkeyId(1), chord("Ctrl+Alt+J")), Ok(()));
}

#[test]
fn a_key_a_mac_keyboard_does_not_have_is_invalid() {
    let platform = Platform::new();
    for text in ["F21", "Ctrl+F22", "F24"] {
        assert!(
            matches!(
                platform.register(HotkeyId(1), chord(text)),
                Err(PlatformError::Invalid { .. })
            ),
            "{text} cannot be registered on a Mac"
        );
    }
    assert_eq!(platform.register(HotkeyId(1), chord("F20")), Ok(()));
}
