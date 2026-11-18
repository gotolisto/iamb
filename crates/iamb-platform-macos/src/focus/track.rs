//! What the focus watcher reports, and when it looks again. Plain Rust, built and tested on
//! every system; the watcher thread (`watcher`, macOS) feeds it what the system said.
//!
//! # Why not every notification is a report
//!
//! Each report makes the engine forget its buffer, suspend, and wait for a new verdict, and
//! it cancels an expansion whose typed characters are still being collected. So a report
//! must mean that the focus changed; a second report for the same change, a few
//! milliseconds after the first, could cancel the expansion the first one allowed.
//! Applications send focus notifications in bursts (the window, then the element in it) and
//! some repeat them for an element that already has the focus. [`Tracker`] therefore reports
//!
//! | Signal | Reported when |
//! | --- | --- |
//! | an application was activated | always |
//! | the Accessibility observer was attached late (see below) | always: changes may have been missed |
//! | the focused or main window of the application changed | the application or the window differs from the last report |
//! | the focused element changed | the element differs from the last one known, or the window does |
//! | a timer | the application or the window differs from the last report |
//!
//! and, for every signal, when secure keyboard entry was switched on or off since the last
//! look.
//!
//! Missing a change would be the worse mistake (a password field taken for the name field
//! before it), so whatever is not known is treated as changed: after a report that was not
//! caused by an element notification the focused element is unknown until the watcher has
//! read it ([`Tracker::learn_element`]), and the first element notification for an unknown
//! element is reported.
//!
//! # Looking again
//!
//! - **Settling.** The window server's window order can trail a notification by a moment,
//!   and an application that is starting shows its window after it was activated. After
//!   every signal the tracker asks for two more looks ([`SETTLE`]).
//! - **Secure keyboard entry.** The system announces neither its start nor its end. Its
//!   start needs no announcement: while it is on the event tap sees no key at all. Its end
//!   does, because a verdict of "secure" would otherwise outlive the password field in an
//!   application that sends no focus notification. While the flag is on the tracker asks for
//!   a look every [`SECURE_INPUT_POLL`]; while it is off it asks for none.
//! - **The observer.** An application that is starting, or busy, may not take the
//!   Accessibility registration. [`Attachment`] retries it every [`ATTACH_RETRY`], at most
//!   [`ATTACH_ATTEMPTS`] times per activation.
//!
//! When nothing changes and secure keyboard entry is off, no timer is pending and the
//! watcher thread sleeps.

use std::time::{Duration, Instant};

use iamb_platform::FocusChange;

/// When the tracker looks again after a signal.
pub(crate) const SETTLE: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(500)];

/// How often the tracker looks while secure keyboard entry is on.
pub(crate) const SECURE_INPUT_POLL: Duration = Duration::from_millis(500);

/// How long after a failed registration the next one is tried.
pub(crate) const ATTACH_RETRY: Duration = Duration::from_secs(1);

/// How many registrations are tried for one activation of an application.
pub(crate) const ATTACH_ATTEMPTS: u8 = 5;

/// What the watcher saw when it looked.
pub(crate) struct Seen {
    /// The frontmost application and its front window; `None` when no application is
    /// frontmost.
    pub(crate) change: Option<FocusChange>,
    /// Whether secure keyboard entry is on.
    pub(crate) secure_input: bool,
}

/// Why the watcher looked. `E` identifies a focused element; two values are equal when they
/// name the same element.
pub(crate) enum Signal<E> {
    /// The system said that an application was activated. Also the first look of a watcher.
    Activated,
    /// The application said that its focused or main window changed.
    Window,
    /// The application said that this element now has the keyboard focus.
    Element(E),
    /// The Accessibility observer was attached on a retry.
    Attached,
    /// A timer the tracker asked for.
    Timer,
}

/// Decides what is reported to the focus sink. See the module docs.
pub(crate) struct Tracker<E> {
    reported: Option<FocusChange>,
    element: Option<E>,
    secure_input: bool,
    settle: [Option<Instant>; SETTLE.len()],
    poll: Option<Instant>,
}

impl<E: PartialEq> Tracker<E> {
    /// A tracker that has reported nothing.
    pub(crate) const fn new() -> Self {
        Self {
            reported: None,
            element: None,
            secure_input: false,
            settle: [None; SETTLE.len()],
            poll: None,
        }
    }

