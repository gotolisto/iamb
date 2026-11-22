//! Onboarding against a real worker on the mock platform, reached through the bridge as the
//! onboarding window's commands reach it (task 3.7).
//!
//! - Permissions: missing, then granted, are what `GetPermissions` says; the settings page opens
//!   through the worker.
//! - The test field: typing the starter abbreviation into it expands it; before it is declared,
//!   and after it is cleared, typing into Iamb's own window does not; another process's window
//!   cannot be declared.
//! - The end: it sets `ui.onboarding_done`, stores the default search shortcut (or reports that
//!   the system refused it), and the window does not open at the next start.
//! - The scripted timing (`scripted_first_run_timing`): the steps automated on the mock platform,
//!   from the window opening to the first expansion; `docs/test-strategy.md` records the result.
//!   No test puts typed text into a log.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::{Endpoint, Service, problems};
use iamb_desktop::library::{self, SnippetDraft};
use iamb_desktop::onboarding::{
    self, DEFAULT_PALETTE_CHORD, DONE_KEY, Grant, Host, LoginView, NoSupervisor, Supervisor,
};
use iamb_desktop::settings;
use iamb_platform::mock::MockWindow;
use iamb_platform::{Chord, FocusToken, Grant as PlatformGrant, LoginItemStatus, PermissionKind};

/// The process id of the pretend UI host.
const OWN_PID: u32 = 4242;
const EMAIL: &str = "ines@brindlewood.example";
/// Text that must never reach a log.
const CANARY: &str = "zqxonboardingcanary";

fn host(service: &Service) -> Host {
    Host {
        focus: Some(Arc::new(service.os.clone())),
        login: Some(Arc::new(service.os.clone())),
        supervisor: Arc::new(NoSupervisor),
        pid: OWN_PID,
    }
}

/// The onboarding window, as the platform sees it: one of Iamb's own windows, with the keyboard.
fn open_onboarding_window(service: &Service) -> FocusToken {
    let window = service
        .os
        .open_window(MockWindow::new("iamb-desktop", OWN_PID).own());
    service.os.focus(window).expect("window is open");
    window
}

/// What the First snippet step saves.
fn starter(folder: &str, abbreviation: &str, body: &str) -> SnippetDraft {
    SnippetDraft {
        id: None,
        folder_id: folder.to_owned(),
        abbreviation: abbreviation.to_owned(),
        label: body.to_owned(),
        kind: "plain".to_owned(),
        body: body.to_owned(),
        trigger: "delimiter".to_owned(),
        case_mode: "exact".to_owned(),
        word_start: true,
        enabled: true,
        tags: vec![],
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, iamb_desktop::contract::CommandError>) -> String {
    r.expect_err("an error").code
}

#[test]
fn missing_permissions_become_granted_and_the_step_can_see_it() {
    let service = Service::start(Endpoint::Mock);
    service
        .os
        .set_permission(PermissionKind::InputMonitoring, PlatformGrant::Denied);
    service
        .os
        .set_permission(PermissionKind::Accessibility, PlatformGrant::Denied);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    let p = onboarding::permissions(&bridge).expect("permissions");
    assert_eq!(p.input_monitoring, Grant::Denied);
    assert_eq!(p.accessibility, Grant::Denied);
    assert!(p.missing());
    assert!(!onboarding::menu_offers_fix(false, &p), "setup is the way");
    assert!(onboarding::menu_offers_fix(true, &p));

    // One at a time, as the person flips the switches; the worker's answer follows each.
    service
        .os
        .set_permission(PermissionKind::InputMonitoring, PlatformGrant::Granted);
    let p = onboarding::permissions(&bridge).expect("permissions");
    assert_eq!(
        (p.input_monitoring, p.accessibility),
        (Grant::Granted, Grant::Denied)
    );
    service
        .os
        .set_permission(PermissionKind::Accessibility, PlatformGrant::Granted);
    let p = onboarding::permissions(&bridge).expect("permissions");
    assert!(!p.missing());
    assert!(!onboarding::menu_offers_fix(true, &p));
    bridge.stop();
}

#[test]
fn windows_needs_no_permission() {
    let service = Service::start(Endpoint::Mock);
    service
        .os
        .set_permission(PermissionKind::InputMonitoring, PlatformGrant::NotRequired);
    service
        .os
        .set_permission(PermissionKind::Accessibility, PlatformGrant::NotRequired);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let p = onboarding::permissions(&bridge).expect("permissions");
    assert_eq!(p.input_monitoring, Grant::NotRequired);
    assert!(!p.missing());
    bridge.stop();
}

#[test]
fn the_settings_page_of_each_permission_opens_through_the_worker() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    onboarding::open_permission_settings(&bridge, "input_monitoring").expect("opens");
    onboarding::open_permission_settings(&bridge, "accessibility").expect("opens");
    assert_eq!(
        service.os.opened_settings(),
        [
            PermissionKind::InputMonitoring,
            PermissionKind::Accessibility
        ]
    );
    assert_eq!(
        code(onboarding::open_permission_settings(&bridge, "camera")),
        "invalid"
    );
    bridge.stop();
}

