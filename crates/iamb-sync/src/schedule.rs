//! When the agent acts (§6.6, §6.7, §6.8): retry with backoff, the polling cadence, reconnecting
//! the live channel and the debounce after a local change.
//!
//! Times are milliseconds on a monotonic clock the caller supplies, so the shipped loop and the
//! simulation (which runs on a simulated clock) use the same rules. Jitter comes from a [`Jitter`]
//! the caller supplies, seeded in the simulation.

/// Up to 20% added at random to a delay (§6.6, §6.8).
pub trait Jitter {
    /// A number in `0..=max`.
    fn upto(&mut self, max: u64) -> u64;
}

/// A small deterministic generator (`SplitMix64`), for jitter and for the simulation.
#[derive(Debug, Clone)]
pub struct SplitMix(pub u64);

impl SplitMix {
    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (`n > 0`).
    pub const fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// True with probability `p` (0 to 1).
    pub fn chance(&mut self, p: f64) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let x = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        x < p
    }
}

impl Jitter for SplitMix {
    fn upto(&mut self, max: u64) -> u64 {
        self.below(max + 1)
    }
}

/// The debounce after `OutboxChanged`: a burst of saves becomes one request (§6.7).
pub const DEBOUNCE_MS: u64 = 250;
/// Poll interval while the channel is down and the user was active in the last 10 minutes.
pub const POLL_ACTIVE_MS: u64 = 2_000;
/// Poll interval while the channel is down and the user was not active.
pub const POLL_IDLE_MS: u64 = 60_000;
/// How long a local change or an open library window counts as activity.
pub const ACTIVE_WINDOW_MS: u64 = 10 * 60_000;
/// While the channel is up the device still pulls this often.
pub const POLL_WITH_CHANNEL_MS: u64 = 5 * 60_000;
/// The first retry delay; it doubles up to [`RETRY_MAX_MS`].
pub const RETRY_FIRST_MS: u64 = 1_000;
/// The longest retry delay.
pub const RETRY_MAX_MS: u64 = 5 * 60_000;
/// The reconnect delays of the live channel, then every minute (§6.6).
pub const RECONNECT_MS: [u64; 5] = [1_000, 2_000, 5_000, 10_000, 30_000];
/// After the service refused the join for lack of connections.
pub const RECONNECT_FULL_MS: u64 = 5 * 60_000;
/// The heartbeat of the live channel.
pub const HEARTBEAT_MS: u64 = 25_000;
/// No heartbeat answer within this long means the channel dropped.
pub const HEARTBEAT_TIMEOUT_MS: u64 = 10_000;
/// The time-out of one call, above every function's own (§6.1).
pub const CALL_TIMEOUT_MS: u64 = 40_000;
/// While stopped on `server_behind` or `server_too_old`, `hello` once an hour (D12).
pub const STOPPED_HELLO_MS: u64 = 60 * 60_000;
/// The access check and the retry of blocked changes run once a day.
pub const DAILY_MS: u64 = 24 * 60 * 60_000;

/// Adds up to 20% of `ms` at random.
pub fn with_jitter(ms: u64, jitter: &mut dyn Jitter) -> u64 {
    ms + jitter.upto(ms / 5)
}

/// Retry with backoff (§6.8): 1, 2, 4, 8 … seconds up to 5 minutes, each with up to 20% added;
/// back to 1 second after a call that succeeds.
#[derive(Debug, Clone, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// A fresh backoff.
    #[must_use]
    pub const fn new() -> Self {
        Self { failures: 0 }
    }

    /// The delay before the next try after one more failure.
    pub fn fail(&mut self, jitter: &mut dyn Jitter) -> u64 {
        let exp = self.failures.min(20);
        self.failures = self.failures.saturating_add(1);
        let base = RETRY_FIRST_MS.saturating_mul(1u64 << exp).min(RETRY_MAX_MS);
        with_jitter(base, jitter)
    }

    /// A call succeeded.
    pub const fn succeed(&mut self) {
        self.failures = 0;
    }

    /// Failures in a row.
    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }
}

/// The state of the live channel as the scheduler sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Joined; pokes arrive.
    Up,
    /// Down; trying to reconnect (`attempt` counts the tries).
    Down {
        /// Reconnect attempts so far.
        attempt: u32,
    },
    /// The join was refused because the project's connections are used up.
    Full,
}

