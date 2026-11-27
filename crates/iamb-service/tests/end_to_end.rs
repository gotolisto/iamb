//! The real worker threads end to end on the mock platform: mock listener → engine thread →
//! injector thread → mock text field, with the focus and store threads and the service
//! endpoint (through the mock's in-memory endpoint).

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{Harness, LIBRARY, T, diagnostic_codes, id, wait_event};
use iamb_core::model::{Abbreviation, Snippet, SyncMeta, Timestamp};
use iamb_ipc::{
    ErrorCode, EventBody, FocusRef, RequestBody, ResponseBody, Role, Secret, WireAction,
};
use iamb_platform::mock::{InjectionPoint, MockKey, MockWindow};
use iamb_platform::{FieldSecurity, Grant, KeyListener, LocalEndpoint, PermissionKind};
use iamb_service::worker::{ExitReason, MAX_CLIENTS, engine_buffer_region};
use iamb_store::Store;

#[test]
fn an_abbreviation_typed_through_the_listener_expands_in_the_field() {
    let h = Harness::start(LIBRARY);
    h.os.type_text("see you, brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "see you, be right back ");
    h.os.type_text("Thanks.\n;sig");
    h.settle();
    assert_eq!(
        h.text(h.editor),
        "see you, be right back Thanks.\nKind regards"
    );

    // Usage is recorded by the store thread once each injection finished.
    let dir = h.stop();
    let store = Store::open(dir.path()).expect("store free after stop");
    assert_eq!(store.usage(id(1)).expect("usage").map(|u| u.count), Some(1));
    assert_eq!(store.usage(id(4)).expect("usage").map(|u| u.count), Some(1));
    assert_eq!(store.usage(id(2)).expect("usage"), None);
}

#[test]
fn a_finished_expansion_is_announced_to_the_ui_and_a_cancelled_one_is_not() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ");
    // One expansion, one announcement (it names no snippet and carries no text).
    let seen = wait_event(&mut ui, |e| matches!(e, EventBody::ExpansionFinished {}));
    assert!(matches!(seen, EventBody::ExpansionFinished {}));

    // An expansion cancelled before its start marker never finished: nothing is announced.
    h.os.interleave(
        InjectionPoint::BeforeStartMarker,
        [
            MockKey::Char('a'),
            MockKey::Key(iamb_core::template::KeyName::Left),
        ],
    );
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back brb a");
    // (Other events, such as the diagnostic of the cancellation, may arrive; this one may not.)
    let mut heard = Vec::new();
    while let Some(event) = ui
        .next_event(Some(Duration::from_millis(300)))
        .expect("event stream")
    {
        heard.push(event);
    }
    assert!(
        !heard
            .iter()
            .any(|e| matches!(e, EventBody::ExpansionFinished {})),
        "a cancelled expansion must not be announced: {heard:?}"
    );
}

#[test]
fn many_expansions_follow_each_other() {
    let h = Harness::start(LIBRARY);
    let mut expected = String::new();
    for round in 0..40 {
        h.os.type_text(if round % 2 == 0 { "brb " } else { "omw." });
        h.settle();
        expected.push_str(if round % 2 == 0 {
            "be right back "
        } else {
            "on my way."
        });
    }
    assert_eq!(h.text(h.editor), expected);
}

#[test]
fn keys_typed_during_an_expansion_are_compensated() {
    let h = Harness::start(LIBRARY);
    h.os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("om"));
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back om");
    // The engine's buffer agrees with the field: finishing the word expands it.
    h.os.type_text("w ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back on my way ");

    // A key between the markers cannot be repaired: reported, and expansion continues.
    h.os.interleave(InjectionPoint::BeforeStep(1), MockKey::text("x"));
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back on my way xbe right back ");
    let mut c = h.client(Role::Ui);
    assert!(diagnostic_codes(&mut c).contains(&"typed_during_injection".to_owned()));
    assert_eq!(h.worker().handoff().len(), 0);
}

#[test]
fn a_boundary_before_the_start_marker_cancels_and_is_answered() {
    let h = Harness::start(LIBRARY);
    h.os.interleave(
        InjectionPoint::BeforeStartMarker,
        [
            MockKey::Char('a'),
            MockKey::Key(iamb_core::template::KeyName::Left),
        ],
    );
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb a");
    let mut c = h.client(Role::Ui);
    assert!(diagnostic_codes(&mut c).contains(&"injection_cancelled".to_owned()));
    // The engine left the flight: the next expansion works.
    h.os.press(iamb_core::template::KeyName::Right);
    h.os.type_text(" omw ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb a on my way ");
}

