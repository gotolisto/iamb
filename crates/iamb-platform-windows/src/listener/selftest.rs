//! The hand-shake of the self-test (architecture §3.2, ADR-0011): one thread posts a marked
//! no-op key event, the hook sees it on the listener thread.
//!
//! The hook must not block, so the two sides share one atomic word and no lock: the caller
//! arms it with the nonce of its event, the hook swaps "armed with this nonce" for "seen",
//! and the caller looks for "seen". A low-level hook is called before the call that posted
//! the event returns, so on a working hook the word already says "seen" when the caller first
//! looks; the wait only matters when the listener thread is busy.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const IDLE: u64 = 0;
const ARMED: u64 = 1 << 32;
const SEEN: u64 = 2 << 32;

/// How often the caller looks while it waits.
const POLL: Duration = Duration::from_millis(1);

/// The word the self-test's two sides share. Nobody waits at a new one.
#[derive(Debug, Default)]
pub(crate) struct SelfTestGate(AtomicU64);

impl SelfTestGate {
    /// Starts waiting for the event with `nonce`. An earlier wait is abandoned.
    pub(crate) fn arm(&self, nonce: u32) {
        self.0.store(ARMED | u64::from(nonce), Ordering::SeqCst);
    }

    /// Stops waiting.
    pub(crate) fn disarm(&self) {
        self.0.store(IDLE, Ordering::SeqCst);
    }

    /// The hook saw a self-test event with `nonce`. Never blocks. An event nobody waits for
    /// (another process's, or one that arrives after its test gave up) changes nothing.
    pub(crate) fn seen(&self, nonce: u32) {
        let nonce = u64::from(nonce);
        let _ = self.0.compare_exchange(
            ARMED | nonce,
            SEEN | nonce,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    /// True once the hook saw the event with `nonce`.
    pub(crate) fn is_seen(&self, nonce: u32) -> bool {
        self.0.load(Ordering::SeqCst) == SEEN | u64::from(nonce)
    }

    /// Waits until the hook saw the event with `nonce`, for at most `timeout`.
    pub(crate) fn wait(&self, nonce: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.is_seen(nonce) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            std::thread::sleep(left.min(POLL));
        }
    }
}

/// A nonce for the next self-test: different for every test of this process, and unlikely to
/// equal one of another process that started at another moment.
pub(crate) fn next_nonce() -> u32 {
    static START: OnceLock<u32> = OnceLock::new();
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let start = *START.get_or_init(|| {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        std::process::id().wrapping_mul(0x9E37_79B1) ^ since_epoch
    });
    start.wrapping_add(COUNT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn only_the_armed_nonce_completes_the_wait() {
        let gate = SelfTestGate::default();
        gate.seen(5);
        assert!(!gate.is_seen(5), "nobody was waiting");
        gate.arm(5);
        gate.seen(6);
        assert!(!gate.is_seen(5) && !gate.is_seen(6));
        gate.seen(5);
        assert!(gate.is_seen(5));
        assert!(gate.wait(5, Duration::ZERO));
        // Seen twice changes nothing; a new test starts from nothing.
        gate.seen(5);
        assert!(gate.is_seen(5));
        gate.arm(7);
        assert!(!gate.is_seen(5) && !gate.is_seen(7));
        gate.disarm();
        gate.seen(7);
        assert!(!gate.is_seen(7));
        // The nonces 0 and the largest are ordinary.
        for nonce in [0, u32::MAX] {
            gate.arm(nonce);
            assert!(!gate.is_seen(nonce));
            gate.seen(nonce);
            assert!(gate.is_seen(nonce));
        }
    }

    #[test]
    fn the_wait_ends_at_the_timeout() {
        let gate = SelfTestGate::default();
        gate.arm(1);
        let started = Instant::now();
        assert!(!gate.wait(1, Duration::from_millis(20)));
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn the_wait_ends_when_another_thread_sees_the_event() {
        let gate = Arc::new(SelfTestGate::default());
        gate.arm(9);
        let hook = Arc::clone(&gate);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            hook.seen(9);
        });
        assert!(gate.wait(9, Duration::from_secs(10)));
        thread.join().expect("the thread ends");
    }

    #[test]
    fn nonces_differ() {
        let a = next_nonce();
        let b = next_nonce();
        let c = next_nonce();
        assert!(a != b && b != c && a != c);
    }
}
