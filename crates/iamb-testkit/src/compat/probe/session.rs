//! The timing rules of the probe (`docs/test-strategy.md` §6), as a state machine with no clock
//! and no window of its own: every input carries its stamps, so the rules are tested here on
//! every system.
//!
//! A trial is armed with the keys the driver is about to type (`typed`, the abbreviation and
//! its delimiter if any) and the text the control must then hold (`expected`). Then:
//!
//! - **T0** is the stamp of the key event after which the characters received since arming
//!   end with `typed`: the last key of the abbreviation. Keys after T0 (the expander's own
//!   deletions and text) are not looked at.
//! - **T1** is the stamp of the first text-change event after T0 after which the text equals
//!   `expected` ([`matches`]: exactly, with `\r\n` and `\r` read as `\n` on both sides, since a
//!   Rich Edit control keeps line breaks as `\r`).
//! - Latency is `T1 − T0`. No match within [`TIMEOUT`] after T0 is a failed trial and counts as
//!   [`TIMEOUT`]; so is a trial whose last key never arrived within its key wait.
//!
//! Two clocks: a stamp comes from the clock of the surface that saw the event (the probe's
//! monotonic clock for the native window, `performance.now()` for the browser page), and
//! `now` is the probe's own clock when the event was handed over. Matching uses stamps only.
//! The deadlines use `now`: a trial times out once `now` is past the moment T0 arrived plus
//! [`TIMEOUT`] plus the surface's delivery slack (zero for the native window, whose events
//! arrive as they happen). A change that arrives later but is stamped within [`TIMEOUT`] still
//! counts only if it came before the deadline was noticed; the page's slack covers its delay.
//!
//! The session keeps the text of the current trial only (to compare), never writes it, and
//! its `Debug` leaves it out.

use std::collections::VecDeque;
use std::time::Duration;

use iamb_bench::{LatencyRecorder, LatencySummary};
use serde::{Deserialize, Serialize};

/// No match this long after T0 is a failed expansion, counted as this latency.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// How long a trial waits for its last key by default.
pub const DEFAULT_KEY_WAIT: Duration = Duration::from_secs(10);

/// [`TIMEOUT`] in nanoseconds.
pub const TIMEOUT_NS: u64 = 2_000_000_000;

/// `text` with `\r\n` and lone `\r` written as `\n`.
pub fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The matching rule: `text` equals `expected` exactly, line breaks aside.
pub fn matches(text: &str, expected: &str) -> bool {
    normalize(text) == normalize(expected)
}

/// Why a trial failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    /// The text did not equal the expected result within [`TIMEOUT`] after T0.
    Timeout,
    /// The last key of the abbreviation never reached the control.
    NoKey,
}

impl Failure {
    /// The name in results and messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::NoKey => "no-key",
        }
    }
}

/// One finished trial. Holds no text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// The driver's id of the trial.
    pub cell: String,
    /// The control held the expected text within [`TIMEOUT`] after T0.
    pub pass: bool,
    /// `T1 − T0`, or [`TIMEOUT`] for a failed trial, ns.
    pub latency_ns: u64,
    /// T0 on the surface's clock, ns.
    pub t0_ns: Option<u64>,
    /// T1 on the surface's clock, ns.
    pub t1_ns: Option<u64>,
    /// Why it failed.
    pub failure: Option<Failure>,
    /// After the settle time the driver asked for, the latest text still equalled the
    /// expected result (`None`: not checked).
    #[serde(default)]
    pub stable: Option<bool>,
}

/// The summary in the units of `LatencyRecorder`, serialisable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Samples.
    pub count: usize,
    /// Smallest, ns.
    pub min: u64,
    /// Median (nearest rank), ns.
    pub p50: u64,
    /// 95th percentile (nearest rank), ns.
    pub p95: u64,
    /// 99th percentile (nearest rank), ns.
    pub p99: u64,
    /// Largest, ns.
    pub max: u64,
    /// Mean, ns.
    pub mean: u64,
}

impl From<LatencySummary> for Summary {
    fn from(s: LatencySummary) -> Self {
        Self {
            count: s.count,
            min: s.min,
            p50: s.p50,
            p95: s.p95,
            p99: s.p99,
            max: s.max,
            mean: s.mean,
        }
    }
}

impl From<Summary> for LatencySummary {
    fn from(s: Summary) -> Self {
        Self {
            count: s.count,
            min: s.min,
            p50: s.p50,
            p95: s.p95,
            p99: s.p99,
            max: s.max,
            mean: s.mean,
        }
    }
}

/// Every trial so far: the samples, the recorder's sorted samples (the `LatencyRecorder`
/// format, one latency per trial, failures as [`TIMEOUT`]) and their summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The trials in order.
    pub samples: Vec<Sample>,
    /// The latencies, ascending, ns.
    pub sorted_ns: Vec<u64>,
    /// Their summary; `None` before the first trial.
    pub summary: Option<Summary>,
}