#[test]
fn declaring_the_test_field_again_keeps_what_is_being_typed() {
    let h = Harness::start(LIBRARY);
    let field = h.os.open_window(MockWindow::new("iamb-desktop", 300).own());
    h.os.focus(field).expect("open");
    h.worker().sync();
    let mut c = h.client(Role::Ui);
    let declare = |c: &mut iamb_ipc::Client| {
        let body = RequestBody::SetTestField {
            window: Some(FocusRef::from(field)),
        };
        assert!(matches!(c.call(body), Ok(ResponseBody::Done {})));
    };
    declare(&mut c);
    h.settle();
    h.os.type_text("br");
    h.settle();
    // The UI host declares the field again (it does every second) between two keys.
    declare(&mut c);
    h.settle();
    h.os.type_text("b ");
    h.settle();
    assert_eq!(h.text(field), "be right back ");
}

/// The entries of the diagnostics ring with `code`, as (app, detail).
fn entries_with(c: &mut iamb_ipc::Client, code: &str) -> Vec<(Option<String>, Option<String>)> {
    match c
        .call(RequestBody::RecentDiagnostics {})
        .expect("diagnostics")
    {
        ResponseBody::Diagnostics { entries } => entries
            .into_iter()
            .filter(|e| e.code == code)
            .map(|e| {
                (
                    e.app,
                    e.detail
                        .and_then(iamb_ipc::DiagnosticDetail::code)
                        .map(str::to_owned),
                )
            })
            .collect(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_first_look_at_another_window_waits_longer() {
    // An application that answers in 200 ms: too slow for 100 ms, in time for the first look
    // at its window (`FIRST_LOOK_TIMEOUT`, 300 ms).
    let h = Harness::start(LIBRARY);
    let slowish = h.os.open_window(
        MockWindow::new("org.example.slowish", 500).inspect_delay(Duration::from_millis(200)),
    );
    h.os.focus(slowish).expect("open");
    h.worker().sync();
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(slowish), "be right back ");
}

#[test]
fn an_unknown_focus_says_why_the_field_could_not_be_judged() {
    let h = Harness::start(LIBRARY);
    let admin =
        h.os.open_window(MockWindow::new("org.example.admin", 200).elevated());
    let slow = h.os.open_window(
        MockWindow::new("org.example.slow", 300).inspect_delay(Duration::from_millis(600)),
    );
    let bank =
        h.os.open_window(MockWindow::new("org.example.bank", 400).secure());
    for window in [admin, slow, bank] {
        h.os.focus(window).expect("open");
        h.worker().sync();
    }
    let mut c = h.client(Role::Ui);
    let unknown = entries_with(&mut c, "suspended_unknown_focus");
    assert!(
        unknown.contains(&(
            Some("org.example.admin".to_owned()),
            Some("elevated".to_owned())
        )),
        "{unknown:?}"
    );
    assert!(
        unknown.contains(&(
            Some("org.example.slow".to_owned()),
            Some("timeout".to_owned())
        )),
        "{unknown:?}"
    );
    // Only an unknown focus carries a detail.
    assert_eq!(
        entries_with(&mut c, "suspended_secure_field"),
        [(Some("org.example.bank".to_owned()), None)]
    );

    // The worker's log names the reason, once per window and reason, and nothing else: the
    // window judged again (an exclusion change) adds no line.
    h.os.focus(admin).expect("open");
    h.worker().sync();
    c.call(RequestBody::PutExclusion {
        app_id: "org.example.other".to_owned(),
        display_name: String::new(),
        excluded: true,
    })
    .expect("stored");
    h.worker().sync();
    let log = std::fs::read_to_string(h.dirs().logs().join("worker.log")).expect("log");
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("could not be judged"))
        .collect();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.ends_with("could not be judged: elevated"))
            .count(),
        2,
        "once at the first focus, once when it came back from another window: {lines:?}"
    );
    assert_eq!(
        lines.iter().filter(|l| l.ends_with(": timeout")).count(),
        1,
        "{lines:?}"
    );
    assert!(
        lines.iter().all(|l| !l.contains("org.example")),
        "{lines:?}"
    );
}

