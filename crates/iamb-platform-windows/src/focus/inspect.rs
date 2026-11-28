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
//! may name its [`Subject`]: the window it is about, the control with the keyboard focus in it
//! and that control's kind, and the watcher's count of the focus changes it has reported. When
//! such a question times out, the given-up thread's answer is kept for when it comes, and:
//!
//! - **its arrival wakes the watcher** (the question's [`Wake`]), which reports the window again
//!   at once, so the answer is used when it is new, not at the next pause;
//! - the next question about the **same subject** that **may take a late answer** (only a report
//!   the watcher made again, not one of a change: the caller decides) takes it if its element
//!   was chosen at most [`LATE_ANSWER_FRESH`] ago, or waits for it if it has not arrived yet.
//!   One that is older is dropped and the application asked afresh.
//!
//! A late answer is the field **as it was when the application answered**, not what a call made
//! at the later question would say: a field that turned into a password field in between, with
//! no focus change, is not in it. The freshness bound keeps "in between" to a quarter of a
//! second, the window of every answer's own race with such a change. It is counted from when
//! the answer arrived ([`Probed::chosen`]): the probe gets the element and its properties in one
//! call (`uia`, "One call into the application"), and the application reads them somewhere
//! inside that call, so the answer is as old as its call took plus that wait. The call is
//! limited too: a late answer whose call took more than [`LATE_CALL_LIMIT`] is not taken, so a
//! late answer is at most half a second old when it is used. A question about another
//! window or control, after another focus change, or that may not take one, drops it: the late
//! answer then goes nowhere, as before. The caller also checks which process provided the
//! element ([`Answer::Late`]).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::classify::{ControlKind, Observation};

/// The most inspection threads at a time: the one in use and those given up that still wait
/// for an application.
pub(crate) const MAX_WORKERS: usize = 4;

/// How long after its element was chosen a late answer may still serve a question (module
/// docs).
pub(crate) const LATE_ANSWER_FRESH: Duration = Duration::from_millis(250);

/// The longest call whose late answer may serve a question (module docs).
pub(crate) const LATE_CALL_LIMIT: Duration = Duration::from_millis(250);

/// What a probe saw, and when it chose the element it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probed {
    /// What the focused element is.
    pub(crate) observation: Observation,
    /// When the system named the focused element (for no element or a failed call: when the
    /// probe returned). The freshness of a late answer is counted from here (module docs).
    pub(crate) chosen: Instant,
    /// How long the calls into the application took (for `--diagnose-focus`).
    pub(crate) took: Duration,
}

/// What the inspection reports to a tracer (`diagnose`): durations and kinds, never content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceEvent {
    /// A probe returned.
    Probed {
        /// What it saw.
        observation: Observation,
        /// How long its calls into the application took.
        took: Duration,
        /// How long after the question was asked the answer arrived.
        after_question: Duration,
        /// The question had stopped waiting: this is a late answer, arriving this long after.
        late_by: Option<Duration>,
    },
    /// A late answer served a question, this long after its element was chosen.
    LateTaken(Duration),
    /// A late answer was refused as too old, this long after its element was chosen.
    LateTooOld(Duration),
    /// A late answer was refused because its call took this long.
    LateTooSlow(Duration),
}

/// Where an inspector reports what it does (`diagnose`).
pub(crate) type Tracer = Arc<dyn Fn(TraceEvent) + Send + Sync>;

/// Asks the system about the focused element. Used on the thread that opened it.
pub(crate) trait Probe {
    /// What the focused element is. May take as long as the application in front likes.
    fn observe(&mut self) -> Probed;
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
    /// The control with the keyboard focus in it (its window handle; the window itself when
    /// the system names none).
    pub(crate) control: u64,
    /// That control's kind.
    pub(crate) kind: ControlKind,
    /// How many focus changes had been counted when the question was asked.
    pub(crate) epoch: u64,
}

/// Called on the inspection thread when the answer of a given-up question arrives.
pub(crate) type Wake = Box<dyn FnOnce() + Send>;

