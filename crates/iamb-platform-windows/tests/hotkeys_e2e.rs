//! Global hotkeys end to end, through the public traits only: a chord is registered with the
//! system, pressed with `SendInput`, and arrives as a `Hotkey` event in the listener's stream.
//! Runs on Windows only (CI's Windows leg).
//!
//! Unlike the listener's tests, the chord's key events cannot be kept away from the system:
//! it is the system that recognises the hotkey. The chords are ones no program uses
//! (Ctrl+Alt+Shift with F22 or F23), so the window in front sees modifiers go down and up and
//! nothing else.
//!
//! A test that needs something the session does not have says `SKIPPED` and why, and fails
//! instead when `IAMB_REQUIRE_DESKTOP` is set.

#![cfg(windows)]

#[path = "listener_support.rs"]
mod support;

use std::ptr;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use iamb_platform::core_types::BoundaryKind;
use iamb_platform::{
    Chord, HotkeyId, Hotkeys, KeyListener, ListenerEvent, PlatformError, event_channel,
};
use iamb_platform_windows::Platform;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_F22, VK_F23, VK_LCONTROL,
    VK_LMENU, VK_LSHIFT,
};

use support::{Key, Seen, alone, desktop, passed, printable, send, skip};

/// Presses and releases Ctrl+Alt+Shift+`vk`, for real.
fn press_chord(vk: u16) {
    let modifiers = [VK_LCONTROL, VK_LSHIFT, VK_LMENU];
    let mut keys: Vec<Key> = modifiers
        .iter()
        .map(|m| Key::press(*m).for_real())
        .collect();
    keys.push(Key::press(vk).for_real());
    keys.push(Key::release(vk).for_real());
    keys.extend(modifiers.iter().rev().map(|m| Key::release(*m).for_real()));
    assert_eq!(send(&keys), keys.len(), "the system took the chord");
}

/// What the listener reports within `wait` after `act`, until `enough` says so. A hotkey
/// press is a message the system posts to the listener thread, so it arrives a moment later.
fn after(
    events: &Receiver<ListenerEvent>,
    wait: Duration,
    enough: impl Fn(&[Seen]) -> bool,
) -> Vec<Seen> {
    let deadline = Instant::now() + wait;
    let mut seen = Vec::new();
    loop {
        seen.extend(events.try_iter().filter_map(|event| printable(&event)));
        let left = deadline.saturating_duration_since(Instant::now());
        if enough(&seen) || left.is_zero() {
            return seen;
        }
        std::thread::sleep(left.min(Duration::from_millis(5)));
    }
}

const PATIENCE: Duration = Duration::from_secs(5);
const A_MOMENT: Duration = Duration::from_millis(300);

/// Registers `chord` under `id`, or says why this session cannot.
fn register_or_skip(platform: &Platform, id: HotkeyId, chord: Chord) -> bool {
    match platform.register(id, chord) {
        Ok(()) => true,
        Err(PlatformError::HotkeyUnavailable) => {
            skip(&format!("another program on this machine holds {chord}"));
            false
        }
        Err(PlatformError::Os { code, context }) => {
            skip(&format!(
                "this session has no hotkeys ({context} failed with code {code})"
            ));
            false
        }
        Err(other) => panic!("registering {chord} failed: {other}"),
    }
}

