//! The user's session: the screen locks, the session is switched out, the user signs out
//! (architecture §4.5, §5; the 2.8 privacy review's finding 13).
//!
//! Remembered form values are user-entered text and must not outlive a locked screen. The
//! service watches the session through [`SessionEvents`] and, on every [`SessionEvent`],
//! overwrites the remembered values and the typed-characters hand-off. An event is a reason to
//! forget, never to remember: a report too many (a secure desktop that is not the lock screen,
//! a display that sleeps) costs only a form's remembered answers, so each OS crate reports
//! every signal that may mean the user left.
//!
//! | Event | Windows | macOS |
//! | --- | --- | --- |
//! | [`Locked`](SessionEvent::Locked) | the input desktop switched (`EVENT_SYSTEM_DESKTOPSWITCH`: the lock screen, the secure desktop of a credential prompt, Ctrl+Alt+Del) | `com.apple.screenIsLocked` (distributed notification); the screens went to sleep (`NSWorkspaceScreensDidSleepNotification`) |
//! | [`SwitchedOut`](SessionEvent::SwitchedOut) | (switching user locks first) | `NSWorkspaceSessionDidResignActiveNotification` |
//! | [`SigningOut`](SessionEvent::SigningOut) | `WM_QUERYENDSESSION`, `WM_ENDSESSION` | `NSWorkspaceWillPowerOffNotification` |
//! | [`Sleeping`](SessionEvent::Sleeping) | — | `NSWorkspaceWillSleepNotification` |
//!
//! Windows' own session notification (`WM_WTSSESSION_CHANGE`) is registered with
//! `WTSRegisterSessionNotification`, which the bindings keep in the Remote Desktop namespace
//! that the service may not link (architecture §2); the desktop switch is the signal the
//! focus watcher's hook machinery can already see.

use crate::error::Result;

/// Something happened to the user's session that means the user may have left the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionEvent {
    /// The screen locked (or, on Windows, another desktop took the input).
    Locked,
    /// The session was switched out for another user's.
    SwitchedOut,
    /// The user signs out, or the computer shuts down or restarts.
    SigningOut,
    /// The computer goes to sleep.
    Sleeping,
}

impl SessionEvent {
    /// The event's name, for logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::SwitchedOut => "switched_out",
            Self::SigningOut => "signing_out",
            Self::Sleeping => "sleeping",
        }
    }
}

/// Where a session watcher puts what it sees. Called on a thread the OS crate owns; it must
/// not block.
pub trait SessionSink: Send {
    /// Takes one event.
    fn session_event(&mut self, event: SessionEvent);
}

impl<F: FnMut(SessionEvent) + Send> SessionSink for F {
    fn session_event(&mut self, event: SessionEvent) {
        self(event);
    }
}

/// Reports screen locks, session switches and sign-outs.
pub trait SessionEvents: Send + Sync {
    /// Starts reporting session events to `sink`.
    ///
    /// # Errors
    ///
    /// [`AlreadyRunning`](crate::PlatformError::AlreadyRunning) when already watching;
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn watch_session(&self, sink: Box<dyn SessionSink>) -> Result<()>;

    /// Stops reporting and drops the sink. Not an error when not watching.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn unwatch_session(&self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closures_are_session_sinks_and_events_have_names() {
        let mut seen = Vec::new();
        {
            let mut sink = |e: SessionEvent| seen.push(e.as_str());
            sink.session_event(SessionEvent::Locked);
            sink.session_event(SessionEvent::SigningOut);
        }
        assert_eq!(seen, ["locked", "signing_out"]);
        assert_eq!(SessionEvent::SwitchedOut.as_str(), "switched_out");
        assert_eq!(SessionEvent::Sleeping.as_str(), "sleeping");
    }
}