/// Where the inspection thread puts the answer of one question.
#[derive(Default)]
struct Mailbox {
    mail: Mutex<Mail>,
    arrived: Condvar,
}

#[derive(Default)]
struct Mail {
    /// The answer, when its element was chosen, and how long its call took.
    answer: Option<(Observation, Instant, Duration)>,
    /// The thread dropped the question (skipped it, or ended).
    closed: bool,
    /// Called once, when the answer arrives (module docs).
    wake: Option<Wake>,
    /// When the question stopped waiting, if it did before the answer came.
    gave_up: Option<Instant>,
}

/// How a wait for a mailbox ended.
enum Waited {
    Arrived(Observation, Instant, Duration),
    Closed,
    TimedOut,
}

impl Mailbox {
    fn mail(&self) -> MutexGuard<'_, Mail> {
        self.mail.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The inspection thread's side: the answer, then the wake (outside the lock). Returns
    /// when the question stopped waiting, if it had.
    fn deliver(&self, probed: Probed) -> Option<Instant> {
        let (wake, gave_up) = {
            let mut mail = self.mail();
            mail.answer = Some((probed.observation, probed.chosen, probed.took));
            (mail.wake.take(), mail.gave_up)
        };
        self.arrived.notify_all();
        if let Some(wake) = wake {
            wake();
        }
        gave_up
    }

    /// The question stops waiting, unless the answer is there.
    fn give_up(&self) {
        let mut mail = self.mail();
        if mail.answer.is_none() {
            mail.gave_up = Some(Instant::now());
        }
    }

    fn close(&self) {
        self.mail().closed = true;
        self.arrived.notify_all();
    }