    /// Takes one look: `seen` is what the system showed at `now`, `signal` why the watcher
    /// looked. Returns the change to report, if any.
    pub(crate) fn observe(
        &mut self,
        signal: Signal<E>,
        now: Instant,
        seen: Seen,
    ) -> Option<FocusChange> {
        let moved = seen.change != self.reported;
        let flag_changed = seen.secure_input != self.secure_input;
        self.secure_input = seen.secure_input;
        self.poll = seen.secure_input.then(|| now + SECURE_INPUT_POLL);

        if matches!(signal, Signal::Timer) {
            for at in &mut self.settle {
                if at.is_some_and(|at| at <= now) {
                    *at = None;
                }
            }
        } else {
            self.settle = SETTLE.map(|after| Some(now + after));
        }

        let (always, named) = match signal {
            Signal::Activated | Signal::Attached => (true, None),
            Signal::Window | Signal::Timer => (false, None),
            Signal::Element(element) => (false, Some(element)),
        };
        let refocused = named
            .as_ref()
            .is_some_and(|element| self.element.as_ref() != Some(element));
        match named {
            Some(element) => self.element = Some(element),
            // Which element has the focus in the application or window seen now is not
            // known until the watcher has read it.
            None if always || moved => self.element = None,
            None => {}
        }
        if !(always || moved || flag_changed || refocused) {
            return None;
        }
        self.reported.clone_from(&seen.change);
        seen.change
    }

    /// Whether the focused element of what was last reported is still unknown.
    pub(crate) const fn wants_element(&self) -> bool {
        self.element.is_none() && self.reported.is_some()
    }

    /// Takes the focused element the watcher read after a report. `None` (the read failed)
    /// leaves it unknown.
    pub(crate) fn learn_element(&mut self, element: Option<E>) {
        if self.element.is_none() {
            self.element = element;
        }
    }

    /// When the tracker wants its next [`Signal::Timer`].
    pub(crate) fn next_timer(&self) -> Option<Instant> {
        self.settle
            .iter()
            .copied()
            .chain([self.poll])
            .flatten()
            .min()
    }

    /// Whether a timer is due at `now`.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.next_timer().is_some_and(|at| at <= now)
    }
}

/// What to do about the Accessibility observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing.
    Keep,
    /// Drop the observer: no application is frontmost.
    Detach,
    /// Drop the observer, if any, and register one with application `pid`; then call
    /// [`Attachment::attached`].
    Attach {
        /// The frontmost application.
        pid: u32,
    },
}

/// Keeps one Accessibility observer on the frontmost application. See the module docs.
#[derive(Debug, Default)]
pub(crate) struct Attachment {
    pid: Option<u32>,
    attached: bool,
    attempts: u8,
    retry: Option<Instant>,
}

impl Attachment {
    /// What to do now that application `front` is frontmost.
    pub(crate) fn step(&mut self, front: Option<u32>, now: Instant) -> Step {
        if front != self.pid {
            *self = Self {
                pid: front,
                ..Self::default()
            };
            return front.map_or(Step::Detach, |pid| Step::Attach { pid });
        }
        match self.pid {
            Some(pid) if !self.attached && self.retry.is_some_and(|at| at <= now) => {
                Step::Attach { pid }
            }
            _ => Step::Keep,
        }
    }

    /// Takes the outcome of the registration [`step`](Self::step) asked for. Returns `true`
    /// when a retry succeeded: the application was not observed since its activation, so
    /// the watcher must look again ([`Signal::Attached`]).
    pub(crate) fn attached(&mut self, registered: bool, now: Instant) -> bool {
        let retried = self.attempts > 0;
        self.attempts = self.attempts.saturating_add(1);
        self.attached = registered;
        self.retry = (!registered && self.attempts < ATTACH_ATTEMPTS).then(|| now + ATTACH_RETRY);
        registered && retried
    }

    /// When the next registration is to be tried.
    pub(crate) const fn next_timer(&self) -> Option<Instant> {
        self.retry
    }

    /// Whether a retry is due at `now`.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.retry.is_some_and(|at| at <= now)
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::{AppIdentity, FocusToken};

