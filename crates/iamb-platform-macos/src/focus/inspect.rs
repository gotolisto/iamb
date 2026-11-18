//! The secure-field answer: what is asked in which order, and the thread that asks. Plain
//! Rust, built and tested on every system; the system calls sit behind [`Probe`].
//!
//! # The answer
//!
//! [`inspect`] answers for one window, the target of the focus change being judged:
//!
//! 1. Another window has the focus: `Unknown(FocusMoved)`; no application is frontmost:
//!    `Unknown(NoFocus)`.
//! 2. Secure keyboard entry is on: `Secure`. The application is not asked anything.
//! 3. Otherwise the focused element is read through the Accessibility API: `Secure` for a
//!    secure text field, `Unknown` for every failure.
//! 4. Before `Normal` is answered the secure-entry flag is read once more, so a password
//!    field that took the focus while the element was being read is not missed.
//! 5. The focus is checked again; if it moved during the inspection the answer is
//!    `Unknown(FocusMoved)`, never the state of the window that had it before.
//!
//! When the flag cannot be read in time the answer is `Unknown(Timeout)`.
//!
//! # The thread
//!
//! The Accessibility read is a message to the focused application, and an application that
//! hangs does not answer it. [`Inspector`] therefore runs every inspection on a thread of its
//! own, and the caller waits for the answer at most its timeout: after that it gets
//! `Unknown(Timeout)` and the late answer is thrown away. The inspection itself is bounded
//! too (the probe gives every message to the application a timeout that ends at the same
//! deadline), so the thread comes free again. A request whose deadline has passed before the
//! thread gets to it is not started, and at most [`QUEUE`] requests wait: a caller that finds
//! the queue full is answered `Unknown(Timeout)` at once.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use iamb_platform::{FieldSecurity, FocusToken, UnknownReason};

/// How many inspections may wait for the thread.
pub(crate) const QUEUE: usize = 4;

/// The system as the inspection sees it.
pub(crate) trait Probe {
    /// The window that has the focus now: the frontmost application and its front window.
    /// `None` when no application is frontmost. Asks no application anything.
    fn front(&self) -> Option<FocusToken>;

    /// Whether secure keyboard entry is on in the login session. `None` when that could not
    /// be found out before `deadline`.
    fn secure_input(&self, deadline: Instant) -> Option<bool>;

    /// The state of the focused element of application `pid`, read through the Accessibility
    /// API. Every message to the application must give up at `deadline`.
    fn focused_field(&self, pid: u32, deadline: Instant) -> FieldSecurity;
}

/// Why the answer cannot be about `target`, if the focus is elsewhere.
fn moved(probe: &dyn Probe, target: FocusToken) -> Option<FieldSecurity> {
    match probe.front() {
        None => Some(FieldSecurity::Unknown(UnknownReason::NoFocus)),
        Some(front) if front != target => Some(FieldSecurity::Unknown(UnknownReason::FocusMoved)),
        Some(_) => None,
    }
}

/// Whether the focused field of window `target` is secure. See the module docs for the order.
pub(crate) fn inspect(probe: &dyn Probe, target: FocusToken, deadline: Instant) -> FieldSecurity {
    const TIMEOUT: FieldSecurity = FieldSecurity::Unknown(UnknownReason::Timeout);
    if let Some(elsewhere) = moved(probe, target) {
        return elsewhere;
    }
    let field = match probe.secure_input(deadline) {
        None => TIMEOUT,
        Some(true) => FieldSecurity::Secure,
        Some(false) => match probe.focused_field(target.pid(), deadline) {
            FieldSecurity::Normal => match probe.secure_input(deadline) {
                Some(false) => FieldSecurity::Normal,
                Some(true) => FieldSecurity::Secure,
                None => TIMEOUT,
            },
            other => other,
        },
    };
    moved(probe, target).unwrap_or(field)
}

/// One question for the inspection thread.
struct Request {
    target: FocusToken,
    deadline: Instant,
    reply: SyncSender<FieldSecurity>,
}

/// The inspection thread. Dropping it ends the thread once its current inspection is over.
pub(crate) struct Inspector {
    requests: SyncSender<Request>,
}

impl Inspector {
    /// Starts the thread, which asks `probe`.
    ///
    /// # Errors
    ///
    /// The error of the thread's creation.
    pub(crate) fn spawn(probe: impl Probe + Send + 'static) -> std::io::Result<Self> {
        let (requests, queue) = sync_channel(QUEUE);
        thread::Builder::new()
            .name("iamb-focus-inspect".to_owned())
            .spawn(move || serve(&probe, &queue))?;
        Ok(Self { requests })
    }

