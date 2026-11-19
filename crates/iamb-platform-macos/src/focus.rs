//! The focused application, window and field (architecture §3.1, §4.1, §4.5, §5, §8;
//! ADR-0012, ADR-0013, ADR-0021; the trait docs of
//! [`FocusInspector`](iamb_platform::FocusInspector)).
//!
//! # Map
//!
//! What this module decides is plain Rust, built and tested on every system. Only the code
//! that calls macOS is behind `cfg(target_os = "macos")`.
//!
//! | Module | Built on | Holds |
//! | --- | --- | --- |
//! | [`field`] | every system | the only Accessibility attributes ever asked for; what a role, a subrole and a failed read mean |
//! | [`identity`] | every system | the application's name and `own` flag; which window the focus token names |
//! | [`inspect`] | every system | the order of the secure-field answer, and the inspection thread with its deadline |
//! | [`track`] | every system | which notifications become a report to the focus sink; the watcher's timers |
//! | [`restore`] | every system | giving the focus back, and how the outcome is told |
//! | `system` | macOS | the system calls: frontmost application, window list, Accessibility reads, the secure-entry flag |
//! | `watcher` | macOS | the activation observer on the main thread and the watcher thread with its Accessibility observer |
//! | `mac` | macOS | [`FocusInspector`](iamb_platform::FocusInspector) for `Platform` |
//!
//! # What each method does
//!
//! - `focused_app`: the frontmost application of the workspace. Its name is the bundle
//!   identifier (the executable's file name when it has none), never a window title; `own`
//!   is true for Iamb's bundles and for the calling process ([`identity`]).
//! - `focus_token`: the frontmost application's process id and the window number of its
//!   front ordinary window ([`identity`]); `NoFocus` when it shows none.
//! - `secure_state`: `Secure` when secure keyboard entry is on in the login session
//!   (Terminal's Secure Keyboard Entry, password managers, every native password field and
//!   the browsers' password inputs switch it on), or when the focused element is a secure
//!   text field by its Accessibility role or subrole. Everything else that is not a plain
//!   "normal" is an `Unknown` ([`inspect`], [`field`]).
//! - `watch`: reports the current focus at once, then every activation of an application and
//!   every change of the focused window or element inside the frontmost one ([`track`]).
//! - `restore_focus`: activates the token's application and waits until its window is the
//!   focused one ([`restore`]).
//! - `allow_foreground`: nothing to do, the form and the palette are non-activating panels
//!   (§4.5 step 3); it succeeds.
//!
//! # Secure fields (KPI: 0 keys buffered)
//!
//! Two independent signals, either of which is enough:
//!
//! 1. **Secure keyboard entry.** While any process has it on, the system shows no keyboard
//!    event to any event tap, so the listener delivers nothing and the engine has nothing to
//!    buffer; `secure_state` reports it all the same, so the engine is suspended and the
//!    "why didn't it expand" log can say why.
//! 2. **The focused element.** A secure text field is reported as secure even when the
//!    application did not switch secure keyboard entry on.
//!
//! The engine is suspended from each report of the watcher until the verdict is "normal"
//! (§4.1), and stores nothing while suspended. How the KPI is measured:
//!
//! - end to end against the mock, on every system: `iamb-service`'s
//!   `secure_fields_excluded_apps_and_own_windows_suspend` types into a secure field, an
//!   excluded application and Iamb's own window and finds the buffer empty and nothing
//!   expanded;
//! - on macOS, `tests/focus.rs` shows that this module answers `Secure` for a focused
//!   `NSSecureTextField` and whenever secure keyboard entry is on, and `Normal` for a plain
//!   field: the answers the first test takes from the mock;
//! - by hand in real applications: `docs/compatibility-run.md` (matrix rows M11, M19, M20
//!   and the secure-field steps of the macOS section).
//!
//! # Threads
//!
//! | Thread | Does | May block on |
//! | --- | --- | --- |
//! | the caller of `secure_state` (the service's focus thread) | waits for the answer, at most `timeout` | nothing longer |
//! | `iamb-focus-inspect` | asks the main thread for the secure-entry flag and the focused application for its focused element | the application, at most until the caller's deadline |
//! | `iamb-focus-watch` | runs a run loop with the Accessibility observer of the frontmost application; calls the sink | the application, when it registers ([`WATCH_MESSAGE_TIMEOUT`] per message) |
//! | the main thread | receives the workspace's activation notification and wakes the watcher; answers the secure-entry flag | nothing |
//!
//! **The worker's main thread must run its run loop** (`run_with_main_loop`): the activation
//! notification is delivered there, and the secure-entry flag is a Carbon call, which the
//! system's header wants on the main thread (`listener::sys`). When it does not, `watch`
//! fails with an [`Os`](iamb_platform::PlatformError::Os) error that says so, and
//! `secure_state` answers `Unknown(Timeout)`: closed, not open.
//!
//! # Privacy
//!
//! - The Accessibility API is asked three things only: which element has the focus, its
//!   role and its subrole ([`field::Attribute`]). The value, title or selection of an
//!   element is never requested, so no typed text reaches this module.
//! - From the window server's list only the owner's process id, the layer and the window
//!   number are read; the window's name is not.
//! - Nothing here writes a log. What leaves the module is an application's bundle
//!   identifier, two numbers and one of three answers.
//!
//! # Limits
//!
//! - An application that neither switches secure keyboard entry on nor exposes its password
//!   field to the Accessibility API as a secure text field cannot be recognised. No such
//!   application is known; the user's exclusion list is the remedy.
//! - An application that does not expose a focused element at all (or when Accessibility is
//!   not granted) is `Unknown(NotInspectable)`: nothing expands there. That is the fail-closed
//!   rule of §4.1, and it is a compatibility cost.
//! - An application that sends no focus notifications (some browsers build their
//!   Accessibility tree only for an assistive client) gets a new verdict only when the
//!   application or window changes, or when secure keyboard entry ends. A move from one
//!   ordinary field to another needs none. A move into a password field is covered by
//!   secure keyboard entry itself.
//! - The token's window is the front ordinary window of the frontmost application as the
//!   window server orders it. A focused panel above the ordinary layer is reported with the
//!   window behind it. `restore_focus` cannot raise one particular window ([`restore`]).
//! - A non-activating panel does not change the frontmost application. While Iamb's own
//!   palette or form panel has the keyboard, this module still names the application behind
//!   it, so `own_window` (§4.5) cannot come from here for such a panel. Whoever shows the
//!   panel has to tell the service; that is open for tasks 3.4 and 3.5.

