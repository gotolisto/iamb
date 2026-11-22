//! The two global shortcuts besides the palette's (UX specification §5.1, §11.2 entry 5):
//! "Fill in the last form again" (`reopen_form_hotkey`) and "Pause or resume" (`pause_hotkey`).
//! Each setting registers its chord with the system when it changes, refuses a chord the system
//! refuses (`conflict`) or cannot read (`invalid`), and a press does what `ReopenLastForm` and
//! `SetPaused` do. On the mock, through the service endpoint.

mod common;

use std::collections::BTreeMap;

use std::time::{Duration, Instant};

use common::{Harness, LIBRARY, diagnostic_codes, wait_event};
use iamb_ipc::{Client, ErrorCode, EventBody, FocusRef, RequestBody, ResponseBody, Role, Secret};
use iamb_platform::mock::MockWindow;
use iamb_platform::{Chord, FocusToken, HotkeyId};

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
        put(&mut ui, "reopen_form_hotkey", Some("Ctrl+Shift+F")),
        Ok(())
    );
    assert_eq!(put(&mut ui, "pause_hotkey", Some("Ctrl+Shift+P")), Ok(()));
    let mut registered = h.os.registered_hotkeys();
    registered.sort_by_key(|(id, _)| *id);
    assert_eq!(
        registered,
        [
            (REOPEN, chord("Ctrl+Shift+F")),
            (PAUSE, chord("Ctrl+Shift+P"))
        ]
    );

    // Another application holds it: refused, not stored, and the old chord stays.
    h.os.reserve_chord(chord("Ctrl+Shift+Q"));
    assert_eq!(
        put(&mut ui, "pause_hotkey", Some("Ctrl+Shift+Q")),
        Err(Some(ErrorCode::Conflict))
    );
    assert!(
        h.os.registered_hotkeys()
            .contains(&(PAUSE, chord("Ctrl+Shift+P")))
    );
    match ui.call(RequestBody::GetSettings {}).expect("settings") {
        ResponseBody::Settings { settings } => assert_eq!(
            settings.get("pause_hotkey").map(String::as_str),
            Some("Ctrl+Shift+P")
        ),
        other => panic!("unexpected {other:?}"),
    }
    // The palette's chord is taken by the pause shortcut: the system refuses it too.
    assert_eq!(
        put(&mut ui, "palette_hotkey", Some("Ctrl+Shift+P")),
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
    assert_eq!(put(&mut ui, "pause_hotkey", Some("Ctrl+Shift+P")), Ok(()));
    drop(ui);
    let dir = h.stop();
    let h = Harness::start_in(dir, |_| {});
    assert_eq!(h.os.registered_hotkeys(), [(PAUSE, chord("Ctrl+Shift+P"))]);
}

#[test]
fn the_pause_shortcut_pauses_and_resumes() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(put(&mut ui, "pause_hotkey", Some("Ctrl+Shift+P")), Ok(()));
    assert!(!paused(&mut ui));

    assert!(
        h.os.press_chord(chord("Ctrl+Shift+P")),
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

    assert!(h.os.press_chord(chord("Ctrl+Shift+P")));
    h.settle();
    assert!(!paused(&mut ui));
}

#[test]
fn the_fill_in_again_shortcut_reopens_the_last_form_in_the_focused_window() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(
        put(&mut ui, "reopen_form_hotkey", Some("Ctrl+Shift+F")),
        Ok(())
    );

    // Nothing to reopen yet: the log says so, as for `ReopenLastForm`.
    assert!(h.os.press_chord(chord("Ctrl+Shift+F")));
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
    assert!(h.os.press_chord(chord("Ctrl+Shift+F")));
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

/// Fills in the form of `hi ` once in the editor, so that there are answers to reopen.
fn fill_in_once(h: &Harness, ui: &mut Client) {
    h.os.type_text("hi ");
    let EventBody::FormRequested { session, .. } =
        wait_event(ui, |e| matches!(e, EventBody::FormRequested { .. }))
    else {
        unreachable!("matched above")
    };
    ui.call(RequestBody::FormAck { session }).expect("ack");
    ui.call(RequestBody::FormSubmit {
        session,
        values: Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())])),
    })
    .expect("submit");
    // Typed by the injector; under load that can take longer than one settle.
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.text(h.editor) != "Hello Ada! " && Instant::now() < deadline {
        h.settle();
    }
    assert_eq!(h.text(h.editor), "Hello Ada! ");
}