    /// Queues one question; the answer arrives on the returned channel, unless the thread
    /// finds the deadline passed. The error is the answer when the question cannot be queued.
    fn submit(
        &self,
        target: FocusToken,
        deadline: Instant,
    ) -> Result<Receiver<FieldSecurity>, FieldSecurity> {
        let (reply, answer) = sync_channel(1);
        let request = Request {
            target,
            deadline,
            reply,
        };
        match self.requests.try_send(request) {
            Ok(()) => Ok(answer),
            // The thread is still busy with earlier inspections: this one would not be
            // answered in time either.
            Err(TrySendError::Full(_)) => Err(FieldSecurity::Unknown(UnknownReason::Timeout)),
            Err(TrySendError::Disconnected(_)) => {
                Err(FieldSecurity::Unknown(UnknownReason::NotInspectable))
            }
        }
    }

    /// Whether the focused field of window `target` is secure, waiting at most `timeout`.
    pub(crate) fn ask(&self, target: FocusToken, timeout: Duration) -> FieldSecurity {
        match self.submit(target, Instant::now() + timeout) {
            // No answer in time, or the thread dropped the request because its deadline had
            // passed: both are a timeout.
            Ok(answer) => answer
                .recv_timeout(timeout)
                .unwrap_or(FieldSecurity::Unknown(UnknownReason::Timeout)),
            Err(refused) => refused,
        }
    }
}