mod field;
mod identity;
mod inspect;
mod restore;
mod track;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod system;
#[cfg(target_os = "macos")]
mod watcher;

#[cfg(all(test, target_os = "macos"))]
mod mac_tests;

use std::time::Duration;

use iamb_platform::PermissionKind;

#[cfg(target_os = "macos")]
pub(crate) use mac::State;
#[cfg(target_os = "macos")]
pub(crate) use system::{frontmost_pid, secure_input};
#[cfg(target_os = "macos")]
pub(crate) use watcher::focus_notes;

use crate::permissions::policy::Probe;

/// The timeout of one message to the application while the watcher registers its observer
/// or reads which element has the focus.
pub(crate) const WATCH_MESSAGE_TIMEOUT: Duration = Duration::from_millis(100);

/// The shortest timeout ever given to an Accessibility message. The system reads a timeout
/// of zero as "use the default", which is several seconds.
pub(crate) const MIN_MESSAGE_TIMEOUT: Duration = Duration::from_millis(5);

/// The error context when the main thread did not answer `watch`.
pub(crate) const MAIN_RUN_LOOP_NOT_RUNNING: &str =
    "watching the frontmost application: the main thread is not running its run loop";

/// The permission the watcher cannot start without, if it is missing: without Accessibility
/// no application says which of its elements has the focus.
pub(crate) fn missing_for_watch(probe: &dyn Probe) -> Option<PermissionKind> {
    (!probe.accessibility()).then_some(PermissionKind::Accessibility)
}

/// The timeout, in seconds, to give an Accessibility message that may take `left`.
pub(crate) fn message_timeout(left: Duration) -> f32 {
    left.max(MIN_MESSAGE_TIMEOUT).as_secs_f32()
}

#[cfg(test)]
mod tests {
    use iamb_platform::{PlatformError, SECURE_STATE_TIMEOUT};

    use super::*;
    use crate::listener::start_gate;
    use crate::permissions::policy::testing::FakeProbe;

    #[test]
    fn the_watcher_needs_accessibility_and_nothing_else() {
        assert_eq!(missing_for_watch(&FakeProbe::new(true, true)), None);
        assert_eq!(missing_for_watch(&FakeProbe::new(true, false)), None);
        for input_monitoring in [false, true] {
            let probe = FakeProbe::new(false, input_monitoring);
            assert_eq!(
                missing_for_watch(&probe),
                Some(PermissionKind::Accessibility)
            );
            // Asking is not requesting.
            assert!(probe.calls().is_empty());
        }
    }

    #[test]
    fn a_watch_without_accessibility_is_refused_and_starts_nothing() {
        let mut slot: Option<u8> = None;
        let probe = FakeProbe::new(false, true);
        assert_eq!(
            start_gate(&mut slot, missing_for_watch(&probe), || panic!(
                "a refused watch must not start a thread"
            )),
            Err(PlatformError::PermissionDenied(
                PermissionKind::Accessibility
            ))
        );
        assert_eq!(slot, None);
    }

    #[test]
    fn a_second_watch_is_already_running() {
        let mut slot = None;
        let probe = FakeProbe::new(true, true);
        assert_eq!(
            start_gate(&mut slot, missing_for_watch(&probe), || Ok(1)),
            Ok(())
        );
        assert_eq!(
            start_gate(&mut slot, missing_for_watch(&probe), || Ok(2)),
            Err(PlatformError::AlreadyRunning)
        );
        assert_eq!(slot, Some(1));
    }

    #[test]
    fn a_message_timeout_is_never_zero() {
        assert!((message_timeout(Duration::ZERO) - 0.005).abs() < 1e-6);
        assert!((message_timeout(Duration::from_millis(1)) - 0.005).abs() < 1e-6);
        assert!((message_timeout(SECURE_STATE_TIMEOUT) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn the_watchers_messages_are_short() {
        // A hung application must not keep the watcher from reporting the next activation
        // for long.
        assert!(WATCH_MESSAGE_TIMEOUT <= Duration::from_millis(250));
        assert!(WATCH_MESSAGE_TIMEOUT >= MIN_MESSAGE_TIMEOUT);
    }
}