#[test]
fn secure_fields_excluded_apps_and_own_windows_suspend() {
    let h = Harness::start(LIBRARY);
    let bank =
        h.os.open_window(MockWindow::new("org.example.bank", 200).secure());
    let palette = h.os.open_window(MockWindow::new("iamb-desktop", 300).own());
    let slow = h.os.open_window(
        MockWindow::new("org.example.slow", 400).inspect_delay(Duration::from_millis(600)),
    );
    for window in [bank, palette, slow] {
        h.os.focus(window).expect("open");
        h.worker().sync();
        h.os.type_text("brb ");
        h.settle();
        assert_eq!(h.text(window), "brb ", "nothing expands there");
    }
    let mut c = h.client(Role::Ui);
    let codes = diagnostic_codes(&mut c);
    assert!(
        codes.contains(&"suspended_secure_field".to_owned()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"suspended_unknown_focus".to_owned()),
        "{codes:?}"
    );

    // A field of the same window turning into a password field suspends too.
    h.os.focus(h.editor).expect("open");
    h.os.set_security(h.editor, FieldSecurity::Secure);
    h.worker().sync();
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb ");
    h.os.set_security(h.editor, FieldSecurity::Normal);
    h.worker().sync();

    // An exclusion added over IPC applies to the focused app at once.
    assert!(matches!(
        c.call(RequestBody::PutExclusion {
            app_id: "org.example.editor".to_owned(),
            display_name: "Editor".to_owned(),
            excluded: true,
        }),
        Ok(ResponseBody::Exclusions { exclusions })
            if exclusions.len() == 1 && exclusions[0].app_id == "org.example.editor"
    ));
    h.worker().sync();
    h.os.type_text("omw ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb omw ");
    assert!(diagnostic_codes(&mut c).contains(&"suspended_excluded_app".to_owned()));
    match c.call(RequestBody::ListExclusions {}) {
        Ok(ResponseBody::Exclusions { exclusions }) => {
            assert_eq!(exclusions.len(), 1);
            assert_eq!(exclusions[0].app_id, "org.example.editor");
        }
        other => panic!("unexpected {other:?}"),
    }
    c.call(RequestBody::PutExclusion {
        app_id: "org.example.editor".to_owned(),
        display_name: String::new(),
        excluded: false,
    })
    .expect("removed");
    h.worker().sync();
    h.os.type_text("omw ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb omw on my way ");
}

#[test]
fn a_palette_pick_waits_for_the_focus_of_its_window() {
    // §4.5 order duty: the pick arrives over IPC while the palette (an own window) still has
    // the focus; the service forwards it only after the focus is back in the editor.
    let h = Harness::start(LIBRARY);
    h.os.type_text("Note: ");
    let palette = h.os.open_window(MockWindow::new("iamb-desktop", 300).own());
    h.os.focus(palette).expect("open");
    h.worker().sync();

    let mut ui = h.client(Role::Ui);
    assert_eq!(
        ui.call(RequestBody::ExpandSnippet {
            id: id(2),
            focus: Some(FocusRef::from(h.editor)),
        }),
        Ok(ResponseBody::Done {})
    );
    h.settle();
    assert_eq!(
        h.text(h.editor),
        "Note: ",
        "parked until the editor has the focus"
    );
    assert_eq!(h.text(palette), "");

    h.os.focus(h.editor).expect("open");
    h.settle();
    assert_eq!(h.text(h.editor), "Note: on my way");
    assert_eq!(h.text(palette), "", "nothing typed into the palette");
}

#[test]
fn a_pick_for_a_window_that_never_gets_the_focus_is_dropped() {
    let h = Harness::start(LIBRARY);
    let other = h.os.open_window(MockWindow::new("org.example.chat", 500));
    let mut ui = h.client(Role::Ui);
    ui.call(RequestBody::ExpandSnippet {
        id: id(2),
        focus: Some(FocusRef::from(other)),
    })
    .expect("accepted");
    std::thread::sleep(Duration::from_millis(1300));
    h.settle();
    assert_eq!(h.text(h.editor), "");
    assert_eq!(h.text(other), "");
    assert!(diagnostic_codes(&mut ui).contains(&"request_focus_timeout".to_owned()));
}

