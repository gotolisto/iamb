//! The probe window (feature `probe`) against the real system: it opens, takes the focus,
//! receives a short abbreviation-free string typed with ordinary synthetic keys (`SendInput`,
//! unmarked), stamps the last key (T0) and the first change after which the control holds the
//! string (T1). Runs on Windows only, with `cargo test -p iamb-platform-windows --features
//! probe`; a session without a desktop says `SKIPPED`, which `IAMB_REQUIRE_DESKTOP` turns into a
//! failure.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iamb_platform_windows::probe::{Event, ProbeWindow, Sink};

/// What the test types: no abbreviation of any library.
const TYPED: &str = "probe 42";

#[derive(Debug, Clone)]
enum Seen {
    Key(Instant, char),
    Changed(Instant, String),
}

fn skip(why: &str) {
    assert!(
        std::env::var_os("IAMB_REQUIRE_DESKTOP").is_none(),
        "IAMB_REQUIRE_DESKTOP is set, but {why}"
    );
    eprintln!("SKIPPED: {why}");
}

fn seen() -> (Arc<Mutex<Vec<Seen>>>, Sink) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&log);
    let sink: Sink = Arc::new(move |event: Event<'_>| {
        let item = match event {
            Event::Key { at, ch } => Seen::Key(at, ch),
            Event::Changed { at, text } => Seen::Changed(at, text.to_owned()),
        };
        into.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(item);
    });
    (log, sink)
}

/// T0: the key that completed `TYPED`; T1: the first change after it whose text is `TYPED`.
fn stamps(log: &[Seen]) -> Option<(Instant, Instant)> {
    let mut typed = String::new();
    let mut t0 = None;
    for item in log {
        match item {
            Seen::Key(at, ch) if t0.is_none() => {
                typed.push(*ch);
                if typed.ends_with(TYPED) {
                    t0 = Some(*at);
                }
            }
            Seen::Changed(at, text) => {
                if let Some(t0) = t0
                    && text == TYPED
                {
                    return Some((t0, *at));
                }
            }
            Seen::Key(..) => {}
        }
    }
    None
}

#[test]
fn the_probe_stamps_the_last_key_and_the_matching_change() {
    let (log, sink) = seen();
    let window = match ProbeWindow::open("Iamb probe test", sink) {
        Ok(w) => w,
        Err(e) => return skip(&e),
    };
    match window.reset() {
        Ok(true) => {}
        Ok(false) => return skip("the probe window could not take the foreground and the focus"),
        Err(e) => panic!("reset: {e}"),
    }
    log.lock().unwrap().clear();
    if let Err(e) = window.type_synthetic(TYPED) {
        return skip(&e);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let found = loop {
        let found = stamps(&log.lock().unwrap());
        if found.is_some() || Instant::now() > deadline {
            break found;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let keys = log
        .lock()
        .unwrap()
        .iter()
        .filter(|s| matches!(s, Seen::Key(..)))
        .count();
    let Some((t0, t1)) = found else {
        panic!("no T0/T1: {keys} key event(s) seen, the text never equalled the typed string");
    };
    assert_eq!(keys, TYPED.chars().count(), "one key event per character");
    assert!(t1 >= t0, "T1 comes after T0");
    assert!(t1 - t0 < Duration::from_secs(2), "within the timeout");
    // A reset empties the control and reports the change.
    log.lock().unwrap().clear();
    assert_eq!(window.reset(), Ok(true));
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|s| matches!(s, Seen::Changed(_, t) if t.is_empty()))
            || window.has_focus(),
        "the reset emptied the control"
    );
}
