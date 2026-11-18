//! The focused application, window and field (architecture §3.1, §4.1, §4.5, §5).
//!
//! The service's focus thread turns what this module reports into the engine's two focus
//! events:
//!
//! 1. [`FocusInspector::watch`] delivers a [`FocusChange`] → the service sends
//!    `Event::FocusChanged(change.app.app)`; the engine suspends.
//! 2. The focus thread calls [`FocusInspector::secure_state`] for the change's window with
//!    [`SECURE_STATE_TIMEOUT`], combines the answer with what only the service knows (the
//!    exclusion list, the onboarding test field) in [`verdict`], and sends
//!    `Event::SecureState(..)`. If the focus has moved on in the meantime the answer is
//!    `Unknown(FocusMoved)`, never the state of some other window; the next change brings its
//!    own verdict.
//!
//! Iamb fails closed: anything that cannot be determined in time is `Unknown`, and the engine
//! stays suspended (§4.1, §8 rule 7).

use std::time::Duration;

use iamb_core::engine::{AppId, SecureState};

use crate::error::Result;

/// How long the focus thread waits for the secure-field answer (architecture §3.1).
pub const SECURE_STATE_TIMEOUT: Duration = Duration::from_millis(100);

/// The longest [`FocusInspector::restore_focus`] may take before it reports
/// [`Timeout`](crate::PlatformError::Timeout).
pub const RESTORE_FOCUS_TIMEOUT: Duration = Duration::from_millis(500);

/// The application that has the focus.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppIdentity {
    /// The bundle identifier (macOS) or the executable's file name (Windows). Never a window
    /// title: titles can contain document text.
    pub app: AppId,
    /// The process id.
    pub pid: u32,
    /// True when the process is part of Iamb (the UI host or the service itself). The OS crate
    /// decides: on macOS by the bundle identifier, on Windows by the executable living in the
    /// installation directory of the running one.
    pub own: bool,
}

impl AppIdentity {
    /// An identity for another application.
    #[must_use]
    pub fn new(app: impl Into<String>, pid: u32) -> Self {
        Self {
            app: AppId(app.into()),
            pid,
            own: false,
        }
    }

    /// The same identity, marked as one of Iamb's own processes.
    #[must_use]
    pub fn own(mut self) -> Self {
        self.own = true;
        self
    }
}

/// Names one window so the focus can be given back to it (architecture §4.5).
///
/// The service captures a token when it asks the UI host for a form or the palette; the token
/// travels over IPC (`FormRequested`, `PaletteRequested`, `ExpandSnippet`, `SetTestField`), so
/// it is plain data: the process id and the OS window number (the window handle on Windows,
/// the window id on macOS). It holds no text and is not a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FocusToken {
    pid: u32,
    window: u64,
}

impl FocusToken {
    /// A token from its two numbers (the OS crates, and IPC decoding).
    #[must_use]
    pub const fn from_raw(pid: u32, window: u64) -> Self {
        Self { pid, window }
    }

    /// The process that owns the window.
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }

    /// The OS window number.
    #[must_use]
    pub const fn window(self) -> u64 {
        self.window
    }
}

/// Why the state of the focused field could not be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnknownReason {
    /// The inspection did not answer within the timeout.
    Timeout,
    /// The window runs at a higher privilege level than Iamb; Windows refuses both inspection
    /// and injection (§5).
    Elevated,
    /// The window does not expose its focused element, or the accessibility API failed.
    NotInspectable,
    /// No window has the focus.
    NoFocus,
    /// The window asked about no longer has the focus.
    FocusMoved,
}

/// What the OS says about the focused field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldSecurity {
    /// An ordinary field.
    Normal,
    /// A password field, or secure keyboard entry is on.
    Secure,
    /// It could not be determined; treated as not expandable.
    Unknown(UnknownReason),
}