/// A trial to arm: what will be typed and what the control must then hold.
#[derive(Clone, PartialEq, Eq)]
pub struct Trial {
    /// The driver's id.
    pub cell: String,
    /// The keys the driver types (the abbreviation, with its delimiter if it has one).
    pub typed: String,
    /// The control's whole text once the expansion is complete.
    pub expected: String,
}

impl std::fmt::Debug for Trial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trial")
            .field("cell", &self.cell)
            .field("typed_chars", &self.typed.chars().count())
            .field("expected_chars", &self.expected.chars().count())
            .finish()
    }
}

struct Armed {
    cell: String,
    typed: Vec<char>,
    recent: VecDeque<char>,
    since: u64,
    key_wait_ns: u64,
    /// T0: the stamp, and `now` when it arrived.
    t0: Option<(u64, u64)>,
}

/// The probe's state: at most one armed trial, the finished samples and their recorder.
pub struct Session {
    slack_ns: u64,
    armed: Option<Armed>,
    /// The expected text of the current trial, normalised (kept after it finished, for the
    /// settle check, until the next trial or reset).
    expected: Option<String>,
    /// Whether the latest change since arming left the expected text.
    latest_matches: Option<bool>,
    finished: Option<Sample>,
    samples: Vec<Sample>,
    recorder: LatencyRecorder,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("armed", &self.armed.as_ref().map(|a| &a.cell))
            .field("samples", &self.samples.len())
            .finish_non_exhaustive()
    }
}

fn ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

impl Session {
    /// A session for a surface whose events reach the probe within `slack` of happening.
    pub fn new(slack: Duration) -> Self {
        Self {
            slack_ns: ns(slack),
            armed: None,
            expected: None,
            latest_matches: None,
            finished: None,
            samples: Vec::new(),
            recorder: LatencyRecorder::with_capacity(1024),
        }
    }

    /// True while a trial is armed and not finished.
    pub fn busy(&self) -> bool {
        self.armed.is_some()
    }

    /// Arms `trial` at `now`; its last key must arrive within `key_wait`.
    pub fn arm(&mut self, trial: Trial, now: u64, key_wait: Duration) -> Result<(), String> {
        if self.armed.is_some() {
            return Err("a trial is still running: wait for it or reset".to_owned());
        }
        if trial.typed.is_empty() {
            return Err("the typed keys of a trial cannot be empty".to_owned());
        }
        let typed: Vec<char> = trial.typed.chars().collect();
        self.expected = Some(normalize(&trial.expected));
        self.latest_matches = None;
        self.finished = None;
        self.armed = Some(Armed {
            cell: trial.cell,
            recent: VecDeque::with_capacity(typed.len()),
            typed,
            since: now,
            key_wait_ns: ns(key_wait),
            t0: None,
        });
        Ok(())
    }

    /// Forgets the armed trial without a sample, and the current expected text.
    pub fn cancel(&mut self) {
        self.armed = None;
        self.expected = None;
        self.latest_matches = None;
        self.finished = None;
    }

    /// The control received a key producing `ch`, stamped `stamp`, handed over at `now`.
    pub fn key(&mut self, stamp: u64, now: u64, ch: char) {
        let Some(armed) = self.armed.as_mut() else {
            return;
        };
        if armed.t0.is_some() {
            return;
        }
        if armed.recent.len() == armed.typed.len() {
            armed.recent.pop_front();
        }
        armed.recent.push_back(ch);
        if armed.recent.iter().eq(armed.typed.iter()) {
            armed.t0 = Some((stamp, now));
        }
    }

    /// The control's text changed to `text`, stamped `stamp`, handed over at `now`.
    pub fn changed(&mut self, stamp: u64, now: u64, text: &str) {
        let Some(expected) = self.expected.as_deref() else {
            return;
        };
        let equal = normalize(text) == expected;
        self.latest_matches = Some(equal);
        self.expire(now);
        let Some(armed) = self.armed.as_ref() else {
            return;
        };
        let Some((t0, _)) = armed.t0 else {
            return;
        };
        if stamp < t0 {
            return;
        }
        if stamp - t0 > TIMEOUT_NS {
            self.finish(Some(t0), None, Some(Failure::Timeout));
        } else if equal {
            self.finish(Some(t0), Some(stamp), None);
        }
    }