#[test]
fn typing_the_starter_abbreviation_into_the_test_field_expands_it() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    library::save_snippet(&bridge, &starter(&folder, ";em", EMAIL)).expect("saved");
    let window = open_onboarding_window(&service);
    let host = host(&service);

    // Before the step declares it, typing into Iamb's own window expands nothing.
    service.type_text(";em ");
    assert_eq!(service.os.text(window).as_deref(), Some(";em "));

    host.declare(&bridge).expect("declared");
    service.worker().settle();
    // The same abbreviation again, now that the field is declared.
    service.type_text(";em ");
    let text = service.os.text(window).expect("open");
    assert!(
        text.ends_with(&format!("{EMAIL} ")),
        "the test field expanded it: {text:?}"
    );
    assert!(!text.ends_with(";em "), "{text:?}");
    bridge.stop();
}

#[test]
fn the_field_stops_expanding_once_it_is_cleared() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    library::save_snippet(&bridge, &starter(&folder, ";em", EMAIL)).expect("saved");
    let window = open_onboarding_window(&service);
    let host = host(&service);

    host.declare(&bridge).expect("declared");
    service.worker().settle();
    service.type_text(";em ");
    assert_eq!(
        service.os.text(window).as_deref(),
        Some(&*format!("{EMAIL} "))
    );

    // Leaving the step (or closing the window) withdraws it.
    onboarding::clear_test_field(&bridge).expect("cleared");
    service.worker().settle();
    service.type_text(";em ");
    let text = service.os.text(window).expect("open");
    assert!(text.ends_with(";em "), "nothing expanded: {text:?}");
    assert_eq!(text.matches(EMAIL).count(), 1, "{text:?}");
    bridge.stop();
}

#[test]
fn only_a_window_of_this_process_can_be_the_test_field() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    // The editor of another application has the keyboard: the command refuses.
    assert_eq!(code(host(&service).declare(&bridge)), "not_permitted");
    service.type_text("brb ");
    assert_eq!(service.text(), "be right back ");
    // A machine whose platform cannot name the focused window says so.
    let none = Host {
        focus: None,
        ..host(&service)
    };
    assert_eq!(code(none.declare(&bridge)), "unavailable");
    bridge.stop();
}

#[test]
fn the_test_field_does_not_open_other_own_windows() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    library::save_snippet(&bridge, &starter(&folder, ";em", EMAIL)).expect("saved");
    let onboarding_window = open_onboarding_window(&service);
    host(&service).declare(&bridge).expect("declared");
    // Another window of Iamb (the library's editor) takes the keyboard: still not expanded.
    let library_window = service
        .os
        .open_window(MockWindow::new("iamb-desktop", OWN_PID).own());
    service.os.focus(library_window).expect("open");
    service.worker().settle();
    service.type_text(";em ");
    assert_eq!(service.os.text(library_window).as_deref(), Some(";em "));
    service.os.focus(onboarding_window).expect("open");
    service.worker().settle();
    service.type_text(";em ");
    assert!(
        service
            .os
            .text(onboarding_window)
            .expect("open")
            .contains(EMAIL)
    );
    bridge.stop();
}