#[test]
fn a_request_without_a_window_types_at_the_focus_even_while_paused() {
    let h = Harness::start(LIBRARY);
    let mut cli = h.client(Role::Cli);
    match cli.call(RequestBody::SetPaused { paused: true }) {
        Ok(ResponseBody::Status { status }) => assert!(status.paused),
        other => panic!("unexpected {other:?}"),
    }
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb ", "pause blocks typed abbreviations");
    cli.call(RequestBody::ExpandSnippet {
        id: id(2),
        focus: None,
    })
    .expect("accepted");
    h.settle();
    assert_eq!(
        h.text(h.editor),
        "brb on my way",
        "but not a deliberate pick"
    );
    cli.call(RequestBody::SetPaused { paused: false })
        .expect("unpaused");
    h.os.type_text(" brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb on my way be right back ");
}

#[test]
fn a_form_is_requested_from_the_ui_host_and_submitted() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    h.os.type_text("hi ");
    let (session, focus) =
        match wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. })) {
            EventBody::FormRequested {
                session,
                snippet,
                spec,
                focus,
                last_values,
            } => {
                assert_eq!(snippet, id(3));
                assert_eq!(spec.len(), 1);
                assert_eq!(spec[0].name, "Name");
                assert!(last_values.is_none());
                (session, focus)
            }
            _ => unreachable!("matched above"),
        };
    assert_eq!(focus, Some(FocusRef::from(h.editor)));
    ui.call(RequestBody::FormAck { session }).expect("ack");
    ui.call(RequestBody::FormSubmit {
        session,
        values: Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())])),
    })
    .expect("submit");
    h.settle();
    assert_eq!(h.text(h.editor), "Hello Ada! ");

    // The values are remembered for the next form of the same snippet.
    h.os.type_text("hi ");
    match wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. })) {
        EventBody::FormRequested {
            session,
            last_values,
            ..
        } => {
            let values = last_values.expect("remembered");
            assert_eq!(values.expose().get("Name").map(String::as_str), Some("Ada"));
            ui.call(RequestBody::FormCancel { session })
                .expect("cancel");
        }
        _ => unreachable!("matched above"),
    }
    h.settle();
    assert_eq!(h.text(h.editor), "Hello Ada! hi ");
}

#[test]
fn a_form_acknowledged_by_a_ui_host_that_then_goes_is_cancelled() {
    let h = Harness::start(LIBRARY);
    {
        let mut ui = h.client(Role::Ui);
        h.os.type_text("hi ");
        let EventBody::FormRequested { session, .. } =
            wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. }))
        else {
            unreachable!("matched above")
        };
        ui.call(RequestBody::FormAck { session }).expect("ack");
        // The UI host goes with the form open.
    }
    std::thread::sleep(Duration::from_millis(200));
    h.settle();
    // Without the cancel, the engine refuses every typed match with `form_open` from here on.
    h.os.type_text("brb ");
    h.settle();
    assert!(
        h.text(h.editor).ends_with("be right back "),
        "{:?}",
        h.text(h.editor)
    );
    let mut ui = h.client(Role::Ui);
    let codes = diagnostic_codes(&mut ui);
    assert!(codes.contains(&"form_ui_host_gone".to_owned()), "{codes:?}");
    assert!(!codes.contains(&"form_open".to_owned()), "{codes:?}");
}

#[test]
fn a_form_with_no_ui_host_or_no_acknowledgement_is_abandoned() {
    let h = Harness::start(LIBRARY);
    let mut cli = h.client(Role::Cli);
    h.os.type_text("hi ");
    h.settle();
    assert_eq!(h.text(h.editor), "hi ");
    // The ring is for the UI; read it through a UI connection opened afterwards.
    drop(cli.call(RequestBody::GetStatus {}));
    let mut ui = h.client(Role::Ui);
    assert!(diagnostic_codes(&mut ui).contains(&"form_no_ui_host".to_owned()));

    // A UI host that never acknowledges: abandoned after 2 s.
    h.os.type_text("hi ");
    let EventBody::FormRequested { session, .. } =
        wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. }))
    else {
        unreachable!("matched above")
    };
    std::thread::sleep(Duration::from_millis(2300));
    h.settle();
    assert!(diagnostic_codes(&mut ui).contains(&"form_not_acknowledged".to_owned()));
    // A late submit belongs to no open form: nothing is typed.
    ui.call(RequestBody::FormSubmit {
        session,
        values: Secret::new(BTreeMap::new()),
    })
    .expect("accepted");
    h.settle();
    assert_eq!(h.text(h.editor), "hi hi ");
}