    /// Ends the armed trial as failed when its deadline has passed at `now`.
    pub fn expire(&mut self, now: u64) {
        let Some(armed) = self.armed.as_ref() else {
            return;
        };
        match armed.t0 {
            Some((t0, seen)) => {
                if now
                    > seen
                        .saturating_add(TIMEOUT_NS)
                        .saturating_add(self.slack_ns)
                {
                    self.finish(Some(t0), None, Some(Failure::Timeout));
                }
            }
            None => {
                if now > armed.since.saturating_add(armed.key_wait_ns) {
                    self.finish(None, None, Some(Failure::NoKey));
                }
            }
        }
    }

    fn finish(&mut self, t0: Option<u64>, t1: Option<u64>, failure: Option<Failure>) {
        let Some(armed) = self.armed.take() else {
            return;
        };
        let latency_ns = match (t0, t1) {
            (Some(t0), Some(t1)) => t1 - t0,
            _ => TIMEOUT_NS,
        };
        let sample = Sample {
            cell: armed.cell,
            pass: failure.is_none(),
            latency_ns,
            t0_ns: t0,
            t1_ns: t1,
            failure,
            stable: None,
        };
        self.recorder.record_ns(latency_ns);
        self.samples.push(sample.clone());
        self.finished = Some(sample);
    }

    /// The trial that finished since the last call, if any.
    pub fn take_finished(&mut self) -> Option<Sample> {
        self.finished.take()
    }

    /// Whether the latest change of the current trial left the expected text (`None`: no
    /// change was seen since arming).
    pub const fn latest_matches(&self) -> Option<bool> {
        self.latest_matches
    }

    /// Records the settle check on the latest sample, and returns it.
    pub fn mark_stable(&mut self, stable: bool) -> Option<Sample> {
        let last = self.samples.last_mut()?;
        last.stable = Some(stable);
        Some(last.clone())
    }

