//! The Windows listener end to end, through the public traits only: real hooks, real input
//! events posted with `SendInput`. Runs on Windows only (CI's Windows leg).
//!
//! The key events of these tests carry a tag, and a hook installed before the listener's
//! swallows them after the listener has seen them (`listener_support.rs`), so they never type
//! into the window in front. They are injected events without an Iamb marker, which the
//! listener must treat as real typing.
//!
//! A test that needs something the session does not have says `SKIPPED` and why, and fails
//! instead when `IAMB_REQUIRE_DESKTOP` is set.

#![cfg(windows)]

#[path = "listener_support.rs"]
mod support;

use std::time::Duration;

use iamb_platform::core_types::{BoundaryKind, ExpansionId};
use iamb_platform::{KeyListener, ListenerEvent, Marker, PlatformError, event_channel};
use iamb_platform_windows::Platform;

use support::{
    Key, Seen, VK_UNASSIGNED, alone, desktop, letters, passed, printable, seen, send, skip,
    swallowed,
};

const SECOND: Duration = Duration::from_secs(1);

/// The letter keys of the layout in front, or `None` (with the reason printed) when it types
/// fewer than two.
fn typable() -> Option<Vec<(u16, String)>> {
    let letters = letters();
    if letters.len() < 2 {
        skip("the layout of the window in front types no letters");
        return None;
    }
    Some(letters)
}

fn tap(vk: u16) -> [Key; 2] {
    [Key::press(vk), Key::release(vk)]
}

#[test]
fn start_and_stop_are_idempotent_with_clear_errors() {
    let _alone = alone();
    let Some(_watcher) = desktop() else {
        return;
    };
    let platform = Platform::new();
    // Stopped: stopping is not an error, testing is.
    assert_eq!(platform.stop(), Ok(()));
    assert_eq!(platform.self_test(SECOND), Err(PlatformError::NotRunning));

    let (sink, _events) = event_channel(16);
    assert_eq!(platform.start(Box::new(sink)), Ok(()));
    let (second, _unused) = event_channel(16);
    assert_eq!(
        platform.start(Box::new(second)),
        Err(PlatformError::AlreadyRunning)
    );
    assert_eq!(platform.stop(), Ok(()));
    assert_eq!(platform.stop(), Ok(()));
    assert_eq!(platform.self_test(SECOND), Err(PlatformError::NotRunning));

    // And it starts again.
    let (sink, _events) = event_channel(16);
    assert_eq!(platform.start(Box::new(sink)), Ok(()));
    assert_eq!(platform.stop(), Ok(()));
}

/// The delivery KPI: 0 dropped or doubled keys in a 10,000-key run. Every key arrives exactly
/// once, in order, with consecutive sequence numbers, and every event (press and release) is
/// passed on to the hooks after the listener's.
#[test]
fn ten_thousand_keys_arrive_exactly_once_and_in_order() {
    const KEYS: usize = 10_000;
    const BATCH: usize = 50;
    let _alone = alone();
    let Some(_watcher) = desktop() else {
        return;
    };
    let Some(letters) = typable() else {
        return;
    };
    let platform = Platform::new();
    let (sink, events) = event_channel(4 * KEYS);
    platform.start(Box::new(sink)).expect("the listener starts");
    let swallowed_before = swallowed();
    let passed_before = passed();

    let mut expected = Vec::with_capacity(KEYS);
    while expected.len() < KEYS {
        let mut batch = Vec::with_capacity(2 * BATCH);
        for i in expected.len()..(expected.len() + BATCH).min(KEYS) {
            let (vk, ch) = &letters[i % letters.len()];
            batch.extend(tap(*vk));
            expected.push(ch.clone());
        }
        assert_eq!(send(&batch), batch.len(), "the system took the batch");
    }
    platform.stop().expect("the listener stops");

    assert_eq!(
        swallowed() - swallowed_before,
        2 * KEYS,
        "events the listener did not pass on to the hooks after it"
    );

    let all: Vec<ListenerEvent> = events.try_iter().collect();
    assert!(
        !all.contains(&ListenerEvent::Overflow),
        "the listener lost events"
    );
    let sequence: Vec<u64> = all.iter().filter_map(ListenerEvent::seq).collect();
    assert!(
        sequence.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "a gap or a repeat in the sequence numbers"
    );
    let typed: Vec<String> = all
        .iter()
        .filter_map(|event| match event {
            ListenerEvent::Char { ch, .. } => Some(ch.expose().to_lowercase().to_string()),
            _ => None,
        })
        .collect();

    if passed() == passed_before {
        // Nobody else typed: the characters are exactly the keys.
        assert_eq!(typed.len(), KEYS, "dropped or doubled keys");
        assert!(typed == expected, "the keys arrived in another order");
        // Apart from the characters: mouse clicks of a person at the machine, at most.
        let others = all
            .iter()
            .filter(|event| {
                !matches!(
                    event,
                    ListenerEvent::Char { .. }
                        | ListenerEvent::Composing { .. }
                        | ListenerEvent::Boundary {
                            kind: BoundaryKind::Click,
                            ..
                        }
                )
            })
            .count();
        assert_eq!(others, 0, "events nobody caused");
    } else {
        eprintln!(
            "note: {} key events of somebody else went by during the run; checking that the \
             test's keys are all there, in order",
            passed() - passed_before
        );
        let mut matched = 0;
        for ch in &typed {
            if matched < KEYS && *ch == expected[matched] {
                matched += 1;
            }
        }
        assert_eq!(matched, KEYS, "keys that did not arrive in order");
    }
}