#[test]
fn a_registered_chord_arrives_as_a_hotkey_event() {
    let _alone = alone();
    let Some(_watcher) = desktop() else {
        return;
    };
    let platform = Platform::new();
    let palette = HotkeyId(41);
    let chord = Chord::parse("Ctrl+Alt+Shift+F22").expect("a valid chord");
    let hotkey = Seen::Hotkey(palette.0);
    let has_hotkey = |seen: &[Seen]| seen.contains(&hotkey);

    // Registered while the listener is stopped: kept, and registered when it starts.
    if !register_or_skip(&platform, palette, chord) {
        return;
    }
    let (sink, events) = event_channel(64);
    platform.start(Box::new(sink)).expect("the listener starts");
    let passed_before = passed();
    press_chord(VK_F22);
    let seen = after(&events, PATIENCE, has_hotkey);
    if passed() - passed_before == 8 {
        // Nobody else typed: the chord is the hotkey and nothing else. Its key is taken by
        // the system, so the listener does not report it as a key.
        assert_eq!(seen, std::slice::from_ref(&hotkey));
    } else {
        assert!(has_hotkey(&seen), "no hotkey event in {seen:?}");
    }

    // Registered again after a restart.
    platform.stop().expect("the listener stops");
    let (sink, events) = event_channel(64);
    platform
        .start(Box::new(sink))
        .expect("the listener starts again");
    press_chord(VK_F22);
    let seen = after(&events, PATIENCE, has_hotkey);
    assert!(
        has_hotkey(&seen),
        "no hotkey event after a restart in {seen:?}"
    );

    // Unregistered while the listener runs: the chord is an ordinary shortcut again.
    assert_eq!(platform.unregister(palette), Ok(()));
    press_chord(VK_F22);
    let shortcut = Seen::Boundary(BoundaryKind::Shortcut);
    let seen = after(&events, A_MOMENT, |_| false);
    assert!(!has_hotkey(&seen), "a hotkey event after unregistering");
    assert!(seen.contains(&shortcut), "no shortcut in {seen:?}");

    // Registered while the listener runs.
    assert_eq!(platform.register(palette, chord), Ok(()));
    press_chord(VK_F22);
    let seen = after(&events, PATIENCE, has_hotkey);
    assert!(has_hotkey(&seen), "no hotkey event in {seen:?}");

    assert_eq!(platform.unregister(palette), Ok(()));
    assert_eq!(
        platform.unregister(palette),
        Ok(()),
        "unknown ids are no error"
    );
    platform.stop().expect("the listener stops");
}

#[test]
fn a_chord_that_is_taken_is_unavailable() {
    const OTHER_PROGRAM: i32 = 0x1AB;
    let _alone = alone();
    let platform = Platform::new();
    let chord = Chord::parse("Ctrl+Alt+Shift+F23").expect("a valid chord");
    let modifiers = MOD_CONTROL | MOD_ALT | MOD_SHIFT;

    // "Another program": this thread registers the chord directly with the system.
    // SAFETY: a null window ties the hotkey to this thread; plain values otherwise.
    let taken = unsafe { RegisterHotKey(ptr::null_mut(), OTHER_PROGRAM, modifiers, VK_F23.into()) };
    if taken == 0 {
        skip("this session has no hotkeys, or another program holds Ctrl+Alt+Shift+F23");
        return;
    }
    let release = || {
        // SAFETY: the id was registered by this thread just above.
        unsafe { UnregisterHotKey(ptr::null_mut(), OTHER_PROGRAM) };
    };

    // While the listener is stopped, and while it runs.
    let stopped = platform.register(HotkeyId(1), chord);
    let (sink, _events) = event_channel(16);
    let started = platform.start(Box::new(sink));
    let running = if started.is_ok() {
        platform.register(HotkeyId(1), chord)
    } else {
        Err(PlatformError::HotkeyUnavailable)
    };
    release();
    assert_eq!(stopped, Err(PlatformError::HotkeyUnavailable));
    if let Err(error) = started {
        skip(&format!(
            "the listener cannot start in this session: {error}"
        ));
        return;
    }
    assert_eq!(running, Err(PlatformError::HotkeyUnavailable));

    // Free again: the chord can be had, once.
    assert_eq!(platform.register(HotkeyId(1), chord), Ok(()));
    assert_eq!(platform.register(HotkeyId(1), chord), Ok(()), "unchanged");
    assert_eq!(
        platform.register(HotkeyId(2), chord),
        Err(PlatformError::HotkeyUnavailable),
        "another id of Iamb's holds it"
    );
    // An id moves to another chord and gives the old one back.
    let other = Chord::parse("Ctrl+Alt+Shift+F24").expect("a valid chord");
    match platform.register(HotkeyId(1), other) {
        Ok(()) => assert_eq!(platform.register(HotkeyId(2), chord), Ok(())),
        Err(PlatformError::HotkeyUnavailable) => {
            eprintln!("note: another program holds {other}; the move was not tested");
        }
        Err(error) => panic!("registering {other} failed: {error}"),
    }
    assert_eq!(platform.unregister(HotkeyId(1)), Ok(()));
    assert_eq!(platform.unregister(HotkeyId(2)), Ok(()));
    platform.stop().expect("the listener stops");
}
