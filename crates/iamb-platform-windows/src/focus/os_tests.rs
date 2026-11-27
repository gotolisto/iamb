//! Tests that need Windows: the hand-declared UI Automation method tables against the real
//! library, and the system calls against real controls.
//!
//! The public tests (`tests/focus_e2e.rs`) cannot show that UI Automation itself reports a
//! password field, because a system edit control is recognised by its style before UI
//! Automation is asked. Here the session is asked directly.
//!
//! They run in the Windows leg of CI. A test that needs something the session does not have
//! (a desktop that lets a window come to the front) says `SKIPPED` and why, and fails
//! instead when `IAMB_REQUIRE_DESKTOP` is set.

#[path = "../../tests/focus_support.rs"]
mod support;

use support::{Field, TestWindow, alone, eventually, skip};

use super::classify::{ControlKind, Observation, control_type};
use super::inspect::Probe;
use super::os::{self, SystemDesk};
use super::secure::{Desk, Front, Privilege};
use super::uia::Session;

/// The control type of a button (`UIA_ButtonControlTypeId`).
const BUTTON: i32 = 50000;

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

/// What the session observes, asked until it is `wanted` (the element with the focus
/// follows the control a moment later) or five seconds have passed.
fn observed(session: &mut Session, wanted: Observation) -> Observation {
    let mut last = Observation::Failed;
    eventually(|| {
        last = session.observe().observation;
        (last == wanted).then_some(())
    });
    last
}

#[test]
fn ui_automation_tells_a_password_field_from_a_text_field() {
    let _alone = alone();
    let Some(window) = open(Field::Plain) else {
        return;
    };
    let mut session = Session::open().expect("UI Automation is part of Windows");

    let plain = Observation::Element {
        password: false,
        control_type: control_type::EDIT,
        process: std::process::id(),
    };
    assert_eq!(observed(&mut session, plain), plain);

    for (field, wanted) in [
        (
            Field::Password,
            Observation::Element {
                password: true,
                control_type: control_type::EDIT,
                process: std::process::id(),
            },
        ),
        (
            Field::Button,
            Observation::Element {
                password: false,
                control_type: BUTTON,
                process: std::process::id(),
            },
        ),
        (Field::Plain, plain),
    ] {
        if !window.focus(field) {
            skip("the test window lost the foreground");
            return;
        }
        assert_eq!(observed(&mut session, wanted), wanted, "{field:?}");
    }
}

#[test]
fn a_session_can_be_opened_and_closed_repeatedly() {
    for _ in 0..3 {
        let session = Session::open();
        assert!(session.is_some());
    }
}

#[test]
fn the_system_tells_the_kind_of_the_focused_control() {
    let _alone = alone();
    let Some(window) = open(Field::Plain) else {
        return;
    };
    let front = os::foreground().expect("a window is in front");
    assert_eq!(
        front,
        Front {
            window: window.number(),
            pid: std::process::id(),
        }
    );
    let desk = SystemDesk;
    assert_eq!(desk.front(), Some(front));
    assert_eq!(desk.privilege(front.pid), Privilege::NotHigher);
    for (field, kind) in [
        (Field::Plain, ControlKind::PlainEdit),
        (Field::Password, ControlKind::PasswordEdit),
        (Field::Button, ControlKind::Other),
    ] {
        if !window.focus(field) {
            skip("the test window lost the foreground");
            return;
        }
        let control = desk.control(front);
        assert_eq!(control.kind, kind, "{field:?}");
        assert_eq!(control.pid, std::process::id(), "{field:?}");
        assert_ne!(control.window, 0, "{field:?}");
    }
}

#[test]
fn a_process_that_does_not_exist_is_not_trusted() {
    // Process ids are multiples of four.
    assert_eq!(SystemDesk.privilege(0xFFFF_FFF1), Privilege::Unknown);
}

#[test]
fn a_window_is_related_to_itself_and_its_controls() {
    let _alone = alone();
    let Some(window) = open(Field::Plain) else {
        return;
    };
    let front = os::foreground().expect("a window is in front");
    let own = os::relation(window.number(), front);
    assert!(!own.unnamed);
    assert!(own.same_process);
    assert!(own.same_root);
    // An event without a window cannot be ruled out.
    assert!(os::relation(0, front).unnamed);
    // A handle that names no window: the same.
    assert!(os::relation(0x7FFF_FFF1, front).unnamed);

    let identity = os::identity_of(front);
    assert_eq!(identity.pid, std::process::id());
    assert!(identity.own);
    let name = std::env::current_exe()
        .expect("the test executable has a path")
        .file_name()
        .expect("and a file name")
        .to_string_lossy()
        .to_lowercase();
    assert_eq!(identity.app.0, name);
}
