//! Iamb's own panels have the keyboard (architecture §4.5; the 2.8 review's finding 16), and a
//! screen lock or sign-out forgets the remembered form values (finding 13), on the mock.
//!
//! On the mock a key always reaches the focused window's field, as on macOS a key typed into a
//! non-activating panel leaves the frontmost application the focused one: the focus thread's
//! verdict stays `normal`, and only `PanelFocus` tells the service where the key went.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{Harness, LIBRARY, diagnostic_codes, id, wait_event};
use iamb_ipc::{
    Client, EventBody, FocusRef, PanelKind, RequestBody, ResponseBody, Role, Secret, WireDiagnostic,
};
use iamb_platform::SessionEvent;
use iamb_platform::mock::MockWindow;

fn panel(ui: &mut Client, panel: PanelKind, shown: bool) {
    match ui.call(RequestBody::PanelFocus { panel, shown }) {
        Ok(ResponseBody::Done {}) => {}
        other => panic!("unexpected {other:?}"),
    }
}

fn diagnostics(ui: &mut Client) -> Vec<WireDiagnostic> {
    match ui
        .call(RequestBody::RecentDiagnostics {})
        .expect("diagnostics")
    {
        ResponseBody::Diagnostics { entries } => entries,
        other => panic!("unexpected {other:?}"),
    }
}

fn suspended(ui: &mut Client) -> bool {
    match ui.call(RequestBody::GetStatus {}).expect("status") {
        ResponseBody::Status { status } => status.suspended,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn keys_typed_while_a_panel_has_the_keyboard_never_reach_the_buffer() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    panel(&mut ui, PanelKind::Palette, true);
    assert!(suspended(&mut ui), "held in own_window");
    // Typed into the palette's query field. Had they been buffered, the "b" typed after the
    // palette closed would complete "brb" and expand.
    h.os.type_text("br");
    h.settle();
    panel(&mut ui, PanelKind::Palette, false);
    assert!(!suspended(&mut ui), "the field's verdict is back");
    h.os.type_text("b ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb ", "nothing expanded");
    // Typing goes on as before.
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb be right back ");
}

#[test]
fn an_abbreviation_typed_into_a_panel_is_neither_expanded_nor_handed_off() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    panel(&mut ui, PanelKind::Form, true);
    h.os.type_text("brb omw. ;sig");
    h.settle();
    assert_eq!(h.text(h.editor), "brb omw. ;sig", "nothing expanded");
    assert!(h.os.injected().is_empty(), "the injector never ran");
    assert_eq!(h.worker().handoff().len(), 0);
    // No diagnostic names a snippet: nothing was matched.
    let named: Vec<WireDiagnostic> = diagnostics(&mut ui)
        .into_iter()
        .filter(|d| d.snippet.is_some())
        .collect();
    assert!(named.is_empty(), "{named:?}");
    // A palette pick while the form has the keyboard types into Iamb's own window: refused.
    ui.call(RequestBody::ExpandSnippet {
        id: id(1),
        focus: Some(FocusRef::from(h.editor)),
    })
    .expect("sent");
    h.settle();
    assert_eq!(h.text(h.editor), "brb omw. ;sig");
    assert!(diagnostic_codes(&mut ui).contains(&"refused_own_window".to_owned()));
}

#[test]
fn a_request_sent_after_the_panel_gave_the_keyboard_back_runs() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    // The palette: hidden, then the pick.
    panel(&mut ui, PanelKind::Palette, true);
    panel(&mut ui, PanelKind::Palette, false);
    ui.call(RequestBody::ExpandSnippet {
        id: id(1),
        focus: Some(FocusRef::from(h.editor)),
    })
    .expect("sent");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back");
    // A pick ends the palette's hold by itself: the UI host hides the palette before it.
    panel(&mut ui, PanelKind::Palette, true);
    ui.call(RequestBody::ExpandSnippet {
        id: id(2),
        focus: Some(FocusRef::from(h.editor)),
    })
    .expect("sent");
    h.settle();
    assert_eq!(h.text(h.editor), "be right backon my way");

    // The form: a submit ends the form's hold, and the values are typed.
    h.os.type_text(" hi ");
    let EventBody::FormRequested { session, .. } =
        wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. }))
    else {
        unreachable!("matched above")
    };
    ui.call(RequestBody::FormAck { session }).expect("ack");
    panel(&mut ui, PanelKind::Form, true);
    ui.call(RequestBody::FormSubmit {
        session,
        values: Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())])),
    })
    .expect("submit");
    h.settle();
    assert_eq!(h.text(h.editor), "be right backon my way Hello Ada! ");
}

