//! The inspection thread: asks the question that can wait for another application, so that
//! the caller never does. No system call here; the thread's work is a [`Probe`].
//!
//! A UI Automation call is answered by the application that owns the focused element. An
//! application that is busy answers late and one that hangs does not answer, so the call is
//! made on a thread of its own and the caller waits for the answer with a limit:
//!
//! - **In time:** the answer.
//! - **Not in time:** [`Answer::TimedOut`]. The thread is given up: it keeps waiting for the
//!   application, ends when the call returns, and its late answer goes nowhere. The next
//!   question gets a fresh thread, so one hung application costs one question, not all that
//!   follow.
//! - **A thread that is still starting** (loading the automation library takes a moment the
//!   first time) is not given up: the question times out, the thread finishes starting and
//!   serves the next one. A question whose time is over when the thread gets to it is
//!   skipped.
//! - At most [`MAX_WORKERS`] threads exist. When that many wait for applications that do not
//!   answer, further questions are answered `TimedOut` at once until one of them returns.
//!
//! A thread without work sleeps in its channel and wakes for nothing else.
//!
//! # A late answer
//!
//! An application that answers, only slower than the caller waits (a browser engine that has
//! just switched its accessibility on, a toolkit busy starting), would time out at every
//! question if each one asked afresh: each call takes as long as the one before. So a question
//! may name its [`Subject`]: the window it is about and the watcher's count of the focus changes
//! it has reported. When such a question times out, the given-up thread's answer is kept for
//! when it comes, and the next question about the **same subject** waits for that answer
//! instead of asking again (and starting another thread). It is the answer about the element
//! the window had focused after the last focus change the watcher saw, which is what a fresh
//! call would have said had it been fast enough. A question about another window, or after
//! another focus change, drops it: the late answer then goes nowhere, as before. A question
//! without a subject never uses one, and none is used once [`LATE_ANSWER_LIFETIME`] has passed
//! since the question it was asked for (a field can change without a focus change, which only a
//! fresh call would see; the watcher's reports again are over well before).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::classify::Observation;

/// The most inspection threads at a time: the one in use and those given up that still wait
/// for an application.
pub(crate) const MAX_WORKERS: usize = 4;

/// How long after its question a late answer may still serve another one (module docs).
pub(crate) const LATE_ANSWER_LIFETIME: Duration = Duration::from_secs(10);

/// Asks the system about the focused element. Used on the thread that opened it.
pub(crate) trait Probe {
    /// What the focused element is. May take as long as the application in front likes.
    fn observe(&mut self) -> Observation;
}

/// Opens probes.
pub(crate) trait ProbeSource: Send + Sync + 'static {
    /// The probe.
    type Probe: Probe;

    /// A probe for the calling thread, which is the inspection thread. `None` when the
    /// system refuses.
    fn open(&self) -> Option<Self::Probe>;
}

/// What a question is about, for the rule of a late answer (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Subject {
    /// The window asked about.
    pub(crate) window: u64,
    /// How many focus changes the watcher had reported when the question was asked.
    pub(crate) epoch: u64,
}

/// The answer of a given-up thread, still to come.
struct Late {
    subject: Subject,
    /// When the question it was asked for was asked.
    asked: Instant,
    answered: Receiver<Observation>,
}

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The probe answered in time.
    Observed(Observation),
    /// It did not.
    TimedOut,
    /// There is no probe: the thread could not be created or could not open one.
    Unavailable,
}

/// One question.
struct Job {
    /// When the caller stops waiting.
    deadline: Instant,
    reply: SyncSender<Observation>,
}

/// The way to an inspection thread.
struct Worker {
    id: u64,
    jobs: Sender<Job>,
    /// The thread has its probe: from here on, a question that times out waits for an
    /// application.
    opened: Arc<AtomicBool>,
}

#[derive(Default)]
struct Slot {
    worker: Option<Worker>,
    last_id: u64,
    /// The answer a question that timed out is still waiting for (module docs).
    late: Option<Late>,
}

