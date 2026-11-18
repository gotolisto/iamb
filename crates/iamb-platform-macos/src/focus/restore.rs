//! Giving the focus back to a window (architecture §4.5 step 4). Plain Rust, built and tested
//! on every system; the system calls sit behind [`Ops`].
//!
//! The caller is the UI host, after it hid its form or palette. Those are non-activating
//! panels, so in the usual case the target application never stopped being the active one
//! and its window is still in front: [`restore`] sees that and returns at once. Otherwise it
//! asks the system to activate the application and waits until the token's window is the
//! focused one.
//!
//! # What it cannot do
//!
//! It activates an application; it cannot raise one particular window of it. Raising a
//! window of another application takes the Accessibility API, and the UI host holds no
//! Accessibility grant (the grants are the worker's, ADR-0021). When the application comes
//! to the front with another of its windows in front, the answer is
//! [`FocusLost`](PlatformError::FocusLost) and nothing is typed.

use std::time::{Duration, Instant};

use iamb_platform::{FocusToken, PlatformError, Result};

/// How often the focus is looked at while waiting for the activation.
pub(crate) const POLL: Duration = Duration::from_millis(10);

/// The system as [`restore`] sees it.
pub(crate) trait Ops {
    /// The window that has the focus now, as the focus token names it.
    fn front(&self) -> Option<FocusToken>;
    /// Whether process `pid` is a running application.
    fn running(&self, pid: u32) -> bool;
    /// Whether the token's window still exists and belongs to the token's process.
    fn window_exists(&self, token: FocusToken) -> bool;
    /// Asks the system to make application `pid` the active one; `false` when the request
    /// could not be made.
    fn activate(&self, pid: u32) -> bool;
    /// Lets `duration` pass (and the system deliver what it has for this thread).
    fn pause(&self, duration: Duration);
    /// The time.
    fn now(&self) -> Instant;
}