    use super::*;

    fn window(pid: u32, window: u64) -> FocusChange {
        FocusChange {
            app: AppIdentity::new(format!("app{pid}"), pid),
            token: FocusToken::from_raw(pid, window),
        }
    }

    fn seen(change: &FocusChange) -> Seen {
        Seen {
            change: Some(change.clone()),
            secure_input: false,
        }
    }

    fn secure(change: &FocusChange) -> Seen {
        Seen {
            change: Some(change.clone()),
            secure_input: true,
        }
    }

    /// A tracker that has reported `change` and learned its focused element.
    fn tracking(change: &FocusChange, element: u32, now: Instant) -> Tracker<u32> {
        let mut tracker = Tracker::new();
        assert_eq!(
            tracker.observe(Signal::Activated, now, seen(change)),
            Some(change.clone())
        );
        assert!(tracker.wants_element());
        tracker.learn_element(Some(element));
        assert!(!tracker.wants_element());
        tracker
    }

    #[test]
    fn the_first_look_reports_the_current_focus() {
        let now = Instant::now();
        let editor = window(1, 10);
        let mut tracker = Tracker::<u32>::new();
        assert_eq!(
            tracker.observe(Signal::Activated, now, seen(&editor)),
            Some(editor)
        );
    }

    #[test]
    fn with_nothing_in_front_nothing_is_reported() {
        let now = Instant::now();
        let mut tracker = Tracker::<u32>::new();
        let nothing = Seen {
            change: None,
            secure_input: false,
        };
        assert_eq!(tracker.observe(Signal::Activated, now, nothing), None);
        assert!(!tracker.wants_element());
    }

    #[test]
    fn an_activation_is_always_reported() {
        let now = Instant::now();
        let editor = window(1, 10);
        let mut tracker = tracking(&editor, 5, now);
        // Even for what was reported last: another application had the focus in between.
        assert_eq!(
            tracker.observe(Signal::Activated, now, seen(&editor)),
            Some(editor.clone())
        );
        let browser = window(2, 20);
        assert_eq!(
            tracker.observe(Signal::Activated, now, seen(&browser)),
            Some(browser)
        );
    }

    #[test]
    fn a_window_notification_is_reported_only_for_another_window() {
        let now = Instant::now();
        let editor = window(1, 10);
        let mut tracker = tracking(&editor, 5, now);
        assert_eq!(tracker.observe(Signal::Window, now, seen(&editor)), None);
        let second = window(1, 11);
        assert_eq!(
            tracker.observe(Signal::Window, now, seen(&second)),
            Some(second.clone())
        );
        assert_eq!(tracker.observe(Signal::Window, now, seen(&second)), None);
    }

    #[test]
    fn a_move_to_another_field_is_reported_once() {
        let now = Instant::now();
        let form = window(1, 10);
        let mut tracker = tracking(&form, 5, now);
        // From the name field (5) to the password field (6).
        assert_eq!(
            tracker.observe(Signal::Element(6), now, seen(&form)),
            Some(form.clone())
        );
        // The application says it again: nothing changed.
        assert_eq!(tracker.observe(Signal::Element(6), now, seen(&form)), None);
        // And back.
        assert_eq!(
            tracker.observe(Signal::Element(5), now, seen(&form)),
            Some(form)
        );
    }

    #[test]
    fn a_notification_for_the_element_that_already_has_the_focus_is_not_reported() {
        let now = Instant::now();
        let form = window(1, 10);
        let mut tracker = tracking(&form, 5, now);
        for _ in 0..3 {
            assert_eq!(tracker.observe(Signal::Element(5), now, seen(&form)), None);
        }
    }

    #[test]
    fn the_burst_after_a_window_change_is_one_report() {
        let now = Instant::now();
        let first = window(1, 10);
        let second = window(1, 11);
        let mut tracker = tracking(&first, 5, now);
        // The window, then the element in it; the watcher read the element in between.
        assert_eq!(
            tracker.observe(Signal::Window, now, seen(&second)),
            Some(second.clone())
        );
        tracker.learn_element(Some(8));
        assert_eq!(
            tracker.observe(Signal::Element(8), now, seen(&second)),
            None
        );
    }