/// Counts a thread for as long as it exists.
struct Live(Arc<AtomicUsize>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The inspection threads of one platform.
pub(crate) struct Inspector<S: ProbeSource> {
    source: Arc<S>,
    slot: Mutex<Slot>,
    live: Arc<AtomicUsize>,
    /// [`LATE_ANSWER_LIFETIME`], shorter in tests.
    late_lifetime: Duration,
}

impl<S: ProbeSource> Inspector<S> {
    /// An inspector without a thread; the first question (or [`warm`](Self::warm)) starts
    /// one.
    pub(crate) fn new(source: S) -> Self {
        Self {
            source: Arc::new(source),
            slot: Mutex::new(Slot::default()),
            live: Arc::new(AtomicUsize::new(0)),
            late_lifetime: LATE_ANSWER_LIFETIME,
        }
    }

    /// The same inspector with another lifetime of a late answer.
    #[cfg(test)]
    pub(crate) const fn with_late_lifetime(mut self, lifetime: Duration) -> Self {
        self.late_lifetime = lifetime;
        self
    }

    fn slot(&self) -> MutexGuard<'_, Slot> {
        // A panic while the slot was locked left a whole value behind.
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many inspection threads exist.
    pub(crate) fn workers(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Starts the inspection thread now, so that the first question does not pay for it.
    pub(crate) fn warm(&self) {
        let mut slot = self.slot();
        if slot.worker.is_none() && self.workers() < MAX_WORKERS {
            slot.worker = self.spawn(&mut slot.last_id);
        }
    }

    /// Asks the probe and waits for the answer for at most `timeout`. With a `subject`, a late
    /// answer to an earlier question about the same subject serves this one (module docs).
    pub(crate) fn observe(&self, subject: Option<Subject>, timeout: Duration) -> Answer {
        let now = Instant::now();
        // A limit the clock cannot express is as good as a day.
        let timeout = timeout.min(Duration::from_hours(24));
        let deadline = now + timeout;
        let late = self.slot().late.take();
        if let Some(late) = late
            && Some(late.subject) == subject
            && late.asked.elapsed() <= self.late_lifetime
        {
            match late.answered.recv_timeout(timeout) {
                Ok(observation) => return Answer::Observed(observation),
                Err(RecvTimeoutError::Timeout) => {
                    // Still not there: it is waited for again by the next question.
                    self.slot().late = Some(late);
                    return Answer::TimedOut;
                }
                // The thread ended without an answer: ask afresh.
                Err(RecvTimeoutError::Disconnected) => {}
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let (reply, answered) = mpsc::sync_channel(1);
        let (id, opened) = match self.submit(Job { deadline, reply }) {
            Ok(worker) => worker,
            Err(answer) => return answer,
        };
        match answered.recv_timeout(left) {
            Ok(observation) => Answer::Observed(observation),
            Err(RecvTimeoutError::Timeout) => {
                if opened.load(Ordering::SeqCst) {
                    self.give_up(id);
                    if let Some(subject) = subject {
                        self.slot().late = Some(Late {
                            subject,
                            asked: now,
                            answered,
                        });
                    }
                }
                Answer::TimedOut
            }
            // The thread dropped the question: it skipped it because the time was over, or
            // it ended.
            Err(RecvTimeoutError::Disconnected) if Instant::now() >= deadline => Answer::TimedOut,
            Err(RecvTimeoutError::Disconnected) => {
                self.give_up(id);
                Answer::Unavailable
            }
        }
    }

    /// Hands `job` to the inspection thread, starting one when there is none.
    fn submit(&self, job: Job) -> Result<(u64, Arc<AtomicBool>), Answer> {
        let mut slot = self.slot();
        let mut job = job;
        if let Some(worker) = &slot.worker {
            match worker.jobs.send(job) {
                Ok(()) => return Ok((worker.id, Arc::clone(&worker.opened))),
                // The thread has ended.
                Err(returned) => {
                    job = returned.0;
                    slot.worker = None;
                }
            }
        }
        if self.workers() >= MAX_WORKERS {
            return Err(Answer::TimedOut);
        }
        let worker = self.spawn(&mut slot.last_id).ok_or(Answer::Unavailable)?;
        let handed = worker
            .jobs
            .send(job)
            .map(|()| (worker.id, Arc::clone(&worker.opened)))
            .map_err(|_| Answer::Unavailable);
        slot.worker = Some(worker);
        handed
    }

    /// Starts an inspection thread. Called with the slot locked, which is what keeps the
    /// count of threads below the limit.
    fn spawn(&self, last_id: &mut u64) -> Option<Worker> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let opened = Arc::new(AtomicBool::new(false));
        let source = Arc::clone(&self.source);
        let is_open = Arc::clone(&opened);
        self.live.fetch_add(1, Ordering::SeqCst);
        // Moved into the thread; dropped with the closure if the thread cannot be created.
        let live = Live(Arc::clone(&self.live));
        std::thread::Builder::new()
            .name("iamb-focus-inspect".to_owned())
            .spawn(move || {
                let _live = live;
                let Some(mut probe) = source.open() else {
                    return;
                };
                is_open.store(true, Ordering::SeqCst);
                // Ends when the inspector has dropped the sender: given up, or gone.
                while let Ok(job) = queue.recv() {
                    if Instant::now() >= job.deadline {
                        continue;
                    }
                    // A caller that stopped waiting has dropped the receiver.
                    let _ = job.reply.send(probe.observe());
                }
            })
            .ok()?;
        *last_id += 1;
        Some(Worker {
            id: *last_id,
            jobs,
            opened,
        })
    }

    /// Waits until the thread in use has its probe.
    #[cfg(test)]
    pub(crate) fn wait_open(&self) {
        self.warm();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let open = self
                .slot()
                .worker
                .as_ref()
                .is_some_and(|worker| worker.opened.load(Ordering::SeqCst));
            if open {
                return;
            }
            assert!(Instant::now() < deadline, "the probe did not open");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Forgets thread `id`, if it is still the one in use. It ends once the call it is in
    /// returns.
    fn give_up(&self, id: u64) {
        let mut slot = self.slot();
        if slot.worker.as_ref().is_some_and(|worker| worker.id == id) {
            slot.worker = None;
        }
    }
}

impl<S: ProbeSource> std::fmt::Debug for Inspector<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inspector")
            .field("workers", &self.workers())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A probe that follows a script, for the tests of this module and of `secure`.

    use std::collections::VecDeque;
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::{Arc, Mutex, PoisonError};

    use super::{Observation, Probe, ProbeSource};

    /// What the probe does when asked.
    pub(crate) enum Step {
        /// Answers at once.
        Answer(Observation),
        /// Waits until the test lets go (or drops the sender), then answers.
        Hang(Receiver<()>, Observation),
    }

    /// A hanging step and the way to end it.
    pub(crate) fn hang(then: Observation) -> (Step, Sender<()>) {
        let (release, released) = std::sync::mpsc::channel();
        (Step::Hang(released, then), release)
    }

    #[derive(Default)]
    pub(crate) struct Script {
        steps: Mutex<VecDeque<Step>>,
        /// Asked how many times.
        pub(crate) asked: Mutex<u32>,
        /// Opening waits for this.
        pub(crate) open_gate: Mutex<Option<Receiver<()>>>,
        /// Opening fails.
        pub(crate) refuse_open: Mutex<bool>,
    }

    impl Script {
        pub(crate) fn push(&self, step: Step) {
            self.steps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(step);
        }

        pub(crate) fn asked(&self) -> u32 {
            *self.asked.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// The source: every probe it opens follows the same script.
    #[derive(Clone, Default)]
    pub(crate) struct FakeSource(pub(crate) Arc<Script>);

    impl FakeSource {
        pub(crate) fn answering(observations: &[Observation]) -> Self {
            let source = Self::default();
            for observation in observations {
                source.0.push(Step::Answer(*observation));
            }
            source
        }
    }

    pub(crate) struct FakeProbe(Arc<Script>);

    impl Probe for FakeProbe {
        fn observe(&mut self) -> Observation {
            *self.0.asked.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            let step = self
                .0
                .steps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            match step {
                Some(Step::Answer(observation)) => observation,
                Some(Step::Hang(released, observation)) => {
                    let _ = released.recv();
                    observation
                }
                None => Observation::Failed,
            }
        }
    }

    impl ProbeSource for FakeSource {
        type Probe = FakeProbe;

        fn open(&self) -> Option<FakeProbe> {
            let gate = self
                .0
                .open_gate
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(gate) = gate {
                let _ = gate.recv();
            }
            let refused = *self
                .0
                .refuse_open
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            (!refused).then(|| FakeProbe(Arc::clone(&self.0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeSource, Step, hang};
    use super::*;

    const LONG: Duration = Duration::from_secs(10);
    const SHORT: Duration = Duration::from_millis(30);

    const PLAIN: Observation = Observation::Element {
        password: false,
        control_type: 50004,
    };
    const PASSWORD: Observation = Observation::Element {
        password: true,
        control_type: 50004,
    };

    /// Waits until `inspector` has `count` threads.
    fn wait_for_workers(inspector: &Inspector<FakeSource>, count: usize) {
        let deadline = Instant::now() + LONG;
        while inspector.workers() != count {
            assert!(Instant::now() < deadline, "still {}", inspector.workers());
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn answers_come_back_in_order_from_one_thread() {
        let source = FakeSource::answering(&[PLAIN, PASSWORD, Observation::NoElement]);
        let inspector = Inspector::new(source.clone());
        assert_eq!(
            inspector.workers(),
            0,
            "no thread before the first question"
        );
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PLAIN));
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PASSWORD));
        assert_eq!(
            inspector.observe(None, LONG),
            Answer::Observed(Observation::NoElement)
        );
        assert_eq!(inspector.workers(), 1);
        assert_eq!(source.0.asked(), 3);
        drop(inspector);
    }

    #[test]
    fn a_hung_application_costs_one_question() {
        let source = FakeSource::default();
        let (step, release) = hang(PASSWORD);
        source.0.push(step);
        source.0.push(Step::Answer(PLAIN));
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();

        let started = Instant::now();
        assert_eq!(inspector.observe(None, SHORT), Answer::TimedOut);
        let waited = started.elapsed();
        assert!(waited >= SHORT, "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");

        // The next question is served by a fresh thread while the first still hangs.
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PLAIN));
        assert_eq!(inspector.workers(), 2);

        // The hung call returns: its late answer goes nowhere and its thread ends.
        release.send(()).expect("the thread still waits");
        wait_for_workers(&inspector, 1);
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn no_more_than_the_limit_of_threads_wait_for_hung_applications() {
        let source = FakeSource::default();
        let mut releases = Vec::new();
        for _ in 0..MAX_WORKERS {
            let (step, release) = hang(PLAIN);
            source.0.push(step);
            releases.push(release);
        }
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone());
        for _ in 0..MAX_WORKERS {
            inspector.wait_open();
            assert_eq!(inspector.observe(None, SHORT), Answer::TimedOut);
        }
        assert_eq!(inspector.workers(), MAX_WORKERS);

        // Every thread hangs: answered at once, without another thread and without asking.
        let started = Instant::now();
        assert_eq!(inspector.observe(None, LONG), Answer::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(inspector.workers(), MAX_WORKERS);
        assert_eq!(source.0.asked(), u32::try_from(MAX_WORKERS).expect("small"));

        // One returns: there is room again.
        releases.remove(0).send(()).expect("the thread still waits");
        wait_for_workers(&inspector, MAX_WORKERS - 1);
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PASSWORD));
        drop(releases);
        wait_for_workers(&inspector, 1);
    }

    #[test]
    fn a_thread_that_is_still_starting_is_kept() {
        let source = FakeSource::answering(&[PLAIN, PASSWORD]);
        let (open, gate) = mpsc::channel();
        *source.0.open_gate.lock().expect("not poisoned") = Some(gate);
        let inspector = Inspector::new(source.clone());

        assert_eq!(inspector.observe(None, SHORT), Answer::TimedOut);
        assert_eq!(inspector.workers(), 1);
        open.send(()).expect("the thread waits to open");

        // The same thread serves the next question. The first one's time was over when the
        // thread got to it, so it was skipped: the probe is asked once and its first answer
        // goes to the second question.
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PLAIN));
        assert_eq!(inspector.workers(), 1);
        assert_eq!(source.0.asked(), 1);
    }

    #[test]
    fn warming_starts_the_thread_before_the_first_question() {
        let source = FakeSource::answering(&[PLAIN]);
        let inspector = Inspector::new(source);
        inspector.warm();
        inspector.warm();
        assert_eq!(inspector.workers(), 1);
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PLAIN));
        assert_eq!(inspector.workers(), 1);
    }

    #[test]
    fn a_probe_that_cannot_be_opened_is_unavailable_not_normal() {
        let source = FakeSource::answering(&[PLAIN]);
        *source.0.refuse_open.lock().expect("not poisoned") = true;
        let inspector = Inspector::new(source.clone());
        assert_eq!(inspector.observe(None, LONG), Answer::Unavailable);
        wait_for_workers(&inspector, 0);
        assert_eq!(source.0.asked(), 0);

        // Once the system lets it open, the next question is served.
        *source.0.refuse_open.lock().expect("not poisoned") = false;
        assert_eq!(inspector.observe(None, LONG), Answer::Observed(PLAIN));
    }

    #[test]
    fn no_time_is_a_timeout() {
        let source = FakeSource::default();
        let (step, _release) = hang(PLAIN);
        source.0.push(step);
        let inspector = Inspector::new(source);
        assert_eq!(inspector.observe(None, Duration::ZERO), Answer::TimedOut);
    }

    #[test]
    fn a_limit_too_large_for_the_clock_is_accepted() {
        let inspector = Inspector::new(FakeSource::answering(&[PLAIN]));
        assert_eq!(
            inspector.observe(None, Duration::MAX),
            Answer::Observed(PLAIN)
        );
    }

    const SUBJECT: Subject = Subject {
        window: 0x1_0000_0A10,
        epoch: 7,
    };

    #[test]
    fn a_late_answer_serves_the_next_question_about_the_same_subject() {
        let source = FakeSource::default();
        let (step, release) = hang(PASSWORD);
        source.0.push(step);
        source.0.push(Step::Answer(PLAIN));
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();

        assert_eq!(inspector.observe(Some(SUBJECT), SHORT), Answer::TimedOut);
        // The application answers after the caller stopped waiting.
        release.send(()).expect("the thread still waits");
        // Asked again about the same window, with no focus change since: that answer, without
        // asking the application again.
        assert_eq!(
            inspector.observe(Some(SUBJECT), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 1);
        // It serves one question: the next one asks.
        assert_eq!(
            inspector.observe(Some(SUBJECT), LONG),
            Answer::Observed(PLAIN)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn a_slow_application_is_waited_for_and_not_asked_again_and_again() {
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();

        // Three questions while the first call is still running: one call, one thread.
        for _ in 0..3 {
            assert_eq!(inspector.observe(Some(SUBJECT), SHORT), Answer::TimedOut);
        }
        assert_eq!(source.0.asked(), 1);
        assert_eq!(inspector.workers(), 1);
        release.send(()).expect("the thread still waits");
        assert_eq!(
            inspector.observe(Some(SUBJECT), LONG),
            Answer::Observed(PLAIN)
        );
        wait_for_workers(&inspector, 0);
    }

    #[test]
    fn a_late_answer_never_serves_another_window_or_a_later_focus_change() {
        let other_window = Subject {
            window: 0x1_0000_0B20,
            ..SUBJECT
        };
        let after_a_change = Subject {
            epoch: SUBJECT.epoch + 1,
            ..SUBJECT
        };
        for later in [Some(other_window), Some(after_a_change), None] {
            let source = FakeSource::default();
            let (step, release) = hang(PLAIN);
            source.0.push(step);
            source.0.push(Step::Answer(PASSWORD));
            let inspector = Inspector::new(source.clone());
            inspector.wait_open();
            assert_eq!(inspector.observe(Some(SUBJECT), SHORT), Answer::TimedOut);
            release.send(()).expect("the thread still waits");
            // The late "plain" is not taken: the application is asked again, and says
            // "password".
            assert_eq!(
                inspector.observe(later, LONG),
                Answer::Observed(PASSWORD),
                "{later:?}"
            );
            assert_eq!(source.0.asked(), 2, "{later:?}");
            // And it is gone: not even the first subject gets it now.
            source.0.push(Step::Answer(PASSWORD));
            assert_eq!(
                inspector.observe(Some(SUBJECT), LONG),
                Answer::Observed(PASSWORD),
                "{later:?}"
            );
        }
    }

    #[test]
    fn an_old_late_answer_is_not_taken() {
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone()).with_late_lifetime(SHORT);
        inspector.wait_open();
        assert_eq!(inspector.observe(Some(SUBJECT), SHORT), Answer::TimedOut);
        release.send(()).expect("the thread still waits");
        std::thread::sleep(SHORT * 2);
        assert_eq!(
            inspector.observe(Some(SUBJECT), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn a_question_without_a_subject_leaves_no_late_answer() {
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();
        assert_eq!(inspector.observe(None, SHORT), Answer::TimedOut);
        release.send(()).expect("the thread still waits");
        assert_eq!(
            inspector.observe(Some(SUBJECT), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn the_debug_form_shows_a_count_only() {
        let inspector = Inspector::new(FakeSource::default());
        assert_eq!(format!("{inspector:?}"), "Inspector { workers: 0, .. }");
    }
}