/// Gives the focus back to the token's window and returns once it has it, waiting at most
/// `timeout` for the activation.
///
/// # Errors
///
/// [`FocusLost`](PlatformError::FocusLost) when the application or the window is gone, the
/// system refuses the request, or the application came to the front with another window;
/// [`Timeout`](PlatformError::Timeout) when the application did not come to the front in
/// time.
pub(crate) fn restore(ops: &dyn Ops, token: FocusToken, timeout: Duration) -> Result<()> {
    if ops.front() == Some(token) {
        return Ok(());
    }
    // A token with window 0 names an application without an ordinary window.
    if !ops.running(token.pid()) || (token.window() != 0 && !ops.window_exists(token)) {
        return Err(PlatformError::FocusLost);
    }
    if !ops.activate(token.pid()) {
        return Err(PlatformError::FocusLost);
    }
    let deadline = ops.now() + timeout;
    loop {
        let front = ops.front();
        if front == Some(token) {
            return Ok(());
        }
        if ops.now() >= deadline {
            let in_front = front.is_some_and(|front| front.pid() == token.pid());
            return Err(if in_front {
                PlatformError::FocusLost
            } else {
                PlatformError::Timeout
            });
        }
        ops.pause(POLL);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    const TARGET: FocusToken = FocusToken::from_raw(42, 60);
    const PANEL: FocusToken = FocusToken::from_raw(7, 70);
    const TIMEOUT: Duration = Duration::from_millis(500);

    /// A desktop with a clock of its own: `pause` moves the clock, nothing sleeps.
    struct Desk {
        start: Instant,
        elapsed: Cell<Duration>,
        front: Cell<Option<FocusToken>>,
        running: bool,
        window_exists: bool,
        accepts: bool,
        /// What comes to the front, and how long after the activation was asked for.
        arrives: Option<(Duration, FocusToken)>,
        activated_at: Cell<Option<Duration>>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Desk {
        fn new(front: Option<FocusToken>) -> Self {
            Self {
                start: Instant::now(),
                elapsed: Cell::new(Duration::ZERO),
                front: Cell::new(front),
                running: true,
                window_exists: true,
                accepts: true,
                arrives: Some((Duration::from_millis(35), TARGET)),
                activated_at: Cell::new(None),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn activations(&self) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| **c == "activate")
                .count()
        }
    }

    impl Ops for Desk {
        fn front(&self) -> Option<FocusToken> {
            if let (Some(asked), Some((after, window))) = (self.activated_at.get(), self.arrives)
                && self.elapsed.get() >= asked + after
            {
                self.front.set(Some(window));
            }
            self.front.get()
        }

        fn running(&self, _pid: u32) -> bool {
            self.running
        }

        fn window_exists(&self, _token: FocusToken) -> bool {
            self.calls.borrow_mut().push("window_exists");
            self.window_exists
        }

        fn activate(&self, _pid: u32) -> bool {
            self.calls.borrow_mut().push("activate");
            if self.accepts {
                self.activated_at.set(Some(self.elapsed.get()));
            }
            self.accepts
        }

        fn pause(&self, duration: Duration) {
            self.elapsed.set(self.elapsed.get() + duration);
        }

        fn now(&self) -> Instant {
            self.start + self.elapsed.get()
        }
    }

    #[test]
    fn a_window_that_still_has_the_focus_needs_nothing() {
        // The usual case: the form was a non-activating panel.
        let desk = Desk::new(Some(TARGET));
        assert_eq!(restore(&desk, TARGET, TIMEOUT), Ok(()));
        assert!(desk.calls.borrow().is_empty());
        assert_eq!(desk.elapsed.get(), Duration::ZERO);
    }

    #[test]
    fn the_application_is_activated_and_the_window_waited_for() {
        let desk = Desk::new(Some(PANEL));
        assert_eq!(restore(&desk, TARGET, TIMEOUT), Ok(()));
        assert_eq!(desk.activations(), 1);
        // It returned when the window was focused, not before and not much later.
        assert!(desk.elapsed.get() >= Duration::from_millis(35));
        assert!(desk.elapsed.get() <= Duration::from_millis(35) + POLL);
    }

    #[test]
    fn an_application_that_is_gone_is_focus_lost_and_nothing_is_activated() {
        let mut desk = Desk::new(Some(PANEL));
        desk.running = false;
        assert_eq!(
            restore(&desk, TARGET, TIMEOUT),
            Err(PlatformError::FocusLost)
        );
        assert_eq!(desk.activations(), 0);
    }

    #[test]
    fn a_window_that_is_gone_is_focus_lost_and_nothing_is_activated() {
        let mut desk = Desk::new(Some(PANEL));
        desk.window_exists = false;
        assert_eq!(
            restore(&desk, TARGET, TIMEOUT),
            Err(PlatformError::FocusLost)
        );
        assert_eq!(desk.activations(), 0);
    }

    #[test]
    fn a_refused_activation_is_focus_lost() {
        let mut desk = Desk::new(Some(PANEL));
        desk.accepts = false;
        assert_eq!(
            restore(&desk, TARGET, TIMEOUT),
            Err(PlatformError::FocusLost)
        );
        assert_eq!(desk.elapsed.get(), Duration::ZERO);
    }

    #[test]
    fn an_application_that_never_comes_to_the_front_is_a_timeout() {
        let mut desk = Desk::new(Some(PANEL));
        desk.arrives = None;
        assert_eq!(restore(&desk, TARGET, TIMEOUT), Err(PlatformError::Timeout));
        assert!(desk.elapsed.get() >= TIMEOUT);
        assert!(desk.elapsed.get() <= TIMEOUT + POLL);
        // Nothing in front at all is the same.
        let mut desk = Desk::new(None);
        desk.arrives = None;
        assert_eq!(restore(&desk, TARGET, TIMEOUT), Err(PlatformError::Timeout));
    }

    #[test]
    fn another_window_of_the_application_in_front_is_focus_lost() {
        let mut desk = Desk::new(Some(PANEL));
        desk.arrives = Some((Duration::from_millis(20), FocusToken::from_raw(42, 61)));
        assert_eq!(
            restore(&desk, TARGET, TIMEOUT),
            Err(PlatformError::FocusLost)
        );
        assert!(desk.elapsed.get() >= TIMEOUT);
    }

    #[test]
    fn a_token_without_a_window_names_the_application() {
        let token = FocusToken::from_raw(42, 0);
        let mut desk = Desk::new(Some(PANEL));
        desk.window_exists = false;
        desk.arrives = Some((Duration::from_millis(20), token));
        assert_eq!(restore(&desk, token, TIMEOUT), Ok(()));
        // The window is not looked for: there is none to look for.
        assert!(!desk.calls.borrow().contains(&"window_exists"));
    }

    #[test]
    fn a_late_arrival_within_the_timeout_still_succeeds() {
        let mut desk = Desk::new(Some(PANEL));
        desk.arrives = Some((TIMEOUT.saturating_sub(Duration::from_millis(15)), TARGET));
        assert_eq!(restore(&desk, TARGET, TIMEOUT), Ok(()));
    }
}
