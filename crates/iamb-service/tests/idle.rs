//! Idle behaviour: after start-up, no thread wakes more often than the heartbeat. Only the
//! engine thread's 500 ms heartbeat (and the 30 s self-test, which does not fall in the
//! window) may fire while nothing happens.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use common::{Harness, LIBRARY};
use iamb_ipc::Role;

#[test]
fn an_idle_worker_wakes_only_for_its_heartbeat() {
    let beats = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&beats);
    let h = Harness::start_with(LIBRARY, move |c| {
        c.heartbeat = Box::new(move || {
            counted.fetch_add(1, Ordering::Relaxed);
        });
    });
    // A connected but silent UI host must not add wake-ups either.
    let _ui = h.client(Role::Ui);
    std::thread::sleep(Duration::from_millis(300));

    let before = h.worker().wake_counts();
    let beats_before = beats.load(Ordering::Relaxed);
    let window = Duration::from_secs(3);
    std::thread::sleep(window);
    let after = h.worker().wake_counts();
    let heartbeats = beats.load(Ordering::Relaxed) - beats_before;

    // 3 s at one heartbeat per 500 ms: 6, give or take scheduling on a loaded machine.
    assert!(
        (4..=7).contains(&heartbeats),
        "{heartbeats} heartbeats in {window:?}"
    );
    assert_eq!(after.heartbeats - before.heartbeats, heartbeats);
    let engine = after.engine - before.engine;
    assert!(
        engine <= heartbeats + 1,
        "the engine thread woke {engine} times for {heartbeats} heartbeats"
    );
    assert_eq!(after.injector, before.injector, "injector woke while idle");
    assert_eq!(after.focus, before.focus, "focus thread woke while idle");
    assert_eq!(after.store, before.store, "store thread woke while idle");
    assert_eq!(after.watchdog, before.watchdog, "watchdog woke while idle");
    assert_eq!(after.accept, before.accept, "accept thread woke while idle");
    assert_eq!(
        after.clients, before.clients,
        "a client thread woke while idle"
    );
}