    #[test]
    fn an_element_notification_is_reported_while_the_element_is_unknown() {
        let now = Instant::now();
        let form = window(1, 10);
        let mut tracker = Tracker::<u32>::new();
        assert!(
            tracker
                .observe(Signal::Activated, now, seen(&form))
                .is_some()
        );
        // The read of the focused element failed.
        tracker.learn_element(None);
        assert!(tracker.wants_element());
        assert_eq!(
            tracker.observe(Signal::Element(6), now, seen(&form)),
            Some(form.clone())
        );
        assert!(!tracker.wants_element());
        assert_eq!(tracker.observe(Signal::Element(6), now, seen(&form)), None);
    }

    #[test]
    fn an_element_in_another_window_is_reported_with_that_window() {
        let now = Instant::now();
        let first = window(1, 10);
        let second = window(1, 11);
        let mut tracker = tracking(&first, 5, now);
        // The element notification came before any window notification.
        assert_eq!(
            tracker.observe(Signal::Element(8), now, seen(&second)),
            Some(second.clone())
        );
        // The window notification that follows is the same change.
        assert_eq!(tracker.observe(Signal::Window, now, seen(&second)), None);
        assert_eq!(
            tracker.observe(Signal::Element(8), now, seen(&second)),
            None
        );
    }

    #[test]
    fn a_learned_element_does_not_replace_one_a_notification_named() {
        let now = Instant::now();
        let form = window(1, 10);
        let mut tracker = tracking(&form, 5, now);
        assert!(
            tracker
                .observe(Signal::Element(6), now, seen(&form))
                .is_some()
        );
        // A read that was under way when the notification arrived.
        tracker.learn_element(Some(5));
        assert_eq!(tracker.observe(Signal::Element(6), now, seen(&form)), None);
    }

    #[test]
    fn a_timer_reports_only_what_differs_from_the_last_report() {
        let start = Instant::now();
        let launching = window(1, 0);
        let mut tracker = tracking(&launching, 5, start);
        let at = start + SETTLE[0];
        assert_eq!(tracker.observe(Signal::Timer, at, seen(&launching)), None);
        // The application's window appeared without a notification.
        let shown = window(1, 10);
        let at = start + SETTLE[1];
        assert_eq!(
            tracker.observe(Signal::Timer, at, seen(&shown)),
            Some(shown.clone())
        );
        assert!(tracker.wants_element());
        assert_eq!(tracker.observe(Signal::Timer, at, seen(&shown)), None);
    }

    #[test]
    fn every_signal_asks_for_two_more_looks_and_then_for_none() {
        let start = Instant::now();
        let editor = window(1, 10);
        let mut tracker = tracking(&editor, 5, start);
        assert_eq!(tracker.next_timer(), Some(start + SETTLE[0]));
        assert!(!tracker.due(start));
        assert!(tracker.due(start + SETTLE[0]));

        let at = start + SETTLE[0];
        assert_eq!(tracker.observe(Signal::Timer, at, seen(&editor)), None);
        assert_eq!(tracker.next_timer(), Some(start + SETTLE[1]));

        let at = start + SETTLE[1];
        assert_eq!(tracker.observe(Signal::Timer, at, seen(&editor)), None);
        // Idle: the watcher thread sleeps until the next notification.
        assert_eq!(tracker.next_timer(), None);
        assert!(!tracker.due(at + Duration::from_hours(1)));

        // A new signal starts over, from its own time.
        let later = at + Duration::from_secs(5);
        assert_eq!(tracker.observe(Signal::Window, later, seen(&editor)), None);
        assert_eq!(tracker.next_timer(), Some(later + SETTLE[0]));
    }

    #[test]
    fn secure_keyboard_entry_is_polled_only_while_it_is_on() {
        let start = Instant::now();
        let terminal = window(1, 10);
        let mut tracker = Tracker::<u32>::new();
        assert!(
            tracker
                .observe(Signal::Activated, start, secure(&terminal))
                .is_some()
        );
        tracker.learn_element(Some(5));
        // Past the settling looks, the poll goes on.
        let mut at = start;
        for _ in 0..6 {
            at = tracker.next_timer().expect("a poll while the flag is on");
            assert_eq!(tracker.observe(Signal::Timer, at, secure(&terminal)), None);
        }
        assert_eq!(tracker.next_timer(), Some(at + SECURE_INPUT_POLL));
        // It was switched off: reported, so that the engine asks for a new verdict.
        let at = at + SECURE_INPUT_POLL;
        assert_eq!(
            tracker.observe(Signal::Timer, at, seen(&terminal)),
            Some(terminal.clone())
        );
        assert_eq!(tracker.next_timer(), None);
        assert_eq!(tracker.observe(Signal::Timer, at, seen(&terminal)), None);
    }

