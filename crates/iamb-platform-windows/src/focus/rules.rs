//! The watcher's decisions, and the wait of `restore_focus`. No system call.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use iamb_platform::PlatformError;

/// Which window event the watcher was called with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventKind {
    /// Another window came to the front.
    Foreground,
    /// An element took the keyboard focus, in whatever window.
    Focus,
}

/// How the window of a focus event relates to the window in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Relation {
    /// The event names no window.
    pub(crate) unnamed: bool,
    /// Its window belongs to the process of the window in front.
    pub(crate) same_process: bool,
    /// Its window is the window in front, a child of it, or a pop-up it owns.
    pub(crate) same_root: bool,
}

/// Whether an event is reported as a focus change.
///
/// - `moved`: the window in front is not the one last reported. Always reported, whatever
///   the event was: this is the change that must never be missed.
/// - A change of foreground is reported.
/// - A focus event is reported when it may be about the field the user types into: its
///   window is part of the window in front, or belongs to the same process (a pop-up, a
///   second window of a browser), or is not named at all. Focus events of other
///   applications' background windows are not: each report clears what the engine holds,
///   so a program that moves its focus around in the background must not undo the user's
///   typing elsewhere.
///
/// The same element taking the focus twice is reported twice. Nothing identifies an element
/// across toolkits without asking the application, which the watcher never does; and a
/// report too many only costs an abbreviation in progress, where a report too few could
/// leave a password field unnoticed.
pub(crate) const fn reportable(kind: EventKind, moved: bool, relation: Relation) -> bool {
    moved
        || match kind {
            EventKind::Foreground => true,
            EventKind::Focus => relation.unnamed || relation.same_process || relation.same_root,
        }
}

/// The pauses before the watcher reports a window again whose state could not be told for a
/// reason that may pass (milliseconds): about 7.5 s in all. A browser engine builds its
/// accessibility tree only once it is first asked, and a large page (an application built on an
/// engine) takes seconds for that; an application that answers slower than the service waits
/// is reported again as soon as its late answer arrives (`inspect`). The first pauses are
/// short, so that a small page or a busy moment costs little.
pub(crate) const LOOK_AGAIN_MS: [u32; 5] = [150, 400, 1000, 2000, 4000];

/// When to report the window in front again.
///
/// `secure_state` cannot tell the state of a field while the application is busy, or while a
/// browser engine has not built its accessibility tree yet (`classify`). The service only
/// asks when the watcher reports a change, so the watcher reports such a window again after
/// a pause: at most [`LOOK_AGAIN_MS`]`.len()` times after each real event, with growing
/// pauses. If the state is still unknown then, it stays unknown (the engine stays
/// suspended) until the focus changes.
#[derive(Debug, Default)]
pub(crate) struct Settle {
    window: u64,
    asked: usize,
}

impl Settle {
    /// A real event for `window` was reported: the count starts again.
    pub(crate) fn event(&mut self, window: u64) {
        self.window = window;
        self.asked = 0;
    }

    /// The window the count is for.
    pub(crate) const fn window(&self) -> u64 {
        self.window
    }

    /// The state of `window` could not be told. The pause after which to report it again,
    /// or `None`: it is not the window last reported, or it has been reported often enough.
    pub(crate) fn look_again(&mut self, window: u64) -> Option<u32> {
        if window != self.window {
            return None;
        }
        let pause = LOOK_AGAIN_MS.get(self.asked).copied()?;
        self.asked += 1;
        Some(pause)
    }
}

/// The count of focus changes and the mark of a report again, shared by the watchers of one
/// platform and its `secure_state` (`watch`).
#[derive(Debug, Default)]
pub(crate) struct Counts {
    epoch: AtomicU64,
    /// The window and count of the last report again, until a question takes it.
    again: Mutex<Option<(u64, u64)>>,
}

impl Counts {
    fn again(&self) -> std::sync::MutexGuard<'_, Option<(u64, u64)>> {
        self.again.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The count as it is.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// A change of the focus: before it is reported (or, for the service's own reading, before
    /// the focus is read).
    pub(crate) fn count_change(&self) {
        *self.again() = None;
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// The next report of `window` is a report again.
    pub(crate) fn mark_again(&self, window: u64) {
        *self.again() = Some((window, self.epoch()));
    }

    /// Takes the mark: true when it is for `window` at count `epoch`.
    pub(crate) fn take_again(&self, window: u64, epoch: u64) -> bool {
        self.again().take() == Some((window, epoch))
    }
}

/// What `restore_focus` sees when it looks whether the window is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// The window is in front.
    InFront,
    /// Not yet.
    NotYet,
    /// The window no longer exists.
    Gone,
}