#[test]
fn library_writes_rebuild_the_snapshot_and_notify() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let (profile, folder) = match ui.call(RequestBody::ListFolders {}) {
        Ok(ResponseBody::Folders { profile, folders }) => (profile, folders[0].meta.id),
        other => panic!("unexpected {other:?}"),
    };
    assert_ne!(profile, folder);
    let ty = Snippet::new_defaults(
        SyncMeta::new(id(9), Timestamp(50)),
        folder,
        Abbreviation::new("ty").expect("abbreviation"),
        "",
        "thank you",
        None,
    );
    match ui.call(RequestBody::PutSnippet {
        snippet: ty,
        tags: None,
    }) {
        Ok(ResponseBody::Saved { meta }) => assert_eq!(meta.id, id(9)),
        other => panic!("unexpected {other:?}"),
    }
    wait_event(&mut ui, |e| {
        matches!(e, EventBody::LibraryChanged { revision: 2, .. })
    });
    h.settle();
    h.os.type_text("ty ");
    h.settle();
    assert_eq!(h.text(h.editor), "thank you ");

    // A conflicting abbreviation is refused by the store.
    let clash = Snippet::new_defaults(
        SyncMeta::new(id(10), Timestamp(60)),
        folder,
        Abbreviation::new("BRB").expect("abbreviation"),
        "",
        "x",
        None,
    );
    assert_eq!(
        ui.call(RequestBody::PutSnippet {
            snippet: clash,
            tags: None,
        })
        .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::Conflict))
    );
    match ui.call(RequestBody::ListSnippets {
        query: iamb_ipc::SnippetQuery::default(),
    }) {
        Ok(ResponseBody::Snippets { snippets, .. }) => assert_eq!(snippets.len(), 5),
        other => panic!("unexpected {other:?}"),
    }
    match ui.call(RequestBody::Search {
        text: Secret::new("thank".to_owned()),
        limit: 5,
    }) {
        Ok(ResponseBody::SearchResults { hits }) => assert_eq!(hits[0].snippet, id(9)),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        ui.call(RequestBody::DeleteSnippet { id: id(9) }),
        Ok(ResponseBody::Done {})
    );
    assert_eq!(
        ui.call(RequestBody::GetSnippet { id: id(9) })
            .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::NotFound))
    );
}