    #[test]
    fn secure_keyboard_entry_seen_switched_on_is_reported() {
        let now = Instant::now();
        let form = window(1, 10);
        let mut tracker = tracking(&form, 5, now);
        // Seen on the occasion of a repeated notification that would not be reported.
        assert_eq!(
            tracker.observe(Signal::Element(5), now, secure(&form)),
            Some(form.clone())
        );
        assert_eq!(
            tracker.observe(Signal::Element(5), now, secure(&form)),
            None
        );
        assert_eq!(tracker.observe(Signal::Window, now, secure(&form)), None);
        assert_eq!(
            tracker.observe(Signal::Window, now, seen(&form)),
            Some(form)
        );
    }

    #[test]
    fn a_late_attachment_is_always_reported() {
        let now = Instant::now();
        let editor = window(1, 10);
        let mut tracker = tracking(&editor, 5, now);
        assert_eq!(
            tracker.observe(Signal::Attached, now, seen(&editor)),
            Some(editor)
        );
        assert!(tracker.wants_element());
    }

    #[test]
    fn the_observer_follows_the_frontmost_application() {
        let now = Instant::now();
        let mut attachment = Attachment::default();
        assert_eq!(attachment.step(None, now), Step::Keep);
        assert_eq!(attachment.step(Some(1), now), Step::Attach { pid: 1 });
        assert!(!attachment.attached(true, now));
        assert_eq!(attachment.step(Some(1), now), Step::Keep);
        assert_eq!(attachment.next_timer(), None);
        assert_eq!(attachment.step(Some(2), now), Step::Attach { pid: 2 });
        assert!(!attachment.attached(true, now));
        assert_eq!(attachment.step(None, now), Step::Detach);
        assert_eq!(attachment.step(None, now), Step::Keep);
        // Back to an application that was observed before: a new observer.
        assert_eq!(attachment.step(Some(2), now), Step::Attach { pid: 2 });
    }

    #[test]
    fn a_failed_registration_is_retried_and_a_late_success_says_so() {
        let start = Instant::now();
        let mut attachment = Attachment::default();
        assert_eq!(attachment.step(Some(1), start), Step::Attach { pid: 1 });
        assert!(!attachment.attached(false, start));
        assert_eq!(attachment.next_timer(), Some(start + ATTACH_RETRY));
        assert!(!attachment.due(start));
        assert_eq!(attachment.step(Some(1), start), Step::Keep);

        let at = start + ATTACH_RETRY;
        assert!(attachment.due(at));
        assert_eq!(attachment.step(Some(1), at), Step::Attach { pid: 1 });
        assert!(attachment.attached(true, at), "a late success");
        assert_eq!(attachment.next_timer(), None);
        assert_eq!(attachment.step(Some(1), at + ATTACH_RETRY), Step::Keep);
    }

    #[test]
    fn retries_end_after_the_last_attempt_and_start_over_with_the_next_activation() {
        let mut at = Instant::now();
        let mut attachment = Attachment::default();
        let mut attempts = 0;
        while let Step::Attach { pid: 1 } = attachment.step(Some(1), at) {
            attempts += 1;
            assert!(!attachment.attached(false, at));
            at += ATTACH_RETRY;
        }
        assert_eq!(attempts, ATTACH_ATTEMPTS);
        assert_eq!(attachment.next_timer(), None);
        assert!(!attachment.due(at + Duration::from_hours(1)));

        assert_eq!(attachment.step(Some(2), at), Step::Attach { pid: 2 });
        assert!(!attachment.attached(true, at));
        assert_eq!(attachment.step(Some(1), at), Step::Attach { pid: 1 });
        assert!(!attachment.attached(false, at));
        assert_eq!(attachment.next_timer(), Some(at + ATTACH_RETRY));
    }
}