    /// Waits for the answer until `deadline`.
    fn wait(&self, deadline: Instant) -> Waited {
        let mut mail = self.mail();
        loop {
            if let Some((observation, at, took)) = mail.answer {
                return Waited::Arrived(observation, at, took);
            }
            if mail.closed {
                return Waited::Closed;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Waited::TimedOut;
            }
            mail = self
                .arrived
                .wait_timeout(mail, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Has `wake` called when the answer arrives; at once if it is already there.
    fn set_wake(&self, wake: Wake) {
        let now = {
            let mut mail = self.mail();
            if mail.answer.is_some() {
                Some(wake)
            } else {
                mail.wake = Some(wake);
                None
            }
        };
        if let Some(wake) = now {
            wake();
        }
    }
}

/// The question's end of a mailbox, held by the job: dropped unanswered, it closes the box.
struct Reply(Arc<Mailbox>);

impl Drop for Reply {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The answer of a given-up thread, still to come or not yet used.
struct Late {
    subject: Subject,
    mailbox: Arc<Mailbox>,
}

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The probe answered in time.
    Observed(Observation),
    /// The answer of an earlier question about the same subject, which arrived after that
    /// question had stopped waiting (module docs). The caller checks that the element is the
    /// window's before it believes it.
    Late(Observation),
    /// It did not.
    TimedOut,
    /// There is no probe: the thread could not be created or could not open one.
    Unavailable,
}

/// One question.
struct Job {
    /// When it was asked.
    asked: Instant,
    /// When the caller stops waiting.
    deadline: Instant,
    reply: Reply,
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
    /// [`LATE_ANSWER_FRESH`], other in tests.
    late_fresh: Duration,
    /// [`LATE_CALL_LIMIT`], other in tests.
    late_call_limit: Duration,
    /// Told what the inspection does, for `--diagnose-focus`; none normally.
    tracer: Arc<Mutex<Option<Tracer>>>,
    /// The subject of the last answer that was a custom element that may be believed at a
    /// second look (`classify`, "A control without a kind").
    custom: Mutex<Option<Subject>>,
}

/// What a question may do with the answer of an earlier one (module docs).
pub(crate) struct Lateness {
    /// What the question is about; `None`: no late answer is kept or taken.
    pub(crate) subject: Option<Subject>,
    /// The question may take a late answer (a report again, not one of a change).
    pub(crate) may_take: bool,
    /// Called when the answer of this question arrives after it stopped waiting.
    pub(crate) wake: Option<Wake>,
}

impl Lateness {
    /// No late answer at all.
    pub(crate) const NONE: Self = Self {
        subject: None,
        may_take: false,
        wake: None,
    };
}

impl<S: ProbeSource> Inspector<S> {
    /// An inspector without a thread; the first question (or [`warm`](Self::warm)) starts
    /// one.
    pub(crate) fn new(source: S) -> Self {
        Self {
            source: Arc::new(source),
            slot: Mutex::new(Slot::default()),
            live: Arc::new(AtomicUsize::new(0)),
            late_fresh: LATE_ANSWER_FRESH,
            late_call_limit: LATE_CALL_LIMIT,
            tracer: Arc::new(Mutex::new(None)),
            custom: Mutex::new(None),
        }
    }

    /// Notes the answer for `subject`: `candidate` when it is a custom element that may be
    /// believed at a second look. True when it is, and the answer before for the same subject
    /// was one too. Any other answer, or another subject, starts again.
    pub(crate) fn custom_again(&self, subject: Option<Subject>, candidate: bool) -> bool {
        let mut seen = self.custom.lock().unwrap_or_else(PoisonError::into_inner);
        let before = *seen;
        *seen = subject.filter(|_| candidate);
        candidate && subject.is_some() && before == subject
    }

    /// Reports from now on what the inspection does to `tracer` (`diagnose`).
    pub(crate) fn set_tracer(&self, tracer: Tracer) {
        *self.tracer.lock().unwrap_or_else(PoisonError::into_inner) = Some(tracer);
    }

    fn trace(&self, event: TraceEvent) {
        let tracer = self
            .tracer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(tracer) = tracer {
            tracer(event);
        }
    }

    /// The same inspector with another limit of a late answer's call.
    #[cfg(test)]
    pub(crate) const fn with_late_call_limit(mut self, limit: Duration) -> Self {
        self.late_call_limit = limit;
        self
    }

    /// The same inspector with another freshness bound of a late answer.
    #[cfg(test)]
    pub(crate) const fn with_late_fresh(mut self, fresh: Duration) -> Self {
        self.late_fresh = fresh;
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

    /// Asks the probe and waits for the answer for at most `timeout`. `lateness` says whether
    /// an answer that comes too late is kept, and whether one kept earlier may serve this
    /// question (module docs).
    pub(crate) fn observe(&self, lateness: Lateness, timeout: Duration) -> Answer {
        let Lateness {
            subject,
            may_take,
            wake,
        } = lateness;
        let now = Instant::now();
        // A limit the clock cannot express is as good as a day.
        let timeout = timeout.min(Duration::from_hours(24));
        let deadline = now + timeout;
        let late = self.slot().late.take();
        if let Some(late) = late
            && may_take
            && Some(late.subject) == subject
        {
            match late.mailbox.wait(deadline) {
                // A call that took too long: the application may have read the field long
                // before it answered. Ask afresh.
                Waited::Arrived(_, _, took) if took > self.late_call_limit => {
                    self.trace(TraceEvent::LateTooSlow(took));
                }
                Waited::Arrived(observation, chosen, _) if chosen.elapsed() <= self.late_fresh => {
                    self.trace(TraceEvent::LateTaken(chosen.elapsed()));
                    return Answer::Late(observation);
                }
                // Too old: the field may have changed since. Ask afresh.
                Waited::Arrived(_, chosen, _) => {
                    self.trace(TraceEvent::LateTooOld(chosen.elapsed()));
                }
                Waited::Closed => {}
                Waited::TimedOut => {
                    // Still not there: the next report again waits for it, and its arrival
                    // calls this question's wake.
                    if let Some(wake) = wake {
                        late.mailbox.set_wake(wake);
                    }
                    self.slot().late = Some(late);
                    return Answer::TimedOut;
                }
            }
        }
        let mailbox = Arc::new(Mailbox::default());
        let job = Job {
            asked: now,
            deadline,
            reply: Reply(Arc::clone(&mailbox)),
        };
        let (id, opened) = match self.submit(job) {
            Ok(worker) => worker,
            Err(answer) => return answer,
        };
        match mailbox.wait(deadline) {
            Waited::Arrived(observation, ..) => Answer::Observed(observation),
            Waited::TimedOut => {
                mailbox.give_up();
                if opened.load(Ordering::SeqCst) {
                    self.give_up(id);
                    if let Some(subject) = subject {
                        if let Some(wake) = wake {
                            mailbox.set_wake(wake);
                        }
                        self.slot().late = Some(Late { subject, mailbox });
                    }
                }
                Answer::TimedOut
            }
            // The thread dropped the question: it skipped it because the time was over, or
            // it ended.
            Waited::Closed if Instant::now() >= deadline => Answer::TimedOut,
            Waited::Closed => {
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
        let tracer = Arc::clone(&self.tracer);
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
                    // A question whose time is over is dropped, which closes its mailbox.
                    if Instant::now() >= job.deadline {
                        continue;
                    }
                    let probed = probe.observe();
                    let gave_up = job.reply.0.deliver(probed);
                    let tracer = tracer
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    if let Some(tracer) = tracer {
                        tracer(TraceEvent::Probed {
                            observation: probed.observation,
                            took: probed.took,
                            after_question: probed.chosen.saturating_duration_since(job.asked),
                            late_by: gave_up.map(|at| probed.chosen.saturating_duration_since(at)),
                        });
                    }
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

    use std::time::Instant;

    use super::{Observation, Probe, ProbeSource, Probed};

    /// What the probe does when asked.
    pub(crate) enum Step {
        /// Answers at once.
        Answer(Observation),
        /// Waits until the test lets go (or drops the sender), then chooses the element and
        /// answers: an application slow before it names its focused element.
        Hang(Receiver<()>, Observation),
        /// Chooses the element at once, then waits until the test lets go before it answers:
        /// an application slow to give the element's properties.
        ChooseThenHang(Receiver<()>, Observation),
    }

    /// A step that chooses the element at once and answers when the test lets go.
    pub(crate) fn choose_then_hang(then: Observation) -> (Step, Sender<()>) {
        let (release, released) = std::sync::mpsc::channel();
        (Step::ChooseThenHang(released, then), release)
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
        fn observe(&mut self) -> Probed {
            *self.0.asked.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            let step = self
                .0
                .steps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            let started = Instant::now();
            let now = |observation| Probed {
                observation,
                chosen: Instant::now(),
                took: started.elapsed(),
            };
            match step {
                Some(Step::Answer(observation)) => now(observation),
                Some(Step::Hang(released, observation)) => {
                    let _ = released.recv();
                    now(observation)
                }
                Some(Step::ChooseThenHang(released, observation)) => {
                    let chosen = Instant::now();
                    let _ = released.recv();
                    Probed {
                        observation,
                        chosen,
                        took: started.elapsed(),
                    }
                }
                None => now(Observation::Failed),
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
        process: 40,
        detail: super::super::classify::Detail::NONE,
    };
    const PASSWORD: Observation = Observation::Element {
        password: true,
        control_type: 50004,
        process: 40,
        detail: super::super::classify::Detail::NONE,
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
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PLAIN)
        );
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
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
        assert_eq!(inspector.observe(Lateness::NONE, SHORT), Answer::TimedOut);
        let waited = started.elapsed();
        assert!(waited >= SHORT, "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");

        // The next question is served by a fresh thread while the first still hangs.
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PLAIN)
        );
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
            assert_eq!(inspector.observe(Lateness::NONE, SHORT), Answer::TimedOut);
        }
        assert_eq!(inspector.workers(), MAX_WORKERS);

        // Every thread hangs: answered at once, without another thread and without asking.
        let started = Instant::now();
        assert_eq!(inspector.observe(Lateness::NONE, LONG), Answer::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(inspector.workers(), MAX_WORKERS);
        assert_eq!(source.0.asked(), u32::try_from(MAX_WORKERS).expect("small"));

        // One returns: there is room again.
        releases.remove(0).send(()).expect("the thread still waits");
        wait_for_workers(&inspector, MAX_WORKERS - 1);
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PASSWORD)
        );
        drop(releases);
        wait_for_workers(&inspector, 1);
    }

    #[test]
    fn a_thread_that_is_still_starting_is_kept() {
        let source = FakeSource::answering(&[PLAIN, PASSWORD]);
        let (open, gate) = mpsc::channel();
        *source.0.open_gate.lock().expect("not poisoned") = Some(gate);
        let inspector = Inspector::new(source.clone());

        assert_eq!(inspector.observe(Lateness::NONE, SHORT), Answer::TimedOut);
        assert_eq!(inspector.workers(), 1);
        open.send(()).expect("the thread waits to open");

        // The same thread serves the next question. The first one's time was over when the
        // thread got to it, so it was skipped: the probe is asked once and its first answer
        // goes to the second question.
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PLAIN)
        );
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
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PLAIN)
        );
        assert_eq!(inspector.workers(), 1);
    }

    #[test]
    fn a_probe_that_cannot_be_opened_is_unavailable_not_normal() {
        let source = FakeSource::answering(&[PLAIN]);
        *source.0.refuse_open.lock().expect("not poisoned") = true;
        let inspector = Inspector::new(source.clone());
        assert_eq!(inspector.observe(Lateness::NONE, LONG), Answer::Unavailable);
        wait_for_workers(&inspector, 0);
        assert_eq!(source.0.asked(), 0);

        // Once the system lets it open, the next question is served.
        *source.0.refuse_open.lock().expect("not poisoned") = false;
        assert_eq!(
            inspector.observe(Lateness::NONE, LONG),
            Answer::Observed(PLAIN)
        );
    }

    #[test]
    fn no_time_is_a_timeout() {
        let source = FakeSource::default();
        let (step, _release) = hang(PLAIN);
        source.0.push(step);
        let inspector = Inspector::new(source);
        assert_eq!(
            inspector.observe(Lateness::NONE, Duration::ZERO),
            Answer::TimedOut
        );
    }

    #[test]
    fn a_limit_too_large_for_the_clock_is_accepted() {
        let inspector = Inspector::new(FakeSource::answering(&[PLAIN]));
        assert_eq!(
            inspector.observe(Lateness::NONE, Duration::MAX),
            Answer::Observed(PLAIN)
        );
    }

    const SUBJECT: Subject = Subject {
        window: 0x1_0000_0A10,
        control: 0x1_0000_0A20,
        kind: ControlKind::WebEngine,
        epoch: 7,
    };

    /// A question about `subject` that keeps a late answer and may take one, with a wake that
    /// says on `woken` when the late answer arrived.
    fn again(subject: Subject, woken: &mpsc::Sender<()>) -> Lateness {
        let woken = woken.clone();
        Lateness {
            subject: Some(subject),
            may_take: true,
            wake: Some(Box::new(move || {
                let _ = woken.send(());
            })),
        }
    }

    /// The first question of a change: keeps a late answer, may not take one.
    fn first(subject: Subject, woken: &mpsc::Sender<()>) -> Lateness {
        Lateness {
            may_take: false,
            ..again(subject, woken)
        }
    }

    #[test]
    fn a_late_answer_wakes_the_watcher_and_serves_the_report_it_makes() {
        let source = FakeSource::default();
        let (step, release) = hang(PASSWORD);
        source.0.push(step);
        source.0.push(Step::Answer(PLAIN));
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();
        let (woken, wakes) = mpsc::channel();

        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        assert!(wakes.try_recv().is_err(), "nothing arrived yet");
        // The application answers after the caller stopped waiting: the watcher is woken at
        // once, and the report it makes takes the answer without asking again.
        release.send(()).expect("the thread still waits");
        wakes
            .recv_timeout(LONG)
            .expect("the arrival wakes the watcher");
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Late(PASSWORD)
        );
        assert_eq!(source.0.asked(), 1);
        // It serves one question: the next one asks.
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Observed(PLAIN)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn a_late_answer_that_waited_too_long_is_not_taken() {
        // Finding M1 of the late-answer review: the field may have become a password field
        // since the application answered.
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone()).with_late_fresh(SHORT);
        inspector.wait_open();
        let (woken, wakes) = mpsc::channel();
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        release.send(()).expect("the thread still waits");
        wakes.recv_timeout(LONG).expect("woken");
        // The report comes after the bound, counted from when the element was chosen (here, at
        // the arrival).
        std::thread::sleep(SHORT * 3);
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn the_bound_counts_from_when_the_element_was_chosen() {
        // The re-verification's 6.3 a: the application named its focused element at once and
        // then took long over the element's properties. The answer arrives just now, but the
        // element is as old as the wait: not taken, the application is asked afresh.
        let source = FakeSource::default();
        let (step, release) = super::fake::choose_then_hang(PLAIN);
        source.0.push(step);
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone()).with_late_fresh(SHORT * 3);
        inspector.wait_open();
        let (woken, wakes) = mpsc::channel();
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        std::thread::sleep(SHORT * 4);
        release.send(()).expect("the thread still waits");
        wakes
            .recv_timeout(LONG)
            .expect("the arrival wakes the watcher");
        // Taken at once after the arrival, so only the element's age can refuse it.
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn the_bound_counts_from_the_choice_not_from_the_question() {
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        let inspector = Inspector::new(source.clone()).with_late_fresh(SHORT * 3);
        inspector.wait_open();
        let (woken, _wakes) = mpsc::channel();
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        // Long after the question, the element is chosen and the answer arrives while a report
        // again waits for it.
        std::thread::sleep(SHORT * 4);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(SHORT);
            release.send(()).expect("the thread still waits");
        });
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Late(PLAIN)
        );
        releaser.join().expect("released");
        assert_eq!(source.0.asked(), 1);
    }

    #[test]
    fn a_slow_application_is_waited_for_and_not_asked_again_and_again() {
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();
        let (woken, _wakes) = mpsc::channel();

        // Three questions while the first call is still running: one call, one thread.
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        for _ in 0..2 {
            assert_eq!(
                inspector.observe(again(SUBJECT, &woken), SHORT),
                Answer::TimedOut
            );
        }
        assert_eq!(source.0.asked(), 1);
        assert_eq!(inspector.workers(), 1);
        release.send(()).expect("the thread still waits");
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Late(PLAIN)
        );
        wait_for_workers(&inspector, 0);
    }

    #[test]
    fn a_late_answer_never_serves_another_window_control_or_count_or_a_change() {
        let others = [
            Subject {
                window: 0x1_0000_0B20,
                ..SUBJECT
            },
            // Finding L2 of the late-answer review: the focus moved to another control of
            // the same window before the change was counted.
            Subject {
                control: 0x1_0000_0C30,
                ..SUBJECT
            },
            Subject {
                kind: ControlKind::Other,
                ..SUBJECT
            },
            Subject {
                epoch: SUBJECT.epoch + 1,
                ..SUBJECT
            },
        ];
        let (woken, _wakes) = mpsc::channel();
        let cases = others
            .iter()
            .map(|other| again(*other, &woken))
            .chain([first(SUBJECT, &woken), Lateness::NONE]);
        for later in cases {
            let label = format!("{:?} may take: {}", later.subject, later.may_take);
            let source = FakeSource::default();
            let (step, release) = hang(PLAIN);
            source.0.push(step);
            source.0.push(Step::Answer(PASSWORD));
            let inspector = Inspector::new(source.clone());
            inspector.wait_open();
            assert_eq!(
                inspector.observe(first(SUBJECT, &woken), SHORT),
                Answer::TimedOut
            );
            release.send(()).expect("the thread still waits");
            // The late "plain" is not taken: the application is asked again, and says
            // "password".
            assert_eq!(
                inspector.observe(later, LONG),
                Answer::Observed(PASSWORD),
                "{label}"
            );
            assert_eq!(source.0.asked(), 2, "{label}");
            // And it is gone: not even the first subject gets it now.
            source.0.push(Step::Answer(PASSWORD));
            assert_eq!(
                inspector.observe(again(SUBJECT, &woken), LONG),
                Answer::Observed(PASSWORD),
                "{label}"
            );
        }
    }

    #[test]
    fn a_question_without_a_subject_leaves_no_late_answer() {
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone());
        inspector.wait_open();
        assert_eq!(inspector.observe(Lateness::NONE, SHORT), Answer::TimedOut);
        release.send(()).expect("the thread still waits");
        let (woken, wakes) = mpsc::channel();
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 2);
        assert!(wakes.try_recv().is_err(), "no wake for an answer in time");
    }

    #[test]
    fn a_late_answer_whose_call_took_too_long_is_not_taken() {
        // The cached-probe review's L3: the application may have read the field early in a
        // long call, so the answer is as old as the call, however new it looks.
        let source = FakeSource::default();
        let (step, release) = hang(PLAIN);
        source.0.push(step);
        source.0.push(Step::Answer(PASSWORD));
        let inspector = Inspector::new(source.clone()).with_late_call_limit(SHORT * 2);
        inspector.wait_open();
        let (woken, wakes) = mpsc::channel();
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        std::thread::sleep(SHORT * 3);
        release.send(()).expect("the thread still waits");
        wakes
            .recv_timeout(LONG)
            .expect("the arrival wakes the watcher");
        // Fresh by its arrival, but its call took four times the limit: asked afresh.
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Observed(PASSWORD)
        );
        assert_eq!(source.0.asked(), 2);
    }

    #[test]
    fn a_tracer_hears_each_answer_and_whether_it_came_late() {
        let source = FakeSource::default();
        source.0.push(Step::Answer(PLAIN));
        let (step, release) = hang(PASSWORD);
        source.0.push(step);
        let inspector = Inspector::new(source.clone());
        let (heard, events) = mpsc::channel();
        inspector.set_tracer(Arc::new(move |event| {
            let _ = heard.send(event);
        }));
        inspector.wait_open();
        let (woken, wakes) = mpsc::channel();

        // In time: no `late_by`.
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), LONG),
            Answer::Observed(PLAIN)
        );
        match events.recv_timeout(LONG).expect("traced") {
            TraceEvent::Probed {
                observation,
                late_by,
                ..
            } => {
                assert_eq!(observation, PLAIN);
                assert_eq!(late_by, None);
            }
            other => panic!("{other:?}"),
        }

        // Late: `late_by` is set when it arrives, and its use is traced too.
        assert_eq!(
            inspector.observe(first(SUBJECT, &woken), SHORT),
            Answer::TimedOut
        );
        release.send(()).expect("the thread still waits");
        wakes.recv_timeout(LONG).expect("woken");
        match events.recv_timeout(LONG).expect("traced") {
            TraceEvent::Probed {
                observation,
                late_by,
                after_question,
                ..
            } => {
                assert_eq!(observation, PASSWORD);
                assert!(late_by.is_some());
                assert!(after_question >= SHORT, "{after_question:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            inspector.observe(again(SUBJECT, &woken), LONG),
            Answer::Late(PASSWORD)
        );
        assert!(matches!(
            events.recv_timeout(LONG).expect("traced"),
            TraceEvent::LateTaken(_)
        ));
    }

    #[test]
    fn the_debug_form_shows_a_count_only() {
        let inspector = Inspector::new(FakeSource::default());
        assert_eq!(format!("{inspector:?}"), "Inspector { workers: 0, .. }");
    }
}