/// The focused application, window or text field changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusChange {
    /// The application that now has the focus.
    pub app: AppIdentity,
    /// Its focused window.
    pub token: FocusToken,
}

/// Where a focus watcher puts what it sees. Called on a thread the OS crate owns; it must not
/// block (the service forwards to the focus thread's channel).
pub trait FocusSink: Send {
    /// Takes one change.
    fn focus_changed(&mut self, change: FocusChange);
}

impl<F: FnMut(FocusChange) + Send> FocusSink for F {
    fn focus_changed(&mut self, change: FocusChange) {
        self(change);
    }
}

/// Inspects and moves the keyboard focus.
///
/// Used by the service (everything) and by the UI host
/// ([`restore_focus`](Self::restore_focus): the process that holds the focus is the one that
/// gives it back, ADR-0013).
pub trait FocusInspector: Send + Sync {
    /// The application that has the focus now.
    ///
    /// # Errors
    ///
    /// [`NoFocus`](crate::PlatformError::NoFocus) when no window is focused.
    fn focused_app(&self) -> Result<AppIdentity>;

    /// Whether the focused field of window `target` is a secure field, waiting at most
    /// `timeout` ([`SECURE_STATE_TIMEOUT`] in the service).
    ///
    /// `target` is the token of the [`FocusChange`] being judged. The answer is only ever
    /// about that window: if another window has the focus before or after the inspection, the
    /// answer is `Unknown(FocusMoved)`. The slow accessibility query runs on a thread the OS
    /// crate owns, so a hung application cannot hold the caller longer than `timeout`. Every
    /// failure is an [`Unknown`](FieldSecurity::Unknown): this never returns an error, so no
    /// caller can mistake a failure for "normal".
    fn secure_state(&self, target: &FocusToken, timeout: Duration) -> FieldSecurity;

    /// A token for the focused window, to give the focus back to it later.
    ///
    /// # Errors
    ///
    /// [`NoFocus`](crate::PlatformError::NoFocus) when no window is focused.
    fn focus_token(&self) -> Result<FocusToken>;

    /// Gives the focus back to the token's window and returns once the OS confirms that the
    /// window is focused (§4.5 step 4), within [`RESTORE_FOCUS_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// [`FocusLost`](crate::PlatformError::FocusLost) when the window is gone or refuses the
    /// focus; [`Timeout`](crate::PlatformError::Timeout) when the OS does not confirm in time.
    fn restore_focus(&self, token: &FocusToken) -> Result<()>;

    /// Lets process `pid` take the foreground: the service grants this to the UI host on
    /// Windows before it asks for a form or the palette (§4.5 step 3). On macOS the panels do
    /// not activate, so this does nothing and succeeds.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn allow_foreground(&self, pid: u32) -> Result<()>;

    /// Starts reporting focus changes to `sink`: another application or window took the focus,
    /// or the focused text field inside a window changed (the secure-field answer belongs to a
    /// field, so a move from a name field to a password field must be reported). The current
    /// focus is reported once, at once.
    ///
    /// # Errors
    ///
    /// [`AlreadyRunning`](crate::PlatformError::AlreadyRunning) when already watching;
    /// [`PermissionDenied`](crate::PlatformError::PermissionDenied) without Accessibility on
    /// macOS; [`Os`](crate::PlatformError::Os) otherwise.
    fn watch(&self, sink: Box<dyn FocusSink>) -> Result<()>;

    /// Stops reporting and drops the sink. Not an error when not watching.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn unwatch(&self) -> Result<()>;
}