#[test]
fn finishing_sets_done_and_the_window_stays_away_at_the_next_start() {
    let mut service = Service::start(Endpoint::Mock);
    {
        let (bridge, seen) = service.bridge();
        seen.wait_connected();
        let done = onboarding::is_done(&bridge).expect("settings");
        assert!(!done);
        assert!(onboarding::should_open_at_start(done, false), "first start");
        assert!(onboarding::menu_offers_setup(done));

        let view = onboarding::finish(&bridge).expect("finished");
        assert_eq!(view.palette_hotkey.as_deref(), Some(DEFAULT_PALETTE_CHORD));
        assert!(!view.hotkey_refused);
        assert_eq!(
            settings::settings(&bridge)
                .expect("settings")
                .get(DONE_KEY)
                .map(String::as_str),
            Some("1")
        );
        // Reaching the last step again changes nothing.
        let again = onboarding::finish(&bridge).expect("finished again");
        assert_eq!(again, view);
        bridge.stop();
    }
    service.restart();
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let done = onboarding::is_done(&bridge).expect("settings");
    assert!(done, "it survives a restart");
    assert!(!onboarding::should_open_at_start(done, false));
    assert!(!onboarding::menu_offers_setup(done), "no more Set up Iamb");
    bridge.stop();
}

#[test]
fn a_service_that_comes_back_does_not_bring_the_window_back() {
    // Not done, and it was already offered at this start (the person closed it).
    assert!(!onboarding::should_open_at_start(false, true));
}

#[test]
fn a_shortcut_the_system_refuses_is_reported_and_onboarding_is_still_done() {
    let service = Service::start(Endpoint::Mock);
    service
        .os
        .reserve_chord(Chord::parse(DEFAULT_PALETTE_CHORD).expect("chord"));
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let view = onboarding::finish(&bridge).expect("finished");
    assert!(view.hotkey_refused);
    assert_eq!(view.palette_hotkey, None);
    assert!(onboarding::is_done(&bridge).expect("settings"));
    // The recorder's choice is stored as any other shortcut.
    settings::put_setting(&bridge, "palette_hotkey", Some("Ctrl+Shift+J")).expect("stored");
    let view = onboarding::finish(&bridge).expect("finished");
    assert_eq!(view.palette_hotkey.as_deref(), Some("Ctrl+Shift+J"));
    bridge.stop();
}

#[test]
fn a_shortcut_that_is_already_set_is_kept() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    settings::put_setting(&bridge, "palette_hotkey", Some("Ctrl+Shift+K")).expect("stored");
    let view = onboarding::finish(&bridge).expect("finished");
    assert_eq!(view.palette_hotkey.as_deref(), Some("Ctrl+Shift+K"));
    bridge.stop();
}

#[test]
fn finishing_withdraws_the_test_field() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    library::save_snippet(&bridge, &starter(&folder, ";em", EMAIL)).expect("saved");
    let window = open_onboarding_window(&service);
    host(&service).declare(&bridge).expect("declared");
    onboarding::finish(&bridge).expect("finished");
    service.worker().settle();
    service.type_text(";em ");
    assert_eq!(service.os.text(window).as_deref(), Some(";em "));
    bridge.stop();
}

#[test]
fn start_at_login_is_the_login_item_of_the_platform() {
    let service = Service::start(Endpoint::Mock);
    let host = host(&service);
    assert_eq!(host.login_status().expect("status"), LoginView::Disabled);
    assert_eq!(host.set_login(true).expect("set"), LoginView::Enabled);
    assert_eq!(host.set_login(false).expect("set"), LoginView::Disabled);
    // macOS asks the person to approve it once.
    service
        .os
        .set_login_item_status(LoginItemStatus::RequiresApproval);
    assert_eq!(
        host.set_login(true).expect("set"),
        LoginView::RequiresApproval
    );
    // A damaged installation cannot register it.
    service.os.set_login_item_status(LoginItemStatus::NotFound);
    assert_eq!(code(host.set_login(true)), "unavailable");
    assert_eq!(host.login_status().expect("status"), LoginView::Unavailable);
    // A system without login items.
    let none = Host {
        login: None,
        ..host.clone()
    };
    assert_eq!(none.login_status().expect("status"), LoginView::Unavailable);
    assert_eq!(code(none.set_login(true)), "unavailable");
}