#[test]
fn settings_are_validated_and_applied() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(
        ui.call(RequestBody::PutSetting {
            key: "date_format".to_owned(),
            value: Some("%Q%".to_owned()),
        })
        .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::Invalid))
    );
    ui.call(RequestBody::PutSetting {
        key: "delimiters".to_owned(),
        value: Some("!".to_owned()),
    })
    .expect("stored");
    wait_event(&mut ui, |e| matches!(e, EventBody::SettingsChanged {}));
    h.settle();
    h.os.type_text("brb!");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back!");
    match ui.call(RequestBody::GetSettings {}) {
        Ok(ResponseBody::Settings { settings }) => {
            assert_eq!(settings.get("delimiters").map(String::as_str), Some("!"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_dry_run_is_answered_from_the_snapshot() {
    let h = Harness::start(LIBRARY);
    let mut cli = h.client(Role::Cli);
    match cli.call(RequestBody::DryRun {
        typed: Secret::new("brb ".to_owned()),
        now: None,
        clipboard: None,
        values: None,
    }) {
        Ok(ResponseBody::DryRun { actions, hints }) => {
            assert!(hints.is_empty());
            assert_eq!(actions[0], WireAction::DeleteBack { n: 4 });
            assert!(
                matches!(&actions[1], WireAction::TypeText { text, .. } if text == "be right back")
            );
            assert!(matches!(actions.last(), Some(WireAction::Expanded { .. })));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(h.text(h.editor), "", "nothing is injected");
}

#[test]
fn roles_are_enforced_and_missing_counterparts_say_so() {
    let h = Harness::start(LIBRARY);
    let mut cli = h.client(Role::Cli);
    assert_eq!(
        cli.call(RequestBody::RecentDiagnostics {})
            .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::NotPermitted))
    );
    assert_eq!(
        cli.call(RequestBody::FormCancel { session: 1 })
            .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::NotPermitted))
    );
    let mut sync = h.client(Role::Sync);
    assert_eq!(
        sync.call(RequestBody::ListFolders {})
            .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::NotPermitted))
    );
    // The Sync group is answered (task 4.4).
    let answer = sync.call(RequestBody::OutboxRead {
        limit: 1,
        phase: None,
    });
    assert!(
        matches!(answer, Ok(ResponseBody::Changes { .. })),
        "{answer:?}"
    );
    // Only the UI host may say that one of its panels has the keyboard.
    assert_eq!(
        cli.call(RequestBody::PanelFocus {
            panel: iamb_ipc::PanelKind::Palette,
            shown: true
        })
        .map_err(|e| e.remote_code()),
        Err(Some(ErrorCode::NotPermitted))
    );
    let mut ui = h.client(Role::Ui);
    // The editor's group is answered (task 3.4): no longer `not_implemented`.
    assert!(matches!(
        ui.call(RequestBody::ParseTemplate {
            source: String::new()
        }),
        Ok(ResponseBody::TemplateTokens { error: None, .. })
    ));
    match ui.call(RequestBody::GetStatus {}) {
        Ok(ResponseBody::Status { status }) => {
            assert_eq!(status.clients, 3);
            assert!(status.ui_connected);
            assert!(status.listener);
            assert!(status.memory_locked);
            assert_eq!(status.snippets, 4);
        }
        other => panic!("unexpected {other:?}"),
    }
    match ui.call(RequestBody::GetPermissions {}) {
        Ok(ResponseBody::Permissions { accessibility, .. }) => {
            assert_eq!(accessibility, iamb_ipc::GrantState::Granted);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn at_most_eight_clients_are_served() {
    let h = Harness::start(LIBRARY);
    let clients: Vec<_> = (0..MAX_CLIENTS).map(|_| h.client(Role::Cli)).collect();
    let stream =
        h.os.connect(
            &h.dirs().service_endpoint(),
            iamb_platform::PeerPolicy::SameUser,
            T,
        )
        .expect("connects");
    assert!(iamb_ipc::Client::handshake(stream, Role::Cli, "x", T).is_err());
    drop(clients);
    // Slots are freed when clients leave.
    std::thread::sleep(Duration::from_millis(200));
    let mut again = h.client(Role::Cli);
    again.call(RequestBody::GetStatus {}).expect("served");
}

#[test]
fn the_key_queue_the_keystroke_buffer_and_the_handoff_are_locked() {
    let h = Harness::start(LIBRARY);
    let locked = h.os.locked_regions();
    assert_eq!(locked.len(), 3);
    assert!(locked.contains(&h.worker().handoff().region()));
    let queue = h.worker().key_queue_region();
    assert_eq!(queue.len(), 256 * size_of::<char>());
    assert!(locked.contains(&queue));
    let engine = iamb_core::engine::Engine::new(iamb_core::engine::Snapshot::empty());
    let region = engine_buffer_region(&engine);
    assert!(!region.is_empty());
    assert!(locked.iter().any(|r| r.len() == region.len()));
}

#[test]
fn a_dead_hook_is_reinstalled_and_a_lasting_one_ends_the_worker() {
    let h = Harness::start_with(LIBRARY, |c| {
        c.self_test_interval = Duration::from_millis(100);
    });
    h.os.kill_hook();
    std::thread::sleep(Duration::from_millis(350));
    // Reinstalled: typing works again.
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ");
    assert!(h.worker().wait_exit(Some(Duration::ZERO)).is_none());

    // A hook that cannot be reinstalled: two failures, and the worker asks for a restart.
    h.os.set_permission(PermissionKind::InputMonitoring, Grant::Denied);
    h.os.kill_hook();
    assert_eq!(
        h.worker().wait_exit(Some(T)),
        Some(ExitReason::ListenerDead)
    );
    let _ = h.os.self_test(Duration::from_millis(1));
}
