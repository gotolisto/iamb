//! The focus inspector end to end, through the public trait only, against real windows of
//! this test: a plain text field, a password field and a button. Runs on Windows only (CI's
//! Windows leg).
//!
//! Nothing is typed. The tests move the foreground and the keyboard focus, so they run one
//! at a time. A test that needs something the session does not have (an interactive desktop
//! that lets a window come to the front) says `SKIPPED` and why, and fails instead when
//! `IAMB_REQUIRE_DESKTOP` is set.
//!
//! # The KPI "0 keys buffered in secure fields"
//!
//! The buffer is the engine's, in the service. It is measured in two halves that meet at
//! one value:
//!
//! 1. Here: with a real password control focused, the platform answers `Secure`, and the
//!    shared rule turns that into the engine's `SecureState::Secure`
//!    ([`a_password_field_is_secure_and_the_engine_is_told_so`]).
//! 2. In `iamb-service` (`tests/end_to_end.rs`, on every system): given that state, keys
//!    typed reach no buffer and nothing expands.
//!
//! Browser and password-manager fields cannot be opened on a build machine; they are manual
//! steps (`docs/compatibility-run.md`, rows W19 and W20).

#![cfg(windows)]

#[path = "focus_support.rs"]
mod support;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use iamb_platform::core_types::SecureState;
use iamb_platform::{
    FieldSecurity, FocusChange, FocusInspector, FocusToken, PlatformError, SECURE_STATE_TIMEOUT,
    UnknownReason, verdict,
};
use iamb_platform_windows::Platform;

use support::{Field, TestWindow, alone, eventually, skip};

/// Generous: the first question loads the automation library.
const PATIENT: Duration = Duration::from_secs(5);

/// A test window with `field` focused; `None` (with the reason said) when the session
/// cannot run the test.
fn open(field: Field) -> Option<TestWindow> {
    let Some(window) = TestWindow::open() else {
        skip("this session cannot create a window");
        return None;
    };
    if !window.focus(field) {
        skip("the test window could not take the foreground and the keyboard focus");
        return None;
    }
    Some(window)
}

/// Gives `field` of `window` the focus; false (with the reason said) when the session
/// refuses.
fn focus(window: &TestWindow, field: Field) -> bool {
    let focused = window.focus(field);
    if !focused {
        skip("the test window could not take the foreground and the keyboard focus");
    }
    focused
}

/// The name this test's executable has as an application.
fn own_name() -> String {
    std::env::current_exe()
        .expect("the test executable has a path")
        .file_name()
        .expect("and a file name")
        .to_string_lossy()
        .to_lowercase()
}

/// The state of the focused field of `token`'s window, asked until the application has
/// answered (the service gets the same through the watcher's second look).
fn settled_state(platform: &Platform, token: &FocusToken) -> FieldSecurity {
    let mut last = FieldSecurity::Unknown(UnknownReason::Timeout);
    eventually(|| {
        last = platform.secure_state(token, PATIENT);
        (!matches!(
            last,
            FieldSecurity::Unknown(UnknownReason::Timeout | UnknownReason::NotInspectable)
        ))
        .then_some(())
    });
    last
}

