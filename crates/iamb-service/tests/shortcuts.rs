//! The two global shortcuts besides the palette's (UX specification §5.1, §11.2 entry 5):
//! "Fill in the last form again" (`reopen_form_hotkey`) and "Pause or resume" (`pause_hotkey`).
//! Each setting registers its chord with the system when it changes, refuses a chord the system
//! refuses (`conflict`) or cannot read (`invalid`), and a press does what `ReopenLastForm` and
//! `SetPaused` do. On the mock, through the service endpoint.

mod common;

use std::collections::BTreeMap;

use common::{Harness, LIBRARY, diagnostic_codes, wait_event};
use iamb_ipc::{Client, ErrorCode, EventBody, RequestBody, ResponseBody, Role, Secret};
use iamb_platform::{Chord, HotkeyId};

const REOPEN: HotkeyId = HotkeyId(2);
const PAUSE: HotkeyId = HotkeyId(3);

fn chord(text: &str) -> Chord {
    Chord::parse(text).expect("chord")
}

fn put(ui: &mut Client, key: &str, value: Option<&str>) -> Result<(), Option<ErrorCode>> {
    ui.call(RequestBody::PutSetting {
        key: key.to_owned(),
        value: value.map(str::to_owned),
    })
    .map(|_| ())
    .map_err(|e| e.remote_code())
}

fn paused(ui: &mut Client) -> bool {
    match ui.call(RequestBody::GetStatus {}).expect("status") {
        ResponseBody::Status { status } => status.paused,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn both_shortcuts_are_registered_refused_and_removed_like_the_palettes() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert!(
        h.os.registered_hotkeys().is_empty(),
        "none is set by default"
    );

    assert_eq!(
        put(&mut ui, "reopen_form_hotkey", Some("Ctrl+Alt+F")),
        Ok(())
    );
    assert_eq!(put(&mut ui, "pause_hotkey", Some("Ctrl+Alt+P")), Ok(()));
    let mut registered = h.os.registered_hotkeys();
    registered.sort_by_key(|(id, _)| *id);
    assert_eq!(
        registered,
        [(REOPEN, chord("Ctrl+Alt+F")), (PAUSE, chord("Ctrl+Alt+P"))]
    );

    // Another application holds it: refused, not stored, and the old chord stays.
    h.os.reserve_chord(chord("Ctrl+Alt+Q"));
    assert_eq!(
        put(&mut ui, "pause_hotkey", Some("Ctrl+Alt+Q")),
        Err(Some(ErrorCode::Conflict))
    );
    assert!(
        h.os.registered_hotkeys()
            .contains(&(PAUSE, chord("Ctrl+Alt+P")))
    );
    match ui.call(RequestBody::GetSettings {}).expect("settings") {
        ResponseBody::Settings { settings } => assert_eq!(
            settings.get("pause_hotkey").map(String::as_str),
            Some("Ctrl+Alt+P")
        ),
        other => panic!("unexpected {other:?}"),
    }
    // The palette's chord is taken by the pause shortcut: the system refuses it too.
    assert_eq!(
        put(&mut ui, "palette_hotkey", Some("Ctrl+Alt+P")),
        Err(Some(ErrorCode::Conflict))
    );
    assert_eq!(
        put(&mut ui, "reopen_form_hotkey", Some("Ctrl+Nope")),
        Err(Some(ErrorCode::Invalid))
    );

    assert_eq!(put(&mut ui, "reopen_form_hotkey", None), Ok(()));
    assert_eq!(put(&mut ui, "pause_hotkey", None), Ok(()));
    assert!(h.os.registered_hotkeys().is_empty());
}

#[test]
fn the_shortcuts_are_registered_again_when_the_worker_starts() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(put(&mut ui, "pause_hotkey", Some("Ctrl+Alt+P")), Ok(()));
    drop(ui);
    let dir = h.stop();
    let h = Harness::start_in(dir, |_| {});
    assert_eq!(h.os.registered_hotkeys(), [(PAUSE, chord("Ctrl+Alt+P"))]);
}

#[test]
fn the_pause_shortcut_pauses_and_resumes() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(put(&mut ui, "pause_hotkey", Some("Ctrl+Alt+P")), Ok(()));
    assert!(!paused(&mut ui));

    assert!(
        h.os.press_chord(chord("Ctrl+Alt+P")),
        "consumed as a hotkey"
    );
    h.settle();
    wait_event(
        &mut ui,
        |e| matches!(e, EventBody::StatusChanged { status } if status.paused),
    );
    assert!(paused(&mut ui));
    // Paused: a typed abbreviation does not expand.
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb ");

    assert!(h.os.press_chord(chord("Ctrl+Alt+P")));
    h.settle();
    assert!(!paused(&mut ui));
}

#[test]
fn the_fill_in_again_shortcut_reopens_the_last_form_in_the_focused_window() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(
        put(&mut ui, "reopen_form_hotkey", Some("Ctrl+Alt+F")),
        Ok(())
    );

    // Nothing to reopen yet: the log says so, as for `ReopenLastForm`.
    assert!(h.os.press_chord(chord("Ctrl+Alt+F")));
    h.settle();
    assert!(diagnostic_codes(&mut ui).contains(&"no_last_form".to_owned()));

    // Fill in a form once.
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

    // The shortcut asks for the same form again, with the last answers.
    assert!(h.os.press_chord(chord("Ctrl+Alt+F")));
    h.settle();
    match wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. })) {
        EventBody::FormRequested {
            session,
            last_values,
            ..
        } => {
            assert_eq!(
                last_values.map(Secret::into_inner),
                Some(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())]))
            );
            ui.call(RequestBody::FormCancel { session })
                .expect("cancel");
        }
        _ => unreachable!("matched above"),
    }
}
