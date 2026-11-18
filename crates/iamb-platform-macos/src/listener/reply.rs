//! The answer of a job that runs on another thread, with a deadline for the one who waits.
//!
//! Used by `main_thread::call`. The rule it keeps: the caller either gets the job's result or
//! knows that the job never started. A job that has started is waited for even past the
//! deadline, because its effects are real (a hotkey is registered or it is not); a job that
//! has not started by the deadline is cancelled and will do nothing when its turn comes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How far a job has come.
enum Outcome<R> {
    /// It has not finished (or not started).
    Pending,
    /// It finished with this result; `None` when the work panicked.
    Finished(Option<R>),
}

/// Where a job leaves its result for the thread that waits for it.
pub(crate) struct Reply<R> {
    outcome: Mutex<Outcome<R>>,
    done: Condvar,
    /// Set by whoever comes first: the job when it starts, or the caller when it gives up.
    claimed: AtomicBool,
}

impl<R> Default for Reply<R> {
    fn default() -> Self {
        Self {
            outcome: Mutex::new(Outcome::Pending),
            done: Condvar::new(),
            claimed: AtomicBool::new(false),
        }
    }
}

impl<R> Reply<R> {
    fn lock(&self) -> MutexGuard<'_, Outcome<R>> {
        self.outcome.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the reply for the job. `false` means the caller gave up first: the job must
    /// not start.
    pub(crate) fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    /// Leaves the job's result (`None` when the work panicked) and wakes the caller.
    pub(crate) fn finish(&self, result: Option<R>) {
        *self.lock() = Outcome::Finished(result);
        self.done.notify_all();
    }

    /// Waits for the job's result, at most `timeout` unless the job has started by then.
    /// `None` when the job was cancelled or panicked.
    pub(crate) fn wait(&self, timeout: Duration) -> Option<R> {
        let deadline = Instant::now() + timeout;
        let mut outcome = self.lock();
        let mut gave_up = false;
        loop {
            if let Outcome::Finished(result) = &mut *outcome {
                return result.take();
            }
            if gave_up {
                // The job started just before the deadline. It will finish, and its effects
                // are real, so wait for its answer.
                outcome = self
                    .done
                    .wait(outcome)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if self.claim() {
                    // Not started, and now it never will.
                    return None;
                }
                gave_up = true;
                continue;
            }
            outcome = self
                .done
                .wait_timeout(outcome, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_reply_that_arrives_in_time_is_returned() {
        let reply = Arc::new(Reply::default());
        let worker = {
            let reply = Arc::clone(&reply);
            std::thread::spawn(move || {
                assert!(reply.claim());
                reply.finish(Some(5));
            })
        };
        assert_eq!(reply.wait(Duration::from_secs(10)), Some(5));
        worker.join().expect("the worker thread");
    }

    #[test]
    fn a_job_that_started_before_the_deadline_is_waited_for() {
        let reply = Arc::new(Reply::default());
        // The job has started (it claimed the reply) but takes longer than the timeout.
        assert!(reply.claim());
        let worker = {
            let reply = Arc::clone(&reply);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(60));
                reply.finish(Some("late"));
            })
        };
        assert_eq!(reply.wait(Duration::from_millis(5)), Some("late"));
        worker.join().expect("the worker thread");
    }

    #[test]
    fn a_job_that_panicked_still_answers() {
        let reply = Reply::<u8>::default();
        assert!(reply.claim());
        reply.finish(None);
        assert_eq!(reply.wait(Duration::from_secs(10)), None);
    }

    #[test]
    fn a_job_that_comes_after_the_caller_gave_up_does_not_start() {
        let reply = Reply::<u8>::default();
        let started = Instant::now();
        assert_eq!(reply.wait(Duration::from_millis(20)), None);
        assert!(started.elapsed() >= Duration::from_millis(20));
        // What the job checks first: the caller was there before it.
        assert!(!reply.claim());
    }

    #[test]
    fn only_one_side_can_claim() {
        let reply = Reply::<u8>::default();
        assert!(reply.claim());
        assert!(!reply.claim());
    }
}