#[test]
fn the_helper_cannot_be_restarted_until_the_supervisor_can_be_asked() {
    assert_eq!(code(NoSupervisor.restart_worker()), "unavailable");
}

#[test]
fn the_commands_log_nothing_the_person_typed() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    library::save_snippet(&bridge, &starter(&folder, ";em", CANARY)).expect("saved");
    let window = open_onboarding_window(&service);
    host(&service).declare(&bridge).expect("declared");
    service.worker().settle();
    service.type_text(&format!(";em {CANARY} "));
    assert!(service.os.text(window).expect("open").contains(CANARY));
    onboarding::finish(&bridge).expect("finished");
    bridge.stop();
    let logs = service.worker_log();
    let desktop =
        std::fs::read_to_string(service.dirs().logs().join("desktop.log")).unwrap_or_default();
    for log in [&logs, &desktop] {
        assert!(!log.contains(CANARY), "typed text reached a log");
        assert!(!log.contains(EMAIL), "typed text reached a log");
    }
    assert!(problems(&logs).is_empty(), "{:?}", problems(&logs));
}

/// The steps automated on the mock platform, from the moment the window opens (the service
/// answered and onboarding is not done) to the first expansion in the test field: read the
/// permissions, grant them, save the first snippet, declare the field, type the abbreviation and
/// a space, and read the expansion. The number is a sanity check on the service's side of the
/// steps, not the KPI: that needs people on fresh machines (`docs/onboarding-walkthrough.md`).
#[test]
fn scripted_first_run_timing() {
    let service = Service::start(Endpoint::Mock);
    service
        .os
        .set_permission(PermissionKind::InputMonitoring, PlatformGrant::Denied);
    service
        .os
        .set_permission(PermissionKind::Accessibility, PlatformGrant::Denied);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let host = host(&service);

    let opened = Instant::now();
    let window = open_onboarding_window(&service);
    let mut marks = vec![("window opens", opened.elapsed())];
    let done = onboarding::is_done(&bridge).expect("settings");
    assert!(onboarding::should_open_at_start(done, false));

    // Step 2: both permissions.
    assert!(
        onboarding::permissions(&bridge)
            .expect("permissions")
            .missing()
    );
    onboarding::open_permission_settings(&bridge, "input_monitoring").expect("opens");
    service
        .os
        .set_permission(PermissionKind::InputMonitoring, PlatformGrant::Granted);
    onboarding::open_permission_settings(&bridge, "accessibility").expect("opens");
    service
        .os
        .set_permission(PermissionKind::Accessibility, PlatformGrant::Granted);
    assert!(
        !onboarding::permissions(&bridge)
            .expect("permissions")
            .missing()
    );
    marks.push(("permissions granted", opened.elapsed()));

    // Step 3: the first snippet.
    let folder = library::folders(&bridge).expect("folders").folders[0]
        .id
        .clone();
    library::save_snippet(&bridge, &starter(&folder, ";em", EMAIL)).expect("saved");
    marks.push(("first snippet saved", opened.elapsed()));

    // Step 4: the test field, the abbreviation and a space.
    host.declare(&bridge).expect("declared");
    service.worker().settle();
    marks.push(("test field declared", opened.elapsed()));
    service.type_text(";em ");
    let text = service.os.text(window).expect("open");
    marks.push(("first expansion", opened.elapsed()));
    assert_eq!(text, format!("{EMAIL} "));

    // Step 5.
    onboarding::finish(&bridge).expect("finished");
    marks.push(("onboarding done", opened.elapsed()));
    bridge.stop();

    for (what, at) in &marks {
        eprintln!(
            "onboarding timing: {what:>22}: {:>8.1} ms",
            at.as_secs_f64() * 1000.0
        );
    }
    let first_expansion = marks
        .iter()
        .find(|(what, _)| *what == "first expansion")
        .map(|(_, at)| *at)
        .expect("measured");
    // The people's budget is two minutes; the automated steps must use a tiny part of it.
    assert!(
        first_expansion.as_secs() < 5,
        "the automated steps took {first_expansion:?}"
    );
}
