//! The cold-open measurement (KPI of task 3.2: "UI opens cold in under 400 ms").
//!
//! "Cold" means the library window does not exist: it is created on demand and destroyed when
//! closed (architecture §10). The clock starts when the tray's "Open library" action is handled
//! ([`OpenTimer::start`], before the window is built) and stops at the window's first paint,
//! which the front end reports through the `report_first_paint` command with the time it
//! painted (`performance.timeOrigin + performance.now()`, read in a task queued from a
//! `requestAnimationFrame` callback, so after the frame was presented). Two numbers come out:
//!
//! - `paint_ms`: the front end's paint time minus the wall-clock time of the action. Both are
//!   the system's wall clock, read in two processes on the same machine.
//! - `report_ms`: the monotonic time from the action until the report arrived in Rust; an upper
//!   bound that adds the report's own trip (about a millisecond).
//!
//! The report also carries when the page's navigation began (`performance.timeOrigin`: the web
//! view exists and asks for the page) and when its script began to run, which split the time
//! into creating the web view, loading the page, and rendering it.
//!
//! `iamb-desktop --measure-open RUNS` runs the action `RUNS` times on its own and prints the
//! numbers (`app::measure`); `docs/test-strategy.md` records how and what it gave.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One open, measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// From the action to the front end's paint, by the wall clock.
    pub paint_ms: f64,
    /// From the action to the report's arrival, by the monotonic clock.
    pub report_ms: f64,
    /// From the action to the start of the page's navigation (the web view exists and asks
    /// for the page), by the wall clock.
    pub navigation_ms: f64,
    /// From the action to the start of the page's script, by the wall clock.
    pub script_ms: f64,
}

/// What the front end reports at its first paint, in epoch milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintMarks {
    /// The page's navigation began (`performance.timeOrigin`).
    pub navigation_at_ms: f64,
    /// Its script began to run.
    pub script_at_ms: f64,
    /// It painted for the first time.
    pub painted_at_ms: f64,
}

#[derive(Debug, Clone, Copy)]
struct Started {
    at: Instant,
    wall_ms: f64,
}

/// The pending open and its result.
#[derive(Debug, Default)]
pub struct OpenTimer {
    state: Mutex<(Option<Started>, Option<Measurement>)>,
    done: Condvar,
}

/// Milliseconds since the Unix epoch, as a float (the front end's clock has the same unit).
#[must_use]
pub fn wall_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

impl OpenTimer {
    /// Starts timing an open (the tray action was handled).
    pub fn start(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        *state = (
            Some(Started {
                at: Instant::now(),
                wall_ms: wall_ms(),
            }),
            None,
        );
    }

    /// The window reported its first paint with `marks`. Returns the measurement when an
    /// open was being timed; a paint without one (a reload) is ignored.
    pub fn painted(&self, marks: PaintMarks) -> Option<Measurement> {
        let arrived = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let started = state.0.take()?;
        let m = Measurement {
            paint_ms: marks.painted_at_ms - started.wall_ms,
            report_ms: arrived.duration_since(started.at).as_secs_f64() * 1000.0,
            navigation_ms: marks.navigation_at_ms - started.wall_ms,
            script_ms: marks.script_at_ms - started.wall_ms,
        };
        state.1 = Some(m);
        self.done.notify_all();
        Some(m)
    }

    /// Waits up to `timeout` for the pending open's measurement.
    pub fn wait(&self, timeout: Duration) -> Option<Measurement> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(m) = state.1.take() {
                return Some(m);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// The result of several opens: the first (the coldest: the first web view of the process)
/// and the later ones.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The first open's paint time.
    pub first_ms: f64,
    /// The median paint time of the later opens (the first when there is only one).
    pub median_later_ms: f64,
    /// The slowest paint time of all opens.
    pub max_ms: f64,
    /// How many opens were measured.
    pub runs: usize,
}

/// Summarises the paint times of `runs` (in order); `None` when empty.
#[must_use]
pub fn summarize(runs: &[Measurement]) -> Option<Summary> {
    let first = runs.first()?.paint_ms;
    let mut later: Vec<f64> = runs.iter().skip(1).map(|m| m.paint_ms).collect();
    later.sort_by(f64::total_cmp);
    let median_later_ms = if later.is_empty() {
        first
    } else if later.len() % 2 == 1 {
        later[later.len() / 2]
    } else {
        f64::midpoint(later[later.len() / 2 - 1], later[later.len() / 2])
    };
    let max_ms = runs.iter().map(|m| m.paint_ms).fold(first, f64::max);
    Some(Summary {
        first_ms: first,
        median_later_ms,
        max_ms,
        runs: runs.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(paint_ms: f64) -> Measurement {
        Measurement {
            paint_ms,
            report_ms: paint_ms + 1.0,
            navigation_ms: paint_ms / 2.0,
            script_ms: paint_ms * 0.75,
        }
    }

    fn marks(after_ms: f64) -> PaintMarks {
        let now = wall_ms();
        PaintMarks {
            navigation_at_ms: now + after_ms / 2.0,
            script_at_ms: now + after_ms * 0.75,
            painted_at_ms: now + after_ms,
        }
    }

    #[test]
    fn a_paint_is_measured_only_for_a_pending_open() {
        let t = OpenTimer::default();
        assert_eq!(t.painted(marks(0.0)), None);
        t.start();
        let got = t.painted(marks(8.0)).expect("pending");
        assert!(got.paint_ms >= 8.0 && got.paint_ms < 1000.0, "{got:?}");
        assert!(got.navigation_ms < got.script_ms && got.script_ms < got.paint_ms);
        assert_eq!(t.wait(Duration::ZERO), Some(got));
        assert_eq!(t.wait(Duration::from_millis(1)), None);
        assert_eq!(t.painted(marks(0.0)), None, "one paint per open");
    }

    #[test]
    fn the_summary_separates_the_first_open() {
        assert_eq!(summarize(&[]), None);
        let s = summarize(&[m(300.0)]).expect("one");
        assert!((s.median_later_ms - 300.0).abs() < f64::EPSILON);
        let s = summarize(&[m(300.0), m(90.0), m(110.0), m(100.0)]).expect("four");
        assert!((s.first_ms - 300.0).abs() < f64::EPSILON);
        assert!((s.median_later_ms - 100.0).abs() < f64::EPSILON);
        assert!((s.max_ms - 300.0).abs() < f64::EPSILON);
        assert_eq!(s.runs, 4);
        let s = summarize(&[m(1.0), m(4.0), m(2.0)]).expect("three");
        assert!((s.median_later_ms - 3.0).abs() < f64::EPSILON);
    }
}