    /// Every trial so far.
    pub fn report(&self) -> Report {
        Report {
            samples: self.samples.clone(),
            sorted_ns: self.recorder.sorted_samples(),
            summary: self.recorder.summary().map(Summary::from),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

    use super::*;

    const MS: u64 = 1_000_000;

    fn trial(typed: &str, expected: &str) -> Trial {
        Trial {
            cell: "L0001".to_owned(),
            typed: typed.to_owned(),
            expected: expected.to_owned(),
        }
    }

    fn type_keys(s: &mut Session, from: u64, keys: &str) -> u64 {
        let mut t = from;
        for ch in keys.chars() {
            t += MS;
            s.key(t, t, ch);
        }
        t
    }

    #[test]
    fn line_breaks_are_compared_as_line_feeds() {
        assert!(matches("a\r\nb", "a\nb"));
        assert!(matches("a\rb", "a\nb"));
        assert!(matches("a\nb\r", "a\r\nb\n"));
        assert!(!matches("a b", "a\nb"));
        assert!(!matches("ab ", "ab"));
        assert!(!matches("Ab", "ab"));
    }

    #[test]
    fn t0_is_the_last_key_and_t1_the_first_matching_change() {
        let mut s = Session::new(Duration::ZERO);
        s.arm(trial(";labc", "Kind regards"), 0, DEFAULT_KEY_WAIT)
            .unwrap();
        // Typing changes the text before T0: never a match.
        s.changed(MS, MS, ";");
        let t0 = type_keys(&mut s, 0, ";labc");
        assert_eq!(t0, 5 * MS);
        // The expander's backspaces and text: keys after T0 are ignored.
        s.key(6 * MS, 6 * MS, '\u{8}');
        s.changed(7 * MS, 7 * MS, "");
        s.changed(8 * MS, 8 * MS, "Kind");
        assert!(s.take_finished().is_none());
        s.changed(9 * MS, 9 * MS, "Kind regards");
        let sample = s.take_finished().unwrap();
        assert!(sample.pass);
        assert_eq!(sample.latency_ns, 4 * MS);
        assert_eq!((sample.t0_ns, sample.t1_ns), (Some(5 * MS), Some(9 * MS)));
        assert_eq!(sample.failure, None);
        // A later change makes the text differ: the settle check sees it.
        s.changed(10 * MS, 10 * MS, "Kind regards!");
        assert_eq!(s.latest_matches(), Some(false));
        assert_eq!(s.mark_stable(false).unwrap().stable, Some(false));
    }

    #[test]
    fn stray_keys_before_the_abbreviation_do_not_matter() {
        let mut s = Session::new(Duration::ZERO);
        s.arm(trial("vab ", "x "), 0, DEFAULT_KEY_WAIT).unwrap();
        let t = type_keys(&mut s, 0, "zzvavab ");
        s.changed(t + MS, t + MS, "x ");
        let sample = s.take_finished().unwrap();
        assert_eq!(sample.t0_ns, Some(t));
        assert_eq!(sample.latency_ns, MS);
    }

    #[test]
    fn no_match_within_two_seconds_is_a_failure_counted_as_two_seconds() {
        let mut s = Session::new(Duration::ZERO);
        s.arm(trial(";l", "body"), 0, DEFAULT_KEY_WAIT).unwrap();
        let t0 = type_keys(&mut s, 0, ";l");
        s.expire(t0 + TIMEOUT_NS);
        assert!(s.take_finished().is_none(), "exactly 2 s is not past");
        s.expire(t0 + TIMEOUT_NS + 1);
        let sample = s.take_finished().unwrap();
        assert!(!sample.pass);
        assert_eq!(sample.failure, Some(Failure::Timeout));
        assert_eq!(sample.latency_ns, TIMEOUT_NS);
        assert_eq!(s.report().sorted_ns, vec![TIMEOUT_NS]);
    }

    #[test]
    fn a_match_stamped_after_two_seconds_fails() {
        // A slow page: the change arrives in time but is stamped late.
        let mut s = Session::new(Duration::from_secs(1));
        s.arm(trial(";l", "body"), 0, DEFAULT_KEY_WAIT).unwrap();
        let t0 = type_keys(&mut s, 0, ";l");
        s.changed(t0 + TIMEOUT_NS + MS, t0 + 100 * MS, "body");
        let sample = s.take_finished().unwrap();
        assert_eq!(sample.failure, Some(Failure::Timeout));
        assert_eq!(sample.t1_ns, None);
    }

    #[test]
    fn the_slack_lets_a_late_delivery_count() {
        let mut s = Session::new(Duration::from_secs(1));
        s.arm(trial(";l", "body"), 0, DEFAULT_KEY_WAIT).unwrap();
        let t0 = type_keys(&mut s, 0, ";l");
        s.expire(t0 + TIMEOUT_NS + 500 * MS);
        assert!(s.take_finished().is_none());
        s.changed(t0 + 30 * MS, t0 + TIMEOUT_NS + 600 * MS, "body");
        let sample = s.take_finished().unwrap();
        assert!(sample.pass);
        assert_eq!(sample.latency_ns, 30 * MS);
    }

    #[test]
    fn a_last_key_that_never_arrives_fails_after_the_key_wait() {
        let mut s = Session::new(Duration::ZERO);
        s.arm(trial(";labc", "body"), 0, Duration::from_millis(100))
            .unwrap();
        type_keys(&mut s, 0, ";lab");
        s.expire(100 * MS);
        assert!(s.take_finished().is_none());
        s.expire(101 * MS);
        let sample = s.take_finished().unwrap();
        assert_eq!(sample.failure, Some(Failure::NoKey));
        assert_eq!((sample.t0_ns, sample.latency_ns), (None, TIMEOUT_NS));
    }

    #[test]
    fn one_trial_at_a_time_and_cancel_records_nothing() {
        let mut s = Session::new(Duration::ZERO);
        s.arm(trial(";l", "b"), 0, DEFAULT_KEY_WAIT).unwrap();
        assert!(s.busy());
        assert!(s.arm(trial(";l", "b"), 0, DEFAULT_KEY_WAIT).is_err());
        s.cancel();
        assert!(!s.busy());
        assert!(s.report().samples.is_empty());
        assert!(s.arm(trial("", "b"), 0, DEFAULT_KEY_WAIT).is_err());
        // Events with nothing armed are ignored.
        s.key(1, 1, ';');
        s.changed(2, 2, "b");
        assert!(s.take_finished().is_none());
    }

    #[test]
    fn the_report_is_in_the_recorder_format() {
        let mut s = Session::new(Duration::ZERO);
        for (i, lat) in [30_u64, 10, 20].into_iter().enumerate() {
            let base = u64::try_from(i).unwrap() * 10_000 * MS;
            s.arm(trial("k", "v"), base, DEFAULT_KEY_WAIT).unwrap();
            s.key(base + MS, base + MS, 'k');
            s.changed(base + MS + lat * MS, base + MS + lat * MS, "v");
            assert!(s.take_finished().unwrap().pass);
        }
        let r = s.report();
        assert_eq!(r.sorted_ns, vec![10 * MS, 20 * MS, 30 * MS]);
        let sum = r.summary.unwrap();
        assert_eq!(
            (sum.count, sum.p50, sum.p95, sum.max),
            (3, 20 * MS, 30 * MS, 30 * MS)
        );
        assert_eq!(r.samples.len(), 3);
    }

    #[test]
    fn debug_leaves_the_text_out() {
        let t = trial(";secretabbr", "secret body");
        let text = format!("{t:?}");
        assert!(!text.contains("secret"), "{text}");
        let mut s = Session::new(Duration::ZERO);
        s.arm(t, 0, DEFAULT_KEY_WAIT).unwrap();
        assert!(!format!("{s:?}").contains("secret"));
    }
}
