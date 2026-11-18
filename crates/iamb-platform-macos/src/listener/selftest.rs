//! The rendezvous between a caller of `self_test` and the tap callback (architecture §3.2).
//!
//! `self_test` posts a no-op key event marked [`Marker::SelfTest`](iamb_platform::Marker) with
//! a nonce and waits here; the tap callback, on seeing that marker, completes the wait. The
//! nonce keeps a late event of an earlier test, or the self-test event of another Iamb
//! process in the same login session, from completing this one.
//!
//! The callback's share is one short critical section, so the callback stays bounded.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One self-test at a time waits here.
#[derive(Debug)]
pub(crate) struct SelfTests {
    slot: Mutex<Slot>,
    seen: Condvar,
    next_nonce: AtomicU32,
}

#[derive(Debug, Default)]
struct Slot {
    /// The nonce a caller waits for.
    waiting_for: Option<u32>,
    /// Whether the callback saw it.
    seen: bool,
}

impl SelfTests {
    /// A rendezvous whose nonces start at a value that differs between processes and runs.
    pub(crate) fn new() -> Self {
        let clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        Self::starting_at(std::process::id().rotate_left(16) ^ clock)
    }

    fn starting_at(first_nonce: u32) -> Self {
        Self {
            slot: Mutex::new(Slot::default()),
            seen: Condvar::new(),
            next_nonce: AtomicU32::new(first_nonce),
        }
    }

    fn slot(&self) -> MutexGuard<'_, Slot> {
        // The slot holds two plain values; a panic elsewhere cannot leave it half-written.
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a test and returns the nonce to put in the marker. An earlier test that was
    /// never completed is forgotten.
    pub(crate) fn begin(&self) -> u32 {
        let nonce = self.next_nonce.fetch_add(1, Ordering::Relaxed);
        let mut slot = self.slot();
        slot.waiting_for = Some(nonce);
        slot.seen = false;
        nonce
    }

    /// Called by the tap callback for a self-test marker. A nonce nobody waits for is ignored.
    pub(crate) fn complete(&self, nonce: u32) {
        let mut slot = self.slot();
        if slot.waiting_for == Some(nonce) {
            slot.seen = true;
            drop(slot);
            self.seen.notify_all();
        }
    }

    /// Waits until the callback saw `nonce` or `deadline` passes. Returns whether it was seen.
    pub(crate) fn wait_until(&self, nonce: u32, deadline: Instant) -> bool {
        let mut slot = self.slot();
        loop {
            if slot.waiting_for != Some(nonce) {
                return false;
            }
            if slot.seen {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left == Duration::ZERO {
                return false;
            }
            slot = self
                .seen
                .wait_timeout(slot, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Ends the test: a marker that arrives from now on is ignored.
    pub(crate) fn end(&self) {
        let mut slot = self.slot();
        slot.waiting_for = None;
        slot.seen = false;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn the_callback_completes_the_waiting_test() {
        let tests = Arc::new(SelfTests::starting_at(7));
        let nonce = tests.begin();
        assert_eq!(nonce, 7);
        let callback = {
            let tests = Arc::clone(&tests);
            thread::spawn(move || tests.complete(nonce))
        };
        assert!(tests.wait_until(nonce, soon()));
        callback.join().expect("the callback thread");
        tests.end();
    }

    #[test]
    fn a_marker_seen_before_the_wait_still_counts() {
        let tests = SelfTests::starting_at(1);
        let nonce = tests.begin();
        tests.complete(nonce);
        assert!(tests.wait_until(nonce, Instant::now()));
    }

    #[test]
    fn an_unseen_marker_times_out() {
        let tests = SelfTests::starting_at(1);
        let nonce = tests.begin();
        let started = Instant::now();
        assert!(!tests.wait_until(nonce, started + Duration::from_millis(30)));
        assert!(started.elapsed() >= Duration::from_millis(30));
        // A deadline in the past does not wait at all.
        assert!(!tests.wait_until(nonce, started));
    }

    #[test]
    fn a_foreign_or_stale_nonce_completes_nothing() {
        let tests = SelfTests::starting_at(10);
        // Nobody waits: ignored.
        tests.complete(10);
        let first = tests.begin();
        tests.end();
        let second = tests.begin();
        assert_ne!(first, second);
        // The late marker of the first test, and another process's marker.
        tests.complete(first);
        tests.complete(0xDEAD_BEEF);
        assert!(!tests.wait_until(second, Instant::now() + Duration::from_millis(10)));
        // A wait for a test that is no longer the current one gives up at once.
        assert!(!tests.wait_until(first, soon()));
        tests.complete(second);
        assert!(tests.wait_until(second, soon()));
    }

    #[test]
    fn nonces_wrap_and_differ_between_instances() {
        let tests = SelfTests::starting_at(u32::MAX);
        assert_eq!(tests.begin(), u32::MAX);
        assert_eq!(tests.begin(), 0);
        // Two listeners in one process start from the same process id but not from the same
        // clock reading; this only checks that construction works and nonces advance.
        let other = SelfTests::new();
        let a = other.begin();
        let b = other.begin();
        assert_eq!(b, a.wrapping_add(1));
    }
}