/// The next change `changes` holds for which `wanted` is true, within five seconds.
fn next_change(
    changes: &Receiver<FocusChange>,
    wanted: impl Fn(&FocusChange) -> bool,
) -> Option<FocusChange> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match changes.recv_timeout(left) {
            Ok(change) if wanted(&change) => return Some(change),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Throws away the changes reported so far, and those of the next moment.
fn drain(changes: &Receiver<FocusChange>) {
    while changes.recv_timeout(Duration::from_millis(300)).is_ok() {}
}

#[test]
fn the_application_in_front_is_named_by_its_executable() {
    let _alone = alone();
    let Some(window) = open(Field::Plain) else {
        return;
    };
    let platform = Platform::new();

    let app = platform.focused_app().expect("a window is in front");
    assert_eq!(app.app.0, own_name());
    assert_eq!(app.pid, std::process::id());
    assert!(app.own, "the executable is in its own directory");
    // A file name, not a path.
    assert!(!app.app.0.contains(['\\', '/', ':']), "{}", app.app.0);

    let token = platform.focus_token().expect("a window is in front");
    assert_eq!(
        token,
        FocusToken::from_raw(std::process::id(), window.number())
    );
}

#[test]
fn a_password_field_is_secure_and_the_engine_is_told_so() {
    let _alone = alone();
    let Some(window) = open(Field::Plain) else {
        return;
    };
    let platform = Platform::new();
    let token = platform.focus_token().expect("a window is in front");
    let app = platform.focused_app().expect("a window is in front");
    assert_eq!(token.window(), window.number());

    // A system edit control answers by its style, at once: the service's own limit is
    // plenty, also for the first question.
    assert_eq!(
        platform.secure_state(&token, SECURE_STATE_TIMEOUT),
        FieldSecurity::Normal
    );

    if !focus(&window, Field::Password) {
        return;
    }
    let field = platform.secure_state(&token, SECURE_STATE_TIMEOUT);
    assert_eq!(field, FieldSecurity::Secure);
    // What the service's focus thread makes of it for another application's window: the
    // engine is suspended, and keys reach no buffer (the KPI, see the module docs).
    let other = iamb_platform::AppIdentity::new(app.app.0.clone(), app.pid);
    assert_eq!(verdict(&other, field, false, false), SecureState::Secure);
    // No time to wait in changes nothing: nothing is waited for.
    assert_eq!(
        platform.secure_state(&token, Duration::ZERO),
        FieldSecurity::Secure
    );

    // And back.
    if !focus(&window, Field::Plain) {
        return;
    }
    assert_eq!(
        platform.secure_state(&token, SECURE_STATE_TIMEOUT),
        FieldSecurity::Normal
    );
}

#[test]
fn a_control_that_is_no_edit_control_is_asked_through_ui_automation() {
    let _alone = alone();
    let Some(_window) = open(Field::Button) else {
        return;
    };
    let platform = Platform::new();
    let token = platform.focus_token().expect("a window is in front");

    // Without time the application cannot be asked, and that is not "normal".
    assert_eq!(
        platform.secure_state(&token, Duration::ZERO),
        FieldSecurity::Unknown(UnknownReason::Timeout)
    );
    // A button is not a password field. This answer comes from UI Automation.
    assert_eq!(settled_state(&platform, &token), FieldSecurity::Normal);
    // Once the library is loaded and the application has been asked, the service's limit
    // is enough.
    let quick = eventually(|| {
        let state = platform.secure_state(&token, SECURE_STATE_TIMEOUT);
        (state == FieldSecurity::Normal).then_some(state)
    });
    assert_eq!(quick, Some(FieldSecurity::Normal));
}

#[test]
fn the_answer_is_only_about_the_window_asked_for() {
    let _alone = alone();
    let Some(first) = open(Field::Password) else {
        return;
    };
    let platform = Platform::new();
    let first_token = platform.focus_token().expect("a window is in front");
    assert_eq!(first_token.window(), first.number());
    assert_eq!(
        platform.secure_state(&first_token, PATIENT),
        FieldSecurity::Secure
    );

    let Some(second) = open(Field::Plain) else {
        return;
    };
    // The first window's password field still has that window's focus, but the window is
    // not in front: the answer is neither its state nor the second window's.
    assert_eq!(
        platform.secure_state(&first_token, PATIENT),
        FieldSecurity::Unknown(UnknownReason::FocusMoved)
    );
    let second_token = platform.focus_token().expect("a window is in front");
    assert_eq!(second_token.window(), second.number());
    assert_eq!(
        platform.secure_state(&second_token, PATIENT),
        FieldSecurity::Normal
    );
    // The same window handle under another process id is another window.
    let forged = FocusToken::from_raw(second_token.pid() + 4, second_token.window());
    assert_eq!(
        platform.secure_state(&forged, PATIENT),
        FieldSecurity::Unknown(UnknownReason::FocusMoved)
    );
}

#[test]
fn the_watcher_reports_the_focus_at_once_and_every_change() {
    let _alone = alone();
    let Some(first) = open(Field::Plain) else {
        return;
    };
    let platform = Platform::new();
    let (sink, changes) = mpsc::channel();
    assert_eq!(
        platform.watch(Box::new(move |change: FocusChange| {
            let _ = sink.send(change);
        })),
        Ok(())
    );

    // The focus as it is, before `watch` returns.
    let current = changes
        .try_recv()
        .expect("the current focus is reported at once");
    assert_eq!(current.token.window(), first.number());
    assert_eq!(current.token.pid(), std::process::id());
    assert_eq!(current.app.app.0, own_name());
    assert!(current.app.own);

    // One watcher at a time.
    assert_eq!(
        platform.watch(Box::new(|_: FocusChange| {})),
        Err(PlatformError::AlreadyRunning)
    );

    // Another window comes to the front.
    let Some(second) = open(Field::Plain) else {
        return;
    };
    let moved = next_change(&changes, |change| change.token.window() == second.number());
    assert!(moved.is_some(), "the change of foreground was not reported");
    let token = moved.expect("checked").token;
    assert_eq!(settled_state(&platform, &token), FieldSecurity::Normal);

    // The focus moves from the text field to the password field of the same window: that
    // must be reported, because the answer belongs to the field.
    drain(&changes);
    if !focus(&second, Field::Password) {
        return;
    }
    let inside = next_change(&changes, |change| change.token == token);
    assert!(inside.is_some(), "the change of field was not reported");
    assert_eq!(
        platform.secure_state(&token, SECURE_STATE_TIMEOUT),
        FieldSecurity::Secure
    );

    // And back to the first window.
    drain(&changes);
    if !focus(&first, Field::Plain) {
        return;
    }
    assert!(
        next_change(&changes, |change| change.token.window() == first.number()).is_some(),
        "the change back was not reported"
    );

    // Stopping drops the sink; stopping again is not an error; watching again works.
    assert_eq!(platform.unwatch(), Ok(()));
    loop {
        match changes.recv_timeout(Duration::from_secs(5)) {
            Ok(_) => {}
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => panic!("the sink was not dropped"),
        }
    }
    assert_eq!(platform.unwatch(), Ok(()));
    let (sink, changes) = mpsc::channel();
    assert_eq!(
        platform.watch(Box::new(move |change: FocusChange| {
            let _ = sink.send(change);
        })),
        Ok(())
    );
    assert!(changes.try_recv().is_ok());
    assert_eq!(platform.unwatch(), Ok(()));
}

#[test]
fn the_focus_is_given_back_to_the_window_of_a_token() {
    let _alone = alone();
    let Some(target) = open(Field::Password) else {
        return;
    };
    let platform = Platform::new();
    let token = platform.focus_token().expect("a window is in front");
    assert_eq!(token.window(), target.number());

    // Already in front: nothing to do.
    assert_eq!(platform.restore_focus(&token), Ok(()));

    // Another window of this process takes the foreground, as the UI host's form does.
    let Some(form) = open(Field::Plain) else {
        return;
    };
    assert!(form.in_front());
    let started = Instant::now();
    assert_eq!(platform.restore_focus(&token), Ok(()));
    assert!(started.elapsed() < Duration::from_secs(2));
    // The call returned only once the window was in front, with the focus on the field
    // that had it.
    assert!(target.in_front());
    assert_eq!(platform.focus_token(), Ok(token));
    assert!(
        eventually(|| target.has_focus(Field::Password).then_some(())).is_some(),
        "the field did not get the keyboard focus back"
    );
    assert_eq!(
        platform.secure_state(&token, SECURE_STATE_TIMEOUT),
        FieldSecurity::Secure
    );

    // A token whose process id is not the window's names no window.
    let forged = FocusToken::from_raw(token.pid() + 4, token.window());
    assert_eq!(
        platform.restore_focus(&forged),
        Err(PlatformError::FocusLost)
    );
    // A window that is gone cannot take the focus back.
    drop(target);
    assert_eq!(
        platform.restore_focus(&token),
        Err(PlatformError::FocusLost)
    );
}

#[test]
fn the_ui_host_may_be_allowed_the_foreground() {
    let _alone = alone();
    let Some(_window) = open(Field::Plain) else {
        return;
    };
    let platform = Platform::new();
    // This process has the foreground, so it may hand the right on; here to itself.
    assert_eq!(platform.allow_foreground(std::process::id()), Ok(()));
}
