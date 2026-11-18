//! The listener through the public API, with the system's real permission state.
//!
//! The permission-revoked case of the plan: without Input Monitoring or Accessibility, `start`
//! must fail with `PermissionDenied` naming the missing permission, and must not leave a tap
//! behind. A hosted CI runner has neither permission, so that is the case CI runs.
//!
//! With both permissions granted the test goes on to start a real listener. That needs the
//! system's keyboard layout, which macOS only hands out on the main thread; the standard test
//! harness keeps the main thread for itself, so here the start is refused with an error that
//! says so, and the test says that it skipped the rest. The same steps run against a real tap
//! in the crate's own tests (`listener::mac_tests::with_a_tap`), which load the layout before
//! the harness starts.
//!
//! A skip writes a line to standard error past the harness's capture; nothing is skipped
//! silently.

#![cfg(target_os = "macos")]

use std::io::Write;
use std::sync::mpsc::channel;
use std::time::Duration;

use iamb_platform::{
    EventSink, KeyListener, ListenerEvent, PermissionKind, Permissions, PlatformError,
};
use iamb_platform_macos::Platform;

fn skip(test: &str, why: &str) {
    let line = format!("SKIPPED listener_lifecycle::{test}: {why}\n");
    let _ = std::io::stderr().write_all(line.as_bytes());
}

fn sink() -> Box<dyn EventSink> {
    let (sender, receiver) = channel::<ListenerEvent>();
    // Keep the receiving end alive for as long as the sink lives.
    Box::new(move |event: ListenerEvent| {
        let _ = &receiver;
        let _ = sender.send(event);
    })
}

/// The permission `start` must name: Input Monitoring first, then Accessibility.
fn expected_refusal(platform: &Platform) -> Option<PermissionKind> {
    let status = platform.permission_status();
    [
        PermissionKind::InputMonitoring,
        PermissionKind::Accessibility,
    ]
    .into_iter()
    .find(|kind| !status.grant(*kind).is_satisfied())
}

#[test]
fn a_stopped_listener_is_not_running() {
    let platform = Platform::new();
    assert_eq!(
        platform.self_test(Duration::from_millis(10)),
        Err(PlatformError::NotRunning)
    );
    // Stopping a stopped listener is not an error.
    assert_eq!(platform.stop(), Ok(()));
    assert_eq!(platform.stop(), Ok(()));
}

#[test]
fn start_is_refused_without_the_permissions_and_runs_with_them() {
    const TEST: &str = "start_is_refused_without_the_permissions_and_runs_with_them";
    let platform = Platform::new();

    if let Some(missing) = expected_refusal(&platform) {
        // The permission-revoked case: no tap that silently sees nothing.
        for _ in 0..2 {
            assert_eq!(
                platform.start(sink()),
                Err(PlatformError::PermissionDenied(missing))
            );
            assert_eq!(
                platform.self_test(Duration::from_millis(10)),
                Err(PlatformError::NotRunning)
            );
        }
        assert_eq!(platform.stop(), Ok(()));
        // Asking did not change the answer: the refused start requested nothing.
        assert_eq!(expected_refusal(&platform), Some(missing));
        skip(
            TEST,
            &format!(
                "the part with a real event tap: the process that runs the tests lacks the \
                 permission \"{missing}\" (System Settings, Privacy & Security). The refusal \
                 itself was tested"
            ),
        );
        return;
    }

    match platform.start(sink()) {
        Ok(()) => {}
        Err(PlatformError::Os { context, .. }) if context.contains("main thread") => {
            skip(
                TEST,
                "the part with a real event tap: both permissions are granted, but the \
                 keyboard layout is read on the main thread, which the standard test harness \
                 keeps for itself. `cargo test -p iamb-platform-macos --lib with_a_tap` runs \
                 these steps against a real tap",
            );
            return;
        }
        Err(other) => panic!("start failed with both permissions granted: {other}"),
    }
    assert_eq!(platform.start(sink()), Err(PlatformError::AlreadyRunning));
    assert_eq!(platform.self_test(Duration::from_secs(1)), Ok(()));
    assert_eq!(platform.stop(), Ok(()));
    assert_eq!(platform.stop(), Ok(()));
    assert_eq!(
        platform.self_test(Duration::from_secs(1)),
        Err(PlatformError::NotRunning)
    );
    assert_eq!(platform.start(sink()), Ok(()));
    assert_eq!(platform.stop(), Ok(()));
}