/// The inspection thread: one request at a time, until the [`Inspector`] is dropped.
fn serve(probe: &dyn Probe, queue: &Receiver<Request>) {
    while let Ok(request) = queue.recv() {
        if Instant::now() >= request.deadline {
            // The caller has given up; it must not make later callers wait.
            continue;
        }
        let answer = catch_unwind(AssertUnwindSafe(|| {
            inspect(probe, request.target, request.deadline)
        }))
        .unwrap_or(FieldSecurity::Unknown(UnknownReason::NotInspectable));
        // The caller may have stopped waiting; then nobody hears the answer.
        let _ = request.reply.try_send(answer);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;

    /// A system a test scripts: each read takes its next scripted answer and repeats the
    /// last one. Counts the Accessibility reads.
    pub(crate) struct FakeProbe {
        pub(crate) front: Mutex<VecDeque<Option<FocusToken>>>,
        pub(crate) secure_input: Mutex<VecDeque<Option<bool>>>,
        pub(crate) field: Mutex<VecDeque<FieldSecurity>>,
        /// How long one Accessibility read takes.
        pub(crate) field_delay: Duration,
        pub(crate) field_reads: Arc<AtomicUsize>,
    }

    fn next<T: Clone>(script: &Mutex<VecDeque<T>>) -> T {
        let mut script = script.lock().unwrap_or_else(PoisonError::into_inner);
        if script.len() > 1 {
            script.pop_front().expect("more than one answer")
        } else {
            script.front().cloned().expect("a scripted answer")
        }
    }

    impl FakeProbe {
        /// An ordinary field in `window`, with secure keyboard entry off.
        pub(crate) fn normal(window: FocusToken) -> Self {
            Self {
                front: Mutex::new([Some(window)].into()),
                secure_input: Mutex::new([Some(false)].into()),
                field: Mutex::new([FieldSecurity::Normal].into()),
                field_delay: Duration::ZERO,
                field_reads: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub(crate) fn front(self, script: impl Into<VecDeque<Option<FocusToken>>>) -> Self {
            *self.front.lock().expect("the script") = script.into();
            self
        }

        pub(crate) fn secure_input(self, script: impl Into<VecDeque<Option<bool>>>) -> Self {
            *self.secure_input.lock().expect("the script") = script.into();
            self
        }

        pub(crate) fn field(self, script: impl Into<VecDeque<FieldSecurity>>) -> Self {
            *self.field.lock().expect("the script") = script.into();
            self
        }

        pub(crate) fn field_delay(mut self, delay: Duration) -> Self {
            self.field_delay = delay;
            self
        }

        pub(crate) fn field_reads(&self) -> usize {
            self.field_reads.load(Ordering::SeqCst)
        }
    }

    impl Probe for FakeProbe {
        fn front(&self) -> Option<FocusToken> {
            next(&self.front)
        }

        fn secure_input(&self, _deadline: Instant) -> Option<bool> {
            next(&self.secure_input)
        }

        fn focused_field(&self, _pid: u32, _deadline: Instant) -> FieldSecurity {
            self.field_reads.fetch_add(1, Ordering::SeqCst);
            thread::sleep(self.field_delay);
            next(&self.field)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::testing::FakeProbe;
    use super::*;

    const WINDOW: FocusToken = FocusToken::from_raw(42, 60);
    const OTHER: FocusToken = FocusToken::from_raw(42, 61);
    const SECURE: FieldSecurity = FieldSecurity::Secure;
    const NORMAL: FieldSecurity = FieldSecurity::Normal;

    const fn unknown(reason: UnknownReason) -> FieldSecurity {
        FieldSecurity::Unknown(reason)
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[test]
    fn an_ordinary_field_is_normal() {
        let probe = FakeProbe::normal(WINDOW);
        assert_eq!(inspect(&probe, WINDOW, soon()), NORMAL);
        assert_eq!(probe.field_reads(), 1);
    }

    #[test]
    fn secure_keyboard_entry_is_secure_and_the_application_is_not_asked() {
        let probe = FakeProbe::normal(WINDOW).secure_input([Some(true)]);
        assert_eq!(inspect(&probe, WINDOW, soon()), SECURE);
        assert_eq!(probe.field_reads(), 0);
    }

    #[test]
    fn a_secure_text_field_is_secure_without_secure_keyboard_entry() {
        let probe = FakeProbe::normal(WINDOW).field([SECURE]);
        assert_eq!(inspect(&probe, WINDOW, soon()), SECURE);
    }

    #[test]
    fn secure_keyboard_entry_that_starts_during_the_read_is_secure() {
        // Off before the element is read, on after: a password field took the focus.
        let probe = FakeProbe::normal(WINDOW).secure_input([Some(false), Some(true)]);
        assert_eq!(inspect(&probe, WINDOW, soon()), SECURE);
    }

    #[test]
    fn an_unreadable_element_is_unknown() {
        for reason in [UnknownReason::NotInspectable, UnknownReason::Timeout] {
            let probe = FakeProbe::normal(WINDOW).field([unknown(reason)]);
            assert_eq!(inspect(&probe, WINDOW, soon()), unknown(reason));
        }
    }

    #[test]
    fn a_flag_that_cannot_be_read_is_a_timeout_never_normal() {
        let probe = FakeProbe::normal(WINDOW).secure_input([None]);
        assert_eq!(
            inspect(&probe, WINDOW, soon()),
            unknown(UnknownReason::Timeout)
        );
        assert_eq!(probe.field_reads(), 0);
        // Readable before the element was read, not after.
        let probe = FakeProbe::normal(WINDOW).secure_input([Some(false), None]);
        assert_eq!(
            inspect(&probe, WINDOW, soon()),
            unknown(UnknownReason::Timeout)
        );
    }

    #[test]
    fn another_window_in_front_is_focus_moved_and_nothing_is_read() {
        let probe = FakeProbe::normal(OTHER);
        assert_eq!(
            inspect(&probe, WINDOW, soon()),
            unknown(UnknownReason::FocusMoved)
        );
        assert_eq!(probe.field_reads(), 0);
        // Another application.
        let probe = FakeProbe::normal(FocusToken::from_raw(7, 60));
        assert_eq!(
            inspect(&probe, WINDOW, soon()),
            unknown(UnknownReason::FocusMoved)
        );
    }

    #[test]
    fn no_frontmost_application_is_no_focus() {
        let probe = FakeProbe::normal(WINDOW).front([None]);
        assert_eq!(
            inspect(&probe, WINDOW, soon()),
            unknown(UnknownReason::NoFocus)
        );
        assert_eq!(probe.field_reads(), 0);
    }

    #[test]
    fn a_focus_that_moves_during_the_inspection_is_focus_moved_whatever_was_read() {
        for field in [NORMAL, SECURE, unknown(UnknownReason::NotInspectable)] {
            let probe = FakeProbe::normal(WINDOW)
                .front([Some(WINDOW), Some(OTHER)])
                .field([field]);
            assert_eq!(
                inspect(&probe, WINDOW, soon()),
                unknown(UnknownReason::FocusMoved)
            );
        }
        let probe = FakeProbe::normal(WINDOW).front([Some(WINDOW), None]);
        assert_eq!(
            inspect(&probe, WINDOW, soon()),
            unknown(UnknownReason::NoFocus)
        );
    }

    #[test]
    fn the_inspector_answers_from_its_own_thread() {
        let inspector = Inspector::spawn(FakeProbe::normal(WINDOW)).expect("the thread");
        assert_eq!(inspector.ask(WINDOW, Duration::from_secs(10)), NORMAL);
        assert_eq!(
            inspector.ask(OTHER, Duration::from_secs(10)),
            unknown(UnknownReason::FocusMoved)
        );
    }

    #[test]
    fn an_application_that_does_not_answer_cannot_hold_the_caller() {
        let probe = FakeProbe::normal(WINDOW).field_delay(Duration::from_millis(400));
        let inspector = Inspector::spawn(probe).expect("the thread");
        let started = Instant::now();
        assert_eq!(
            inspector.ask(WINDOW, Duration::from_millis(30)),
            unknown(UnknownReason::Timeout)
        );
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(30), "{waited:?}");
        assert!(waited < Duration::from_millis(300), "{waited:?}");
    }

    /// Waits until the inspection thread has started its `n`th Accessibility read.
    fn reading(reads: &std::sync::atomic::AtomicUsize, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while reads.load(std::sync::atomic::Ordering::SeqCst) < n {
            assert!(
                Instant::now() < deadline,
                "the thread never got to read {n}"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_request_the_caller_gave_up_on_is_not_started() {
        let probe = FakeProbe::normal(WINDOW).field_delay(Duration::from_millis(200));
        let reads = Arc::clone(&probe.field_reads);
        let inspector = Inspector::spawn(probe).expect("the thread");
        // The first holds the thread.
        let first = inspector
            .submit(WINDOW, Instant::now() + Duration::from_secs(10))
            .expect("queued");
        reading(&reads, 1);
        // The next two time out while they wait in the queue.
        for _ in 0..2 {
            assert_eq!(
                inspector.ask(WINDOW, Duration::from_millis(20)),
                unknown(UnknownReason::Timeout)
            );
        }
        // Once the thread is free, a patient caller is answered, and the two stale requests
        // were dropped without asking the application.
        assert_eq!(inspector.ask(WINDOW, Duration::from_secs(10)), NORMAL);
        assert_eq!(first.recv().expect("the first answer"), NORMAL);
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn a_full_queue_is_a_timeout_at_once() {
        let probe = FakeProbe::normal(WINDOW).field_delay(Duration::from_millis(150));
        let reads = Arc::clone(&probe.field_reads);
        let inspector = Inspector::spawn(probe).expect("the thread");
        let later = Instant::now() + Duration::from_secs(30);
        // One inspection in progress, then a full queue behind it.
        let mut waiting = vec![inspector.submit(WINDOW, later).expect("queued")];
        reading(&reads, 1);
        waiting.extend((0..QUEUE).map(|_| inspector.submit(WINDOW, later).expect("queued")));
        let started = Instant::now();
        assert_eq!(
            inspector.ask(WINDOW, Duration::from_secs(30)),
            unknown(UnknownReason::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        for answer in waiting {
            assert_eq!(answer.recv().expect("an answer"), NORMAL);
        }
    }

    #[test]
    fn a_zero_timeout_is_a_timeout() {
        let inspector = Inspector::spawn(FakeProbe::normal(WINDOW)).expect("the thread");
        assert_eq!(
            inspector.ask(WINDOW, Duration::ZERO),
            unknown(UnknownReason::Timeout)
        );
    }

    #[test]
    fn a_probe_that_panics_is_unknown_and_the_thread_lives_on() {
        struct Panics;
        impl Probe for Panics {
            fn front(&self) -> Option<FocusToken> {
                Some(WINDOW)
            }
            fn secure_input(&self, _deadline: Instant) -> Option<bool> {
                Some(false)
            }
            fn focused_field(&self, pid: u32, _deadline: Instant) -> FieldSecurity {
                assert_ne!(pid, WINDOW.pid(), "the scripted failure");
                FieldSecurity::Normal
            }
        }
        let inspector = Inspector::spawn(Panics).expect("the thread");
        assert_eq!(
            inspector.ask(WINDOW, Duration::from_secs(10)),
            unknown(UnknownReason::NotInspectable)
        );
        assert_eq!(
            inspector.ask(WINDOW, Duration::from_secs(10)),
            unknown(UnknownReason::NotInspectable)
        );
    }
}
