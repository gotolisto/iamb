//! The permission status through the public API, as the worker's `GetPermissions` handler
//! will ask for it. Runs on every Mac, with or without the permissions.
//!
//! `open_settings` is not called here: it would open System Settings on the machine that runs
//! the tests and make the system show its permission prompt. Its rules are tested with a fake
//! probe in the crate (`permissions::policy`).

#![cfg(target_os = "macos")]

use std::sync::Arc;

use iamb_platform::{Grant, PermissionKind, Permissions};
use iamb_platform_macos::Platform;

#[test]
fn the_status_names_both_permissions_and_macos_always_requires_them() {
    let platform = Platform::new();
    let status = platform.permission_status();
    for kind in PermissionKind::ALL {
        assert!(
            matches!(status.grant(kind), Grant::Granted | Grant::Denied),
            "{kind} is granted or denied on a Mac, never not required"
        );
    }
    let missing = status.missing();
    assert_eq!(status.all_satisfied(), missing.is_empty());
    for kind in PermissionKind::ALL {
        assert_eq!(missing.contains(&kind), status.grant(kind) == Grant::Denied);
    }
}

#[test]
fn a_status_check_can_be_polled_and_never_changes_the_answer() {
    // Onboarding polls once a second. If a check requested access as a side effect, the
    // system would show its prompt during this test and, on a machine that was never asked,
    // the answers could differ from one call to the next.
    let platform = Platform::new();
    let first = platform.permission_status();
    for _ in 0..200 {
        assert_eq!(platform.permission_status(), first);
    }
    // Another platform object in the same process sees the same thing: the status belongs to
    // the process, not to the object.
    assert_eq!(Platform::new().permission_status(), first);
}

#[test]
fn the_status_is_reachable_through_the_trait_object_from_any_thread() {
    let platform: Arc<dyn iamb_platform::Platform> = Arc::new(Platform::new());
    let here = platform.permission_status();
    let elsewhere = {
        let platform = Arc::clone(&platform);
        std::thread::spawn(move || platform.permission_status())
            .join()
            .expect("the status thread")
    };
    assert_eq!(here, elsewhere);
}
