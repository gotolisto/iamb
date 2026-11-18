//! Latency recorder with exact percentiles.
//!
//! Criterion reports mean and median with confidence intervals; the KPIs are stated as
//! percentiles (p95, p99). The recorder keeps every sample in nanoseconds and computes
//! nearest-rank percentiles over the sorted samples, using integer arithmetic only.

use std::time::{Duration, Instant};

/// Collects latency samples in nanoseconds.
#[derive(Debug, Clone, Default)]
pub struct LatencyRecorder {
    samples: Vec<u64>,
}

/// Summary statistics of a set of samples, all in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencySummary {
    /// Number of samples.
    pub count: usize,
    /// Smallest sample.
    pub min: u64,
    /// Median (nearest rank).
    pub p50: u64,
    /// 95th percentile (nearest rank).
    pub p95: u64,
    /// 99th percentile (nearest rank).
    pub p99: u64,
    /// Largest sample.
    pub max: u64,
    /// Arithmetic mean, rounded down.
    pub mean: u64,
}

impl LatencyRecorder {
    /// Creates an empty recorder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    /// Creates an empty recorder with room for `n` samples, so recording does not allocate.
    #[must_use]
    pub fn with_capacity(n: usize) -> Self {
        Self {
            samples: Vec::with_capacity(n),
        }
    }

    /// Records one sample in nanoseconds.
    pub fn record_ns(&mut self, ns: u64) {
        self.samples.push(ns);
    }

    /// Records one sample; durations above `u64::MAX` nanoseconds (584 years) saturate.
    pub fn record(&mut self, d: Duration) {
        self.record_ns(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    }

    /// Runs `f`, records how long it took, and returns its result.
    pub fn time<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(start.elapsed());
        out
    }

    /// Number of samples recorded.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.samples.len()
    }

    /// True when nothing has been recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Returns the samples sorted ascending (the form written to `.samples` files).
    #[must_use]
    pub fn sorted_samples(&self) -> Vec<u64> {
        let mut v = self.samples.clone();
        v.sort_unstable();
        v
    }

    /// Computes the summary, or `None` when no samples were recorded.
    #[must_use]
    pub fn summary(&self) -> Option<LatencySummary> {
        summarize_sorted(&self.sorted_samples())
    }
}

/// Nearest-rank quantile of ascending `sorted` samples, with the quantile given in
/// per-mille (500 = median, 950 = p95, 990 = p99, 999 = p99.9, 1000 = max).
///
/// Rank is `ceil(permille * n / 1000)`, clamped to `1..=n`. Returns `None` for no samples.
#[must_use]
pub fn quantile_permille(sorted: &[u64], permille: u32) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    let p = usize::try_from(permille.min(1000)).unwrap_or(1000);
    let rank = (p * n).div_ceil(1000).clamp(1, n);
    Some(sorted[rank - 1])
}

/// Summarises ascending `sorted` samples, or `None` when empty.
#[must_use]
pub fn summarize_sorted(sorted: &[u64]) -> Option<LatencySummary> {
    let (&min, &max) = (sorted.first()?, sorted.last()?);
    let sum: u128 = sorted.iter().map(|&x| u128::from(x)).sum();
    let count = sorted.len();
    let mean = u64::try_from(sum / u128::try_from(count).unwrap_or(1)).unwrap_or(u64::MAX);
    Some(LatencySummary {
        count,
        min,
        p50: quantile_permille(sorted, 500)?,
        p95: quantile_permille(sorted, 950)?,
        p99: quantile_permille(sorted, 990)?,
        max,
        mean,
    })
}

#[cfg(test)]
mod tests {
    use super::{LatencyRecorder, quantile_permille, summarize_sorted};
    use std::time::Duration;

    #[test]
    fn empty_recorder_has_no_summary() {
        assert!(LatencyRecorder::new().summary().is_none());
        assert_eq!(quantile_permille(&[], 500), None);
    }

    #[test]
    fn one_to_hundred_gives_textbook_percentiles() {
        let mut r = LatencyRecorder::with_capacity(100);
        // Insert in reverse to prove the recorder sorts.
        for ns in (1..=100).rev() {
            r.record_ns(ns);
        }
        let s = r.summary().expect("non-empty");
        assert_eq!(
            (s.count, s.min, s.p50, s.p95, s.p99, s.max),
            (100, 1, 50, 95, 99, 100)
        );
        assert_eq!(s.mean, 50); // 5050 / 100 = 50.5, rounded down.
    }

    #[test]
    fn single_sample_is_every_percentile() {
        let s = summarize_sorted(&[7]).expect("non-empty");
        assert_eq!(
            (s.min, s.p50, s.p95, s.p99, s.max, s.mean),
            (7, 7, 7, 7, 7, 7)
        );
    }

    #[test]
    fn nearest_rank_rounds_up() {
        // n = 10: p95 rank = ceil(9.5) = 10, p50 rank = 5, p99 rank = ceil(9.9) = 10.
        let v: Vec<u64> = (10..=100).step_by(10).collect();
        assert_eq!(quantile_permille(&v, 500), Some(50));
        assert_eq!(quantile_permille(&v, 950), Some(100));
        assert_eq!(quantile_permille(&v, 990), Some(100));
        assert_eq!(quantile_permille(&v, 0), Some(10));
        assert_eq!(quantile_permille(&v, 1000), Some(100));
        assert_eq!(quantile_permille(&v, 5000), Some(100));
    }

    #[test]
    fn tail_outliers_show_in_p99_not_p50() {
        // 990 fast samples and 10 slow ones: p99 is still fast, p99.9 and max are slow.
        let mut v = vec![1_000_u64; 990];
        v.extend([1_000_000_u64; 10]);
        assert_eq!(quantile_permille(&v, 500), Some(1_000));
        assert_eq!(quantile_permille(&v, 990), Some(1_000));
        assert_eq!(quantile_permille(&v, 999), Some(1_000_000));
    }

    #[test]
    fn durations_are_recorded_in_nanoseconds() {
        let mut r = LatencyRecorder::new();
        r.record(Duration::from_micros(3));
        r.record(Duration::MAX);
        assert_eq!(r.sorted_samples(), vec![3_000, u64::MAX]);
        let out = r.time(|| 5);
        assert_eq!((out, r.len()), (5, 3));
    }
}