/// The `FormRequested` events that arrive within `wait`, with the window each names.
fn forms_within(ui: &mut Client, wait: Duration) -> Vec<Option<FocusRef>> {
    let deadline = Instant::now() + wait;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        if let Some(EventBody::FormRequested { focus, session, .. }) = ui
            .next_event(Some(Duration::from_millis(50)))
            .expect("event stream")
        {
            out.push(focus);
            ui.call(RequestBody::FormCancel { session })
                .expect("cancel");
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Target {
    PasswordField,
    ExcludedApp,
    OrdinaryWindow,
}

/// Fills in a form in the editor, then switches to `target` and presses the shortcut before
/// the service heard of the switch (the beta-gap review's finding 2).
fn press_right_after_a_switch(target: Target) -> (Harness, Client, FocusToken) {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(
        put(&mut ui, "reopen_form_hotkey", Some("Ctrl+Shift+F")),
        Ok(())
    );
    fill_in_once(&h, &mut ui);
    let window = match target {
        Target::PasswordField => MockWindow::new("org.example.bank", 200).secure(),
        Target::ExcludedApp => {
            ui.call(RequestBody::PutExclusion {
                app_id: "org.example.private".to_owned(),
                display_name: "Private".to_owned(),
                excluded: true,
            })
            .expect("excluded");
            MockWindow::new("org.example.private", 200)
        }
        Target::OrdinaryWindow => MockWindow::new("org.example.other", 200),
    };
    let token = h.os.open_window(window);
    h.settle();
    let _ = forms_within(&mut ui, Duration::from_millis(100));

    h.os.hold_focus_reports(true);
    h.os.focus(token).expect("window is open");
    assert!(h.os.press_chord(chord("Ctrl+Shift+F")));
    h.settle();
    (h, ui, token)
}

#[test]
fn the_fill_in_again_shortcut_waits_for_the_verdict_of_the_window_of_the_press() {
    for target in [Target::PasswordField, Target::ExcludedApp] {
        let (h, mut ui, _) = press_right_after_a_switch(target);
        // While the report is late, no form is sent: not for the window just left.
        assert_eq!(
            forms_within(&mut ui, Duration::from_millis(300)),
            [],
            "{target:?}: a form opened before the verdict"
        );
        // The report arrives with its verdict: the request is refused there.
        h.os.hold_focus_reports(false);
        h.settle();
        assert_eq!(
            forms_within(&mut ui, Duration::from_millis(300)),
            [],
            "{target:?}"
        );
        let codes = diagnostic_codes(&mut ui);
        let expected = match target {
            Target::PasswordField => "refused_secure_field",
            _ => "refused_excluded_app",
        };
        assert!(
            codes.contains(&expected.to_owned()),
            "{target:?}: {codes:?}"
        );
    }

    // An ordinary window: the form opens once its report is in, for that window.
    let (h, mut ui, token) = press_right_after_a_switch(Target::OrdinaryWindow);
    assert_eq!(forms_within(&mut ui, Duration::from_millis(300)), []);
    h.os.hold_focus_reports(false);
    h.settle();
    assert_eq!(
        forms_within(&mut ui, Duration::from_millis(500)),
        [Some(FocusRef::from(token))]
    );
}

#[test]
fn the_trays_fill_in_again_waits_for_the_focus_to_come_back_from_iambs_window() {
    // On Windows the tray menu takes the foreground (an own window of Iamb's); the UI host
    // gives the focus back to the app the menu was for and sends `ReopenLastForm` at once. The
    // request waits for that app's report instead of being refused in Iamb's window.
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    fill_in_once(&h, &mut ui);
    let menu = h.os.open_window(MockWindow::new("iamb-desktop", 300).own());
    h.os.focus(menu).expect("window is open");
    h.settle();

    h.os.hold_focus_reports(true);
    h.os.focus(h.editor).expect("window is open");
    ui.call(RequestBody::ReopenLastForm {}).expect("asked");
    h.settle();
    assert_eq!(forms_within(&mut ui, Duration::from_millis(200)), []);
    h.os.hold_focus_reports(false);
    h.settle();
    assert_eq!(
        forms_within(&mut ui, Duration::from_millis(500)),
        [Some(FocusRef::from(h.editor))]
    );
    assert!(
        !diagnostic_codes(&mut ui).contains(&"refused_own_window".to_owned()),
        "refused while the hand-back was on its way"
    );

    // No report at all: dropped after the time limit, not run in Iamb's window.
    h.os.focus(menu).expect("window is open");
    h.settle();
    h.os.hold_focus_reports(true);
    ui.call(RequestBody::ReopenLastForm {}).expect("asked");
    assert_eq!(forms_within(&mut ui, Duration::from_millis(1500)), []);
    assert!(diagnostic_codes(&mut ui).contains(&"request_focus_timeout".to_owned()));
    h.os.hold_focus_reports(false);
}

#[test]
fn editing_chords_and_chords_that_type_are_refused_by_the_service() {
    // Not only the recorder: the service refuses them for every shortcut setting (the beta-gap
    // review's finding 4). Iamb's own paste sends Ctrl+V or Cmd+V.
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    for key in ["palette_hotkey", "reopen_form_hotkey", "pause_hotkey"] {
        for chord in [
            "Ctrl+V",
            "Meta+V",
            "Ctrl+C",
            "Ctrl+Z",
            "Ctrl+Backspace",
            "Alt+Q",
        ] {
            assert_eq!(
                put(&mut ui, key, Some(chord)),
                Err(Some(ErrorCode::Invalid)),
                "{key} = {chord}"
            );
        }
    }
    assert!(h.os.registered_hotkeys().is_empty());
}