/// Decides when the agent syncs next (§6.6, §6.7).
#[derive(Debug, Clone)]
pub struct Schedule {
    /// The live channel.
    pub channel: Channel,
    /// When the user was last active (a local change, the library window open).
    pub last_active: Option<u64>,
    /// When the last round of sync finished.
    pub last_sync: Option<u64>,
    /// A round is due at this time (a poke, a local change after the debounce, a retry).
    pub due: Option<u64>,
    /// Retry with backoff.
    pub backoff: Backoff,
    /// When the channel should next try to reconnect.
    pub reconnect_at: Option<u64>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    /// A schedule with the channel down and a round due at once.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            channel: Channel::Down { attempt: 0 },
            last_active: None,
            last_sync: None,
            due: Some(0),
            backoff: Backoff::new(),
            reconnect_at: Some(0),
        }
    }

    fn due_at(&mut self, at: u64) {
        self.due = Some(self.due.map_or(at, |d| d.min(at)));
    }

    /// The service said the outbox changed: sync after the debounce.
    pub fn outbox_changed(&mut self, now: u64) {
        self.last_active = Some(now);
        self.due_at(now + DEBOUNCE_MS);
    }

    /// A poke whose head is above the device's position: pull at once.
    pub fn poked(&mut self, now: u64) {
        self.due_at(now);
    }

    /// The channel (re)connected: pull once, pokes may have been missed.
    pub fn channel_up(&mut self, now: u64) {
        self.channel = Channel::Up;
        self.reconnect_at = None;
        self.due_at(now);
    }

    /// The channel dropped (or the join was refused).
    pub fn channel_down(&mut self, now: u64, full: bool, jitter: &mut dyn Jitter) {
        if full {
            self.channel = Channel::Full;
            self.reconnect_at = Some(now + with_jitter(RECONNECT_FULL_MS, jitter));
            return;
        }
        let attempt = match self.channel {
            Channel::Down { attempt } => attempt + 1,
            _ => 0,
        };
        self.channel = Channel::Down { attempt };
        let i = usize::try_from(attempt).unwrap_or(usize::MAX);
        let base = RECONNECT_MS.get(i).copied().unwrap_or(60_000);
        self.reconnect_at = Some(now + with_jitter(base, jitter));
    }

    /// A round finished well.
    pub fn synced(&mut self, now: u64) {
        self.last_sync = Some(now);
        self.backoff.succeed();
        self.due = None;
    }

    /// A round failed without an answer: try again after the backoff.
    pub fn failed(&mut self, now: u64, jitter: &mut dyn Jitter) {
        let delay = self.backoff.fail(jitter);
        self.due = Some(now + delay);
    }

    /// The server asked to wait (`rate_limited`).
    pub fn wait(&mut self, now: u64, ms: u64) {
        self.due = Some(now + ms);
    }

    /// The polling interval in force now.
    #[must_use]
    pub fn poll_interval(&self, now: u64) -> u64 {
        match self.channel {
            Channel::Up => POLL_WITH_CHANNEL_MS,
            Channel::Down { .. } | Channel::Full => {
                if self
                    .last_active
                    .is_some_and(|a| now.saturating_sub(a) < ACTIVE_WINDOW_MS)
                {
                    POLL_ACTIVE_MS
                } else {
                    POLL_IDLE_MS
                }
            }
        }
    }

    /// When the next round is due.
    #[must_use]
    pub fn next_round(&self, now: u64) -> u64 {
        let poll = self
            .last_sync
            .map_or(now, |l| l.saturating_add(self.poll_interval(now)));
        match self.due {
            Some(d) if self.backoff.failures() > 0 => d,
            Some(d) => d.min(poll),
            None => poll,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoJitter;
    impl Jitter for NoJitter {
        fn upto(&mut self, _: u64) -> u64 {
            0
        }
    }

    #[test]
    fn backoff_doubles_up_to_five_minutes_and_resets() {
        let mut b = Backoff::new();
        let mut j = NoJitter;
        let delays: Vec<u64> = (0..12).map(|_| b.fail(&mut j)).collect();
        assert_eq!(&delays[..5], &[1_000, 2_000, 4_000, 8_000, 16_000]);
        assert_eq!(delays[11], RETRY_MAX_MS);
        b.succeed();
        assert_eq!(b.fail(&mut j), 1_000);
    }

    #[test]
    fn jitter_adds_at_most_a_fifth() {
        let mut r = SplitMix(7);
        for _ in 0..1000 {
            let d = with_jitter(10_000, &mut r);
            assert!((10_000..=12_000).contains(&d));
        }
    }

    #[test]
    fn polling_follows_the_channel_and_activity() {
        let mut s = Schedule::new();
        s.synced(0);
        assert_eq!(s.poll_interval(0), POLL_IDLE_MS);
        s.outbox_changed(10);
        assert_eq!(s.poll_interval(20), POLL_ACTIVE_MS);
        assert_eq!(s.next_round(20), 10 + DEBOUNCE_MS);
        s.channel_up(30);
        s.synced(40);
        assert_eq!(s.poll_interval(50), POLL_WITH_CHANNEL_MS);
        assert_eq!(s.next_round(50), 40 + POLL_WITH_CHANNEL_MS);
    }

    #[test]
    fn reconnects_after_one_two_five_ten_thirty_then_every_minute() {
        let mut s = Schedule::new();
        let mut j = NoJitter;
        s.channel = Channel::Up;
        let mut at = Vec::new();
        for _ in 0..7 {
            s.channel_down(0, false, &mut j);
            at.push(s.reconnect_at.unwrap_or(0));
        }
        assert_eq!(at, vec![1_000, 2_000, 5_000, 10_000, 30_000, 60_000, 60_000]);
    }
}
