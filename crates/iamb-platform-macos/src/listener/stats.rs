//! What the listener measures about itself: how long its callback takes and how many events
//! it handled. Numbers only; nothing here can hold a key or a character (architecture §8).
//!
//! The callback records with relaxed atomic operations, so measuring costs a few nanoseconds
//! and never blocks. The histogram answers "what is the 99th percentile" to the precision of
//! its bucket bounds, which is what the delivery target (under 1 ms at the 99th percentile)
//! needs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::target::TargetSource;

/// Exclusive upper bounds of the histogram buckets, in nanoseconds. A duration falls in the
/// first bucket whose bound it is below; the last bucket takes everything else.
const BOUNDS_NANOS: [u64; 13] = [
    1_000,
    2_000,
    5_000,
    10_000,
    20_000,
    50_000,
    100_000,
    200_000,
    500_000,
    1_000_000,
    2_000_000,
    10_000_000,
    100_000_000,
];

const BUCKETS: usize = BOUNDS_NANOS.len() + 1;

/// Counters shared between the listener thread and whoever asks about its health.
#[derive(Debug, Default)]
pub(crate) struct ListenerStats {
    callback: [AtomicU64; BUCKETS],
    slowest_nanos: AtomicU64,
    key_downs: AtomicU64,
    own_events: AtomicU64,
    panics: AtomicU64,
    targets_from_event: AtomicU64,
    targets_from_frontmost: AtomicU64,
    targets_unknown: AtomicU64,
    target_disagreements: AtomicU64,
}

/// A copy of the counters at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatsSnapshot {
    /// How many times the tap callback ran.
    pub(crate) callbacks: u64,
    /// An upper bound of the 99th percentile of the callback's duration: the bound of the
    /// histogram bucket that holds it (the slowest callback when that is smaller).
    pub(crate) callback_p99: Duration,
    /// The slowest callback.
    pub(crate) callback_slowest: Duration,
    /// Key presses that were not Iamb's own.
    pub(crate) key_downs: u64,
    /// Events that carried an Iamb marker.
    pub(crate) own_events: u64,
    /// Panics caught at the callback boundary. Always 0 unless there is a bug.
    pub(crate) panics: u64,
    /// Key presses stamped from the event's own `kCGEventTargetUnixProcessID`.
    pub(crate) targets_from_event: u64,
    /// Key presses stamped with the frontmost application as last noted (the field was 0).
    pub(crate) targets_from_frontmost: u64,
    /// Key presses delivered without a stamp: neither source knew.
    pub(crate) targets_unknown: u64,
    /// Key presses whose field named another process than the frontmost application.
    pub(crate) target_disagreements: u64,
}

impl ListenerStats {
    /// Records how long one callback took.
    pub(crate) fn record_callback(&self, took: Duration) {
        let nanos = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
        let bucket = BOUNDS_NANOS
            .iter()
            .position(|bound| nanos < *bound)
            .unwrap_or(BOUNDS_NANOS.len());
        if let Some(count) = self.callback.get(bucket) {
            count.fetch_add(1, Ordering::Relaxed);
        }
        self.slowest_nanos.fetch_max(nanos, Ordering::Relaxed);
    }

