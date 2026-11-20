//! A fill-in form session always ends (the Phase 3 UI review's finding 1, service side): a
//! submit that cannot reach its window closes the session and keeps its answers for
//! `ReopenLastForm`, and a session the UI host stops renewing expires. Either way the engine
//! expands typed abbreviations again instead of refusing every one with `form_open`.

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::{Harness, LIBRARY, T, diagnostic_codes, wait_event};
use iamb_ipc::{Client, EventBody, RequestBody, Role, Secret, WireValues};
use iamb_platform::mock::MockWindow;
use iamb_platform::{FocusToken, PlatformError};

fn ada() -> Secret<WireValues> {
    Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())]))
}

/// Types `hi ` into `window` and acknowledges the form it asks for; returns its session.
fn open_form(h: &Harness, ui: &mut Client, window: FocusToken) -> u64 {
    h.os.focus(window).expect("open");
    h.worker().sync();
    h.os.type_text("hi ");
    h.settle();
    let EventBody::FormRequested { session, .. } =
        wait_event(ui, |e| matches!(e, EventBody::FormRequested { .. }))
    else {
        unreachable!("matched above")
    };
    ui.call(RequestBody::FormAck { session }).expect("ack");
    session
}

/// Types `brb ` into `window` until it expands (up to `T`).
fn wait_expands(h: &Harness, window: FocusToken) {
    let deadline = Instant::now() + T;
    loop {
        let before = h.text(window);
        h.os.type_text("brb ");
        h.settle();
        if h.text(window) == format!("{before}be right back ") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "typed abbreviations stay refused: {:?}",
            h.text(window)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The form reopened by `ReopenLastForm` carries `Ada`.
fn reopens_with_ada(ui: &mut Client) {
    ui.call(RequestBody::ReopenLastForm {}).expect("sent");
    match wait_event(ui, |e| matches!(e, EventBody::FormRequested { .. })) {
        EventBody::FormRequested {
            session,
            last_values,
            ..
        } => {
            assert_eq!(
                last_values.map(Secret::into_inner),
                Some(ada().into_inner()),
                "the answers were kept"
            );
            ui.call(RequestBody::FormCancel { session })
                .expect("cancel");
        }
        _ => unreachable!("matched above"),
    }
}

#[test]
fn a_submit_whose_window_is_gone_ends_the_form_and_keeps_its_answers() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let mail = h.os.open_window(MockWindow::new("org.example.mail", 300));
    let session = open_form(&h, &mut ui, mail);
    // The mail window closes while the form waits in another app; the UI host hands in the
    // answers it has (it cannot give the focus back to a window that is gone).
    h.os.focus(h.editor).expect("open");
    h.os.close_window(mail);
    assert_eq!(h.os.focus(mail), Err(PlatformError::FocusLost));
    h.worker().sync();
    ui.call(RequestBody::FormSubmit {
        session,
        values: ada(),
    })
    .expect("submit");
    h.settle();
    wait_expands(&h, h.editor);
    assert_eq!(h.text(h.editor), "be right back ", "nothing else was typed");
    assert!(diagnostic_codes(&mut ui).contains(&"form_target_gone".to_owned()));
    reopens_with_ada(&mut ui);
}

#[test]
fn a_submit_whose_window_never_gets_the_focus_ends_the_form_and_keeps_its_answers() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    let mail = h.os.open_window(MockWindow::new("org.example.mail", 300));
    let session = open_form(&h, &mut ui, mail);
    // The submit waits for the mail window's focus, which never comes (another window keeps
    // it): it is dropped after the park timeout, and so is the form session.
    h.os.focus(h.editor).expect("open");
    h.worker().sync();
    ui.call(RequestBody::FormSubmit {
        session,
        values: ada(),
    })
    .expect("submit");
    h.settle();
    wait_expands(&h, h.editor);
    assert_eq!(
        h.text(mail),
        "hi ",
        "nothing was typed into the mail window"
    );
    assert!(diagnostic_codes(&mut ui).contains(&"request_focus_timeout".to_owned()));
    reopens_with_ada(&mut ui);
}

#[test]
fn a_form_the_ui_host_stops_renewing_expires() {
    let h = Harness::start_with(LIBRARY, |c| {
        c.form_lease = Duration::from_millis(400);
    });
    let mut ui = h.client(Role::Ui);
    let session = open_form(&h, &mut ui, h.editor);
    // Renewed (each `FormAck` again): the form stays open, and a second abbreviation does
    // nothing (flow 4).
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(150));
        ui.call(RequestBody::FormAck { session }).expect("renew");
    }
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "hi brb ", "the form is still open");
    // Not renewed: the session expires and typing expands again.
    std::thread::sleep(Duration::from_millis(600));
    h.settle();
    wait_expands(&h, h.editor);
    assert!(diagnostic_codes(&mut ui).contains(&"form_expired".to_owned()));
    // A submit for it now changes nothing.
    ui.call(RequestBody::FormSubmit {
        session,
        values: ada(),
    })
    .expect("sent");
    h.settle();
    assert!(!h.text(h.editor).contains("Ada"));
}