/// Waits until `look` sees the window in front, for at most `timeout`, looking every
/// `pause`.
///
/// # Errors
///
/// [`FocusLost`](PlatformError::FocusLost) when the window is gone;
/// [`Timeout`](PlatformError::Timeout) when it is not in front in time.
pub(crate) fn await_front(
    mut look: impl FnMut() -> Seen,
    timeout: Duration,
    pause: Duration,
) -> Result<(), PlatformError> {
    let started = Instant::now();
    loop {
        match look() {
            Seen::InFront => return Ok(()),
            Seen::Gone => return Err(PlatformError::FocusLost),
            Seen::NotYet => {}
        }
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(PlatformError::Timeout);
        }
        std::thread::sleep(pause.min(left));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn relation(unnamed: bool, same_process: bool, same_root: bool) -> Relation {
        Relation {
            unnamed,
            same_process,
            same_root,
        }
    }

    const UNRELATED: Relation = relation(false, false, false);

    #[test]
    fn a_new_window_in_front_is_always_reported() {
        for kind in [EventKind::Foreground, EventKind::Focus] {
            assert!(reportable(kind, true, UNRELATED));
        }
        // A foreground event for the window already reported (it was activated again).
        assert!(reportable(EventKind::Foreground, false, UNRELATED));
    }

    #[test]
    fn focus_events_count_when_they_may_be_about_the_field_in_front() {
        for related in [
            relation(false, true, true),  // a control of the window in front
            relation(false, false, true), // an application hosted in another process's frame
            relation(false, true, false), // a pop-up or a second window of the same process
            relation(true, false, false), // no window named: cannot be ruled out
        ] {
            assert!(reportable(EventKind::Focus, false, related), "{related:?}");
        }
        // A background application moving its own focus.
        assert!(!reportable(EventKind::Focus, false, UNRELATED));
    }

    #[test]
    fn a_window_is_reported_again_a_few_times_with_growing_pauses() {
        let mut settle = Settle::default();
        settle.event(7);
        assert_eq!(settle.window(), 7);
        let pauses: Vec<u32> = std::iter::from_fn(|| settle.look_again(7)).collect();
        assert_eq!(pauses, LOOK_AGAIN_MS);
        assert!(pauses.is_sorted());
        // Often enough: it stays unknown until something happens.
        assert_eq!(settle.look_again(7), None);

        // A real event starts the count again, for the same window too.
        settle.event(7);
        assert_eq!(settle.look_again(7), Some(LOOK_AGAIN_MS[0]));
        settle.event(8);
        assert_eq!(settle.look_again(8), Some(LOOK_AGAIN_MS[0]));
    }

    #[test]
    fn the_reports_again_give_a_slow_engine_several_seconds() {
        let total: u32 = LOOK_AGAIN_MS.iter().sum();
        assert!(total >= 5_000, "{total} ms");
        // Not so long that a window stays unknown for ever without saying so.
        assert!(total <= 10_000, "{total} ms");
        assert!(LOOK_AGAIN_MS[0] <= 200, "the first look comes soon");
    }

    #[test]
    fn a_window_that_is_no_longer_the_one_reported_is_not_reported_again() {
        let mut settle = Settle::default();
        settle.event(7);
        assert_eq!(settle.look_again(8), None);
        // And the answer about the old window does not use up the new one's count.
        assert_eq!(settle.look_again(7), Some(LOOK_AGAIN_MS[0]));
        // Before any event nothing is reported again.
        assert_eq!(Settle::default().look_again(7), None);
    }

    #[test]
    fn a_report_again_is_marked_for_its_window_and_count_and_taken_once() {
        let counts = Counts::default();
        counts.count_change();
        let epoch = counts.epoch();
        counts.mark_again(7);
        assert!(counts.take_again(7, epoch));
        assert!(!counts.take_again(7, epoch), "taken once");
        // Another window, or another count, does not match (and still uses the mark up).
        counts.mark_again(7);
        assert!(!counts.take_again(8, epoch));
        assert!(!counts.take_again(7, epoch));
        counts.mark_again(7);
        assert!(!counts.take_again(7, epoch + 1));
    }

    #[test]
    fn a_change_clears_the_mark_of_a_report_again() {
        // Finding L3 of the late-answer review: a change the service read itself, or one the
        // watcher reported after the mark, makes the question one about a change.
        let counts = Counts::default();
        counts.mark_again(7);
        let epoch = counts.epoch();
        counts.count_change();
        assert!(!counts.take_again(7, epoch));
        assert!(!counts.take_again(7, counts.epoch()));
    }

    #[test]
    fn waiting_ends_when_the_window_is_in_front() {
        let mut looks = 0;
        let outcome = await_front(
            || {
                looks += 1;
                if looks < 4 {
                    Seen::NotYet
                } else {
                    Seen::InFront
                }
            },
            Duration::from_secs(10),
            Duration::from_millis(1),
        );
        assert_eq!(outcome, Ok(()));
        assert_eq!(looks, 4);

        // Already in front: no waiting at all, even without time.
        assert_eq!(
            await_front(|| Seen::InFront, Duration::ZERO, Duration::from_secs(1)),
            Ok(())
        );
    }

    #[test]
    fn waiting_gives_up_in_time() {
        let started = Instant::now();
        let outcome = await_front(
            || Seen::NotYet,
            Duration::from_millis(30),
            Duration::from_millis(2),
        );
        assert_eq!(outcome, Err(PlatformError::Timeout));
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(30), "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");

        // A pause longer than the limit does not stretch it.
        let started = Instant::now();
        let outcome = await_front(
            || Seen::NotYet,
            Duration::from_millis(20),
            Duration::from_mins(1),
        );
        assert_eq!(outcome, Err(PlatformError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_window_that_disappears_is_lost_not_late() {
        let mut looks = 0;
        let outcome = await_front(
            || {
                looks += 1;
                if looks < 3 { Seen::NotYet } else { Seen::Gone }
            },
            Duration::from_secs(10),
            Duration::from_millis(1),
        );
        assert_eq!(outcome, Err(PlatformError::FocusLost));
    }
}