/// Iamb's own events: output is dropped, the two injection markers are reported in their
/// place in the stream, once each however the injector posts them.
#[test]
fn iambs_own_output_is_dropped_and_its_markers_are_reported() {
    let _alone = alone();
    let Some(_watcher) = desktop() else {
        return;
    };
    let Some(letters) = typable() else {
        return;
    };
    let (first, first_char) = letters[0].clone();
    let (second, second_char) = letters[1].clone();
    let platform = Platform::new();
    let (sink, events) = event_channel(64);
    platform.start(Box::new(sink)).expect("the listener starts");

    let one = ExpansionId::from_raw(0xE2E0_0001);
    let two = ExpansionId::from_raw(0xE2E0_0002);
    let no_op = Key::release(VK_UNASSIGNED);
    let mut keys = Vec::new();
    keys.extend(tap(first));
    // A marker posted as a single release.
    keys.push(no_op.marked(Marker::InjectionStart(one)));
    // Output: the same keys as above, a named key and a key nobody has, pressed and released.
    for vk in [first, second, 0x08, VK_UNASSIGNED] {
        keys.extend(tap(vk).map(|key| key.marked(Marker::Output)));
    }
    keys.push(no_op.marked(Marker::InjectionEnd(one)));
    keys.extend(tap(second));
    // Markers posted as a press and a release: still reported once each.
    for marker in [Marker::InjectionStart(two), Marker::InjectionEnd(two)] {
        keys.extend(tap(VK_UNASSIGNED).map(|key| key.marked(marker)));
    }
    keys.extend(tap(first));
    assert_eq!(send(&keys), keys.len());
    platform.stop().expect("the listener stops");

    assert_eq!(
        seen(&events),
        [
            Seen::Char(first_char.clone()),
            Seen::Started(one.raw()),
            Seen::Finished(one.raw()),
            Seen::Char(second_char),
            Seen::Started(two.raw()),
            Seen::Finished(two.raw()),
            Seen::Char(first_char),
        ]
    );
}

/// The self-test's marked event goes through the system and comes back through the hook;
/// nothing is delivered and no sequence number is taken.
#[test]
fn a_self_test_round_trips_and_delivers_nothing() {
    let _alone = alone();
    let Some(_watcher) = desktop() else {
        return;
    };
    let Some(letters) = typable() else {
        return;
    };
    let platform = Platform::new();
    let (sink, events) = event_channel(64);
    platform.start(Box::new(sink)).expect("the listener starts");

    for _ in 0..3 {
        assert_eq!(platform.self_test(SECOND), Ok(()));
    }
    assert_eq!(send(&tap(letters[0].0)), 2);
    platform.stop().expect("the listener stops");
    assert_eq!(platform.self_test(SECOND), Err(PlatformError::NotRunning));

    // Whatever else went by (a person's click), every sequence number that was taken belongs
    // to a delivered event: the self-tests took none.
    let all: Vec<ListenerEvent> = events.try_iter().collect();
    for (number, event) in (1_u64..).zip(&all) {
        assert_eq!(event.seq(), Some(number));
    }
    assert_eq!(
        all.iter().filter_map(printable).collect::<Vec<_>>(),
        [Seen::Char(letters[0].1.clone())],
        "a self-test delivers nothing"
    );
}

/// The sequence numbers belong to the listener, not to one run of its thread.
#[test]
fn a_restart_keeps_counting() {
    let _alone = alone();
    let Some(_watcher) = desktop() else {
        return;
    };
    let Some(letters) = typable() else {
        return;
    };
    let platform = Platform::new();
    let mut last = 0;
    for _ in 0..3 {
        let (sink, events) = event_channel(64);
        platform.start(Box::new(sink)).expect("the listener starts");
        assert_eq!(send(&tap(letters[0].0)), 2);
        assert_eq!(send(&tap(letters[1].0)), 2);
        platform.stop().expect("the listener stops");
        let all: Vec<ListenerEvent> = events.try_iter().collect();
        assert_eq!(
            all.iter().filter_map(printable).collect::<Vec<_>>(),
            [
                Seen::Char(letters[0].1.clone()),
                Seen::Char(letters[1].1.clone())
            ]
        );
        for seq in all.iter().filter_map(ListenerEvent::seq) {
            assert!(seq > last, "numbers only grow, across restarts too");
            last = seq;
        }
    }
    assert!(last >= 6);
}