    /// Counts a key press that was not Iamb's own.
    pub(crate) fn count_key_down(&self) {
        self.key_downs.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts an event that carried an Iamb marker.
    pub(crate) fn count_own_event(&self) {
        self.own_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts which source stamped a key press, and whether the event's field disagreed
    /// with the frontmost application.
    pub(crate) fn count_target(&self, source: TargetSource, disagrees: bool) {
        let counter = match source {
            TargetSource::Event => &self.targets_from_event,
            TargetSource::Frontmost => &self.targets_from_frontmost,
            TargetSource::Unknown => &self.targets_unknown,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        if disagrees {
            self.target_disagreements.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Counts a panic caught at the callback boundary.
    pub(crate) fn count_panic(&self) {
        self.panics.fetch_add(1, Ordering::Relaxed);
    }

    /// The counters now.
    pub(crate) fn snapshot(&self) -> StatsSnapshot {
        let counts: [u64; BUCKETS] = std::array::from_fn(|i| {
            self.callback
                .get(i)
                .map_or(0, |c| c.load(Ordering::Relaxed))
        });
        let callbacks: u64 = counts.iter().sum();
        let slowest = self.slowest_nanos.load(Ordering::Relaxed);
        Self::summarise(&counts, callbacks, slowest, self)
    }

    fn summarise(
        counts: &[u64; BUCKETS],
        callbacks: u64,
        slowest: u64,
        stats: &Self,
    ) -> StatsSnapshot {
        // The smallest rank that has at least 99% of the samples at or below it.
        let rank = callbacks - callbacks / 100;
        let mut below = 0;
        let mut p99 = 0;
        if callbacks > 0 {
            p99 = slowest;
            for (count, bound) in counts.iter().zip(BOUNDS_NANOS) {
                below += count;
                if below >= rank {
                    p99 = bound.min(slowest);
                    break;
                }
            }
        }
        StatsSnapshot {
            callbacks,
            callback_p99: Duration::from_nanos(p99),
            callback_slowest: Duration::from_nanos(slowest),
            key_downs: stats.key_downs.load(Ordering::Relaxed),
            own_events: stats.own_events.load(Ordering::Relaxed),
            panics: stats.panics.load(Ordering::Relaxed),
            targets_from_event: stats.targets_from_event.load(Ordering::Relaxed),
            targets_from_frontmost: stats.targets_from_frontmost.load(Ordering::Relaxed),
            targets_unknown: stats.targets_unknown.load(Ordering::Relaxed),
            target_disagreements: stats.target_disagreements.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn an_empty_histogram_reports_zero() {
        let snapshot = ListenerStats::default().snapshot();
        assert_eq!(snapshot.callbacks, 0);
        assert_eq!(snapshot.callback_p99, Duration::ZERO);
        assert_eq!(snapshot.callback_slowest, Duration::ZERO);
    }

    #[test]
    fn the_99th_percentile_ignores_one_slow_callback_in_a_hundred() {
        let stats = ListenerStats::default();
        for _ in 0..99 {
            stats.record_callback(Duration::from_micros(30));
        }
        stats.record_callback(Duration::from_millis(40));
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.callbacks, 100);
        // 30 µs falls in the bucket below 50 µs.
        assert_eq!(snapshot.callback_p99, Duration::from_micros(50));
        assert!(snapshot.callback_p99 < MS);
        assert_eq!(snapshot.callback_slowest, Duration::from_millis(40));
    }

    #[test]
    fn two_slow_callbacks_in_a_hundred_show() {
        let stats = ListenerStats::default();
        for _ in 0..98 {
            stats.record_callback(Duration::from_micros(30));
        }
        stats.record_callback(Duration::from_millis(3));
        stats.record_callback(Duration::from_millis(40));
        let snapshot = stats.snapshot();
        // The 99th of 100 samples is the 3 ms one: its bucket's bound is 10 ms.
        assert_eq!(snapshot.callback_p99, Duration::from_millis(10));
        assert!(snapshot.callback_p99 >= MS);
    }

    #[test]
    fn the_bound_is_never_above_the_slowest_callback() {
        let stats = ListenerStats::default();
        stats.record_callback(Duration::from_nanos(1_200));
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.callback_p99, Duration::from_nanos(1_200));
        assert_eq!(snapshot.callback_slowest, Duration::from_nanos(1_200));
    }

    #[test]
    fn bucket_bounds_are_exclusive_and_the_last_bucket_takes_the_rest() {
        let stats = ListenerStats::default();
        // Exactly 1 ms is not "under 1 ms".
        for _ in 0..10 {
            stats.record_callback(MS);
        }
        assert_eq!(stats.snapshot().callback_p99, MS);
        let just_under = Duration::from_nanos(999_999);
        let stats = ListenerStats::default();
        for _ in 0..10 {
            stats.record_callback(just_under);
        }
        assert_eq!(stats.snapshot().callback_p99, just_under);
        assert!(stats.snapshot().callback_p99 < MS);

        let stats = ListenerStats::default();
        stats.record_callback(Duration::from_secs(3));
        stats.record_callback(Duration::MAX);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.callbacks, 2);
        assert_eq!(snapshot.callback_slowest, Duration::from_nanos(u64::MAX));
        assert_eq!(snapshot.callback_p99, Duration::from_nanos(u64::MAX));
    }

    #[test]
    fn counters_count() {
        let stats = ListenerStats::default();
        stats.count_key_down();
        stats.count_key_down();
        stats.count_own_event();
        stats.count_panic();
        stats.count_target(TargetSource::Event, true);
        stats.count_target(TargetSource::Event, false);
        stats.count_target(TargetSource::Frontmost, false);
        stats.count_target(TargetSource::Unknown, false);
        let snapshot = stats.snapshot();
        assert_eq!(
            (snapshot.key_downs, snapshot.own_events, snapshot.panics),
            (2, 1, 1)
        );
        assert_eq!(
            (
                snapshot.targets_from_event,
                snapshot.targets_from_frontmost,
                snapshot.targets_unknown,
                snapshot.target_disagreements
            ),
            (2, 1, 1, 1)
        );
    }

    #[test]
    fn the_bounds_grow() {
        assert!(BOUNDS_NANOS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(BOUNDS_NANOS.contains(&1_000_000));
    }
}