#[test]
fn a_focus_verdict_while_a_panel_has_the_keyboard_does_not_end_it() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    panel(&mut ui, PanelKind::Palette, true);
    // Another ordinary window takes the focus behind the panel; its verdict is `normal`.
    let notes = h.os.open_window(MockWindow::new("org.example.notes", 300));
    h.os.focus(notes).expect("open");
    h.worker().sync();
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(notes), "brb ", "still Iamb's panel");
    panel(&mut ui, PanelKind::Palette, false);
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(notes), "brb be right back ");
}

#[test]
fn a_panel_whose_lease_runs_out_or_whose_ui_host_goes_falls_back() {
    let h = Harness::start_with(LIBRARY, |c| {
        c.panel_focus_lease = Duration::from_millis(300);
    });
    {
        let mut ui = h.client(Role::Ui);
        panel(&mut ui, PanelKind::Form, true);
        // Renewed before it runs out: still held.
        std::thread::sleep(Duration::from_millis(200));
        panel(&mut ui, PanelKind::Form, true);
        std::thread::sleep(Duration::from_millis(200));
        h.os.type_text("brb ");
        h.settle();
        assert_eq!(h.text(h.editor), "brb ");
        // Not renewed: the hold ends by itself.
        std::thread::sleep(Duration::from_millis(500));
        h.settle();
        h.os.type_text("brb ");
        h.settle();
        assert_eq!(h.text(h.editor), "brb be right back ");
        assert!(diagnostic_codes(&mut ui).contains(&"panel_focus_expired".to_owned()));
    }
    // A UI host that goes while its panel has the keyboard (the UI-host-gone path).
    let h = Harness::start(LIBRARY);
    {
        let mut ui = h.client(Role::Ui);
        panel(&mut ui, PanelKind::Palette, true);
    }
    std::thread::sleep(Duration::from_millis(200));
    h.settle();
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ");
}

#[test]
fn a_screen_lock_or_sign_out_forgets_the_remembered_form_values() {
    for event in [SessionEvent::Locked, SessionEvent::SigningOut] {
        let h = Harness::start(LIBRARY);
        assert!(h.os.watching_session(), "the worker watches the session");
        let mut ui = h.client(Role::Ui);
        h.os.type_text("hi ");
        let EventBody::FormRequested { session, .. } =
            wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. }))
        else {
            unreachable!("matched above")
        };
        ui.call(RequestBody::FormAck { session }).expect("ack");
        ui.call(RequestBody::FormSubmit {
            session,
            values: Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())])),
        })
        .expect("submit");
        h.settle();
        assert_eq!(h.text(h.editor), "Hello Ada! ");

        assert!(h.os.session_event(event), "reported");
        h.settle();
        // The next form of the snippet starts empty, and there is nothing to reopen.
        h.os.type_text("hi ");
        match wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. })) {
            EventBody::FormRequested {
                session,
                last_values,
                ..
            } => {
                assert!(last_values.is_none(), "{event:?}: forgotten");
                ui.call(RequestBody::FormCancel { session })
                    .expect("cancel");
            }
            _ => unreachable!("matched above"),
        }
        ui.call(RequestBody::ReopenLastForm { focus: None })
            .expect("sent");
        h.settle();
        assert!(
            diagnostic_codes(&mut ui).contains(&"no_last_form".to_owned()),
            "{event:?}"
        );
        let dir = h.stop();
        drop(dir);
    }
}

#[test]
fn the_session_watcher_is_released_when_the_worker_stops() {
    let h = Harness::start(LIBRARY);
    let os = h.os.clone();
    assert!(os.watching_session());
    drop(h.stop());
    assert!(!os.watching_session());
    assert!(!os.session_event(SessionEvent::Locked), "nobody listens");
}