/// The engine's focus verdict (`Event::SecureState`) for the focused field.
///
/// - `excluded`: the application is on the user's exclusion list (the service looks
///   `app.app` up in the snapshot's settings);
/// - `test_field`: the focused window is the onboarding test field the UI host declared with
///   `SetTestField` (§4.5), the one window of Iamb's own where typing expands.
///
/// Order of the rules: Iamb's own windows first (so the palette never expands in itself, even
/// if the user excluded nothing), then exclusions (an excluded application is not even
/// reported as secure or unknown), then what the OS said about the field.
#[must_use]
pub const fn verdict(
    app: &AppIdentity,
    field: FieldSecurity,
    excluded: bool,
    test_field: bool,
) -> SecureState {
    if app.own && !test_field {
        return SecureState::OwnWindow;
    }
    if excluded {
        return SecureState::Excluded;
    }
    match field {
        FieldSecurity::Normal => SecureState::Normal,
        FieldSecurity::Secure => SecureState::Secure,
        FieldSecurity::Unknown(_) => SecureState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_and_tokens_are_plain_data() {
        let app = AppIdentity::new("org.example.editor", 42);
        assert_eq!(app.app, AppId("org.example.editor".to_owned()));
        assert_eq!(app.pid, 42);
        assert!(!app.own);
        assert!(app.clone().own().own);

        let token = FocusToken::from_raw(42, 0xDEAD_BEEF_0001);
        assert_eq!(token.pid(), 42);
        assert_eq!(token.window(), 0xDEAD_BEEF_0001);
        assert_eq!(token, FocusToken::from_raw(token.pid(), token.window()));
        assert_ne!(token, FocusToken::from_raw(42, 1));
    }

    #[test]
    fn the_verdict_follows_the_field_for_other_applications() {
        let app = AppIdentity::new("editor", 1);
        assert_eq!(
            verdict(&app, FieldSecurity::Normal, false, false),
            SecureState::Normal
        );
        assert_eq!(
            verdict(&app, FieldSecurity::Secure, false, false),
            SecureState::Secure
        );
        for reason in [
            UnknownReason::Timeout,
            UnknownReason::Elevated,
            UnknownReason::NotInspectable,
            UnknownReason::NoFocus,
            UnknownReason::FocusMoved,
        ] {
            assert_eq!(
                verdict(&app, FieldSecurity::Unknown(reason), false, false),
                SecureState::Unknown,
                "fails closed"
            );
        }
    }

    #[test]
    fn exclusion_wins_over_the_field() {
        let app = AppIdentity::new("bank", 1);
        for field in [
            FieldSecurity::Normal,
            FieldSecurity::Secure,
            FieldSecurity::Unknown(UnknownReason::Timeout),
        ] {
            assert_eq!(verdict(&app, field, true, false), SecureState::Excluded);
        }
    }

    #[test]
    fn own_windows_never_expand_except_the_test_field() {
        let own = AppIdentity::new("iamb-desktop", 7).own();
        for (field, excluded) in [
            (FieldSecurity::Normal, false),
            (FieldSecurity::Normal, true),
            (FieldSecurity::Secure, false),
        ] {
            assert_eq!(
                verdict(&own, field, excluded, false),
                SecureState::OwnWindow
            );
        }
        assert_eq!(
            verdict(&own, FieldSecurity::Normal, false, true),
            SecureState::Normal
        );
        // The test field is still judged like any field.
        assert_eq!(
            verdict(&own, FieldSecurity::Secure, false, true),
            SecureState::Secure
        );
        assert_eq!(
            verdict(
                &own,
                FieldSecurity::Unknown(UnknownReason::Timeout),
                false,
                true
            ),
            SecureState::Unknown
        );
        // The flag means nothing for other applications.
        assert_eq!(
            verdict(
                &AppIdentity::new("editor", 1),
                FieldSecurity::Secure,
                false,
                true
            ),
            SecureState::Secure
        );
    }

    #[test]
    fn closures_are_focus_sinks() {
        let mut seen = Vec::new();
        {
            let mut sink = |change: FocusChange| seen.push(change.token.window());
            sink.focus_changed(FocusChange {
                app: AppIdentity::new("a", 1),
                token: FocusToken::from_raw(1, 10),
            });
        }
        assert_eq!(seen, [10]);
    }
}
