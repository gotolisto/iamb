//! Tests of the focus module against the real system, under the standard test harness.
//! macOS only.
//!
//! The harness keeps the main thread for itself and never runs its run loop, so what needs
//! the main thread cannot succeed here: the secure-entry flag is not answered and the
//! activation notification cannot be registered. These tests check that both fail closed.
//! The tests with a running main loop, a real window and a real password field are in
//! `tests/focus.rs`, which has its own `main`.
//!
//! Nothing here needs a permission. A skipped check writes its reason straight to standard
//! error, which the harness does not capture.

use std::io::Write;
use std::time::{Duration, Instant};

use iamb_platform::{
    FieldSecurity, FocusChange, FocusInspector, FocusToken, PermissionKind, PlatformError,
    UnknownReason,
};
use objc2::MainThreadMarker;
use objc2_application_services::AXError;

use super::field::code;
use super::{MAIN_RUN_LOOP_NOT_RUNNING, identity, system};
use crate::Platform;
use crate::login_item::HELPER_BUNDLE_ID;
use crate::permissions::SystemProbe;
use crate::permissions::policy::Probe;

fn skipped(test: &str, reason: &str) {
    let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {reason}");
}

#[test]
fn the_error_codes_are_the_systems() {
    assert_eq!(code::SUCCESS, AXError::Success.0);
    assert_eq!(code::CANNOT_COMPLETE, AXError::CannotComplete.0);
    assert_eq!(code::ATTRIBUTE_UNSUPPORTED, AXError::AttributeUnsupported.0);
    assert_eq!(
        code::NOTIFICATION_UNSUPPORTED,
        AXError::NotificationUnsupported.0
    );
    assert_eq!(
        code::NOTIFICATION_ALREADY_REGISTERED,
        AXError::NotificationAlreadyRegistered.0
    );
    assert_eq!(code::API_DISABLED, AXError::APIDisabled.0);
    assert_eq!(code::NO_VALUE, AXError::NoValue.0);
}

#[test]
fn the_helper_app_is_one_of_iambs_own() {
    assert!(identity::is_own_bundle(HELPER_BUNDLE_ID));
    assert!(identity::is_own_bundle(identity::APP_BUNDLE_ID));
}

#[test]
fn allow_foreground_does_nothing_and_succeeds() {
    let platform = Platform::new();
    assert_eq!(platform.allow_foreground(1), Ok(()));
    assert_eq!(platform.allow_foreground(std::process::id()), Ok(()));
}

#[test]
fn the_frontmost_application_has_a_name_and_a_process() {
    let platform = Platform::new();
    match platform.focused_app() {
        Ok(app) => {
            assert!(!app.app.0.is_empty());
            assert_ne!(app.pid, 0);
            if app.pid == std::process::id() {
                assert!(app.own, "the calling process is one of Iamb's own");
            }
        }
        Err(PlatformError::NoFocus) => skipped(
            "the_frontmost_application_has_a_name_and_a_process",
            "no application is frontmost in this session",
        ),
        Err(other) => panic!("focused_app: {other}"),
    }
}

#[test]
fn the_focus_token_names_the_frontmost_application() {
    let platform = Platform::new();
    // The focus can move between the two reads; three tries make that an accident.
    for _ in 0..3 {
        let (Ok(app), Ok(token)) = (platform.focused_app(), platform.focus_token()) else {
            continue;
        };
        if token.pid() == app.pid {
            assert_ne!(token.window(), 0);
            assert!(system::window_exists(token));
            return;
        }
    }
    skipped(
        "the_focus_token_names_the_frontmost_application",
        "no application with an ordinary window is frontmost in this session",
    );
}

#[test]
fn the_window_list_can_be_read_without_any_permission() {
    // This process shows no window.
    assert_eq!(system::front_window(std::process::id()), 0);
    assert!(!system::window_exists(FocusToken::from_raw(
        std::process::id(),
        u64::from(u32::MAX)
    )));
    assert!(!system::window_exists(FocusToken::from_raw(1, u64::MAX)));
}

#[test]
fn a_window_that_does_not_have_the_focus_is_never_normal() {
    let platform = Platform::new();
    // No process has this id.
    let nowhere = FocusToken::from_raw(u32::MAX - 1, 1);
    let started = Instant::now();
    let answer = platform.secure_state(&nowhere, Duration::from_secs(5));
    assert!(
        matches!(
            answer,
            FieldSecurity::Unknown(UnknownReason::FocusMoved | UnknownReason::NoFocus)
        ),
        "{answer:?}"
    );
    // Decided from who is in front alone: neither the main thread nor an application was
    // waited for.
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn without_a_main_run_loop_the_secure_state_fails_closed_in_time() {
    if MainThreadMarker::new().is_some() {
        return;
    }
    let platform = Platform::new();
    let Ok(token) = platform.focus_token() else {
        skipped(
            "without_a_main_run_loop_the_secure_state_fails_closed_in_time",
            "no application with an ordinary window is frontmost in this session",
        );
        return;
    };
    let timeout = Duration::from_millis(100);
    let started = Instant::now();
    let answer = platform.secure_state(&token, timeout);
    let waited = started.elapsed();
    // The main thread never says whether secure keyboard entry is on. (`FocusMoved` when
    // the focus happened to move during the test.)
    assert!(
        matches!(
            answer,
            FieldSecurity::Unknown(UnknownReason::Timeout | UnknownReason::FocusMoved)
        ),
        "{answer:?}"
    );
    assert!(waited < timeout + Duration::from_millis(400), "{waited:?}");
}

#[test]
fn a_watch_is_refused_without_accessibility_or_without_a_main_run_loop() {
    if MainThreadMarker::new().is_some() {
        return;
    }
    let platform = Platform::new();
    let result = platform.watch(Box::new(|_: FocusChange| {}));
    if SystemProbe.accessibility() {
        // The permission is there; the main thread, which would have to register for the
        // activation notification, is not running its run loop.
        assert_eq!(
            result,
            Err(PlatformError::os(0, MAIN_RUN_LOOP_NOT_RUNNING)),
            "with Accessibility"
        );
    } else {
        assert_eq!(
            result,
            Err(PlatformError::PermissionDenied(
                PermissionKind::Accessibility
            )),
            "without Accessibility"
        );
    }
    // Nothing was started, so there is nothing to stop, and stopping is not an error.
    assert_eq!(platform.unwatch(), Ok(()));
    assert_eq!(platform.unwatch(), Ok(()));
}

#[test]
fn the_state_shows_threads_and_nothing_about_the_focus() {
    let platform = Platform::new();
    assert_eq!(
        format!("{:?}", platform.focus),
        "Focus { inspecting: false, watching: false }"
    );
    let _ = platform.secure_state(&FocusToken::from_raw(1, 1), Duration::from_millis(10));
    assert_eq!(
        format!("{:?}", platform.focus),
        "Focus { inspecting: true, watching: false }"
    );
}
