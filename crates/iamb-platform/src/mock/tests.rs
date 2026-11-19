//! Unit tests of the mock, one trait at a time. The end-to-end tests with the engine are in
//! `tests/end_to_end.rs`.

use std::io::{ErrorKind, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use iamb_core::engine::{AppId, BoundaryKind, ExpansionId};
use iamb_core::template::KeyName;

use super::*;
use crate::endpoint::{EndpointKind, PeerRejection};
use crate::platform::Platform;

const WAIT: Duration = Duration::from_secs(5);

type Seen = Arc<Mutex<Vec<ListenerEvent>>>;

/// A started listener whose sink records every event.
fn listening(os: &MockPlatform) -> Seen {
    let seen = Seen::default();
    let log = Arc::clone(&seen);
    os.start(Box::new(move |event: ListenerEvent| lock(&log).push(event)))
        .expect("listener starts");
    seen
}

/// A started listener whose sink feeds the hand-off directly, as the service's engine thread
/// does, and records every event. Injector tests need it: the mock injector waits for the
/// hand-off to seal.
fn listening_with(os: &MockPlatform, handoff: &Arc<TypedHandoff>) -> Seen {
    let seen = Seen::default();
    let log = Arc::clone(&seen);
    let handoff = Arc::clone(handoff);
    os.start(Box::new(move |event: ListenerEvent| {
        handoff.observe(&event);
        lock(&log).push(event);
    }))
    .expect("listener starts");
    seen
}

fn editor(os: &MockPlatform) -> FocusToken {
    let token = os.open_window(MockWindow::new("org.example.editor", 100));
    os.focus(token).expect("window is open");
    token
}

fn id(n: u64) -> ExpansionId {
    ExpansionId::from_raw(n)
}

fn text_step(text: &str) -> InjectionStep {
    InjectionStep::TypeText(text.to_owned())
}

fn take(seen: &Seen) -> Vec<ListenerEvent> {
    std::mem::take(&mut *lock(seen))
}

// ---------------------------------------------------------------------------------------------
// Listener
// ---------------------------------------------------------------------------------------------

#[test]
fn keys_reach_the_field_and_the_listener_in_order() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let seen = listening(&os);
    assert!(os.listener_running());

    os.type_text("a b\n");
    os.press(KeyName::Backspace);
    os.press(KeyName::Left);
    os.click();
    assert!(!os.press_chord(Chord::parse("Ctrl+B").expect("chord")));

    assert_eq!(os.text(window).as_deref(), Some("a b"));
    assert_eq!(os.caret(window), Some(2));
    let boundary = |kind, seq| ListenerEvent::Boundary { kind, seq };
    assert_eq!(
        take(&seen),
        [
            ListenerEvent::char('a', 1),
            ListenerEvent::char(' ', 2),
            ListenerEvent::char('b', 3),
            boundary(BoundaryKind::Enter, 4),
            ListenerEvent::Backspace { seq: 5 },
            boundary(BoundaryKind::Arrow, 6),
            boundary(BoundaryKind::Click, 7),
            boundary(BoundaryKind::Shortcut, 8),
        ]
    );
}

#[test]
fn scripted_streams_and_text_conversion() {
    assert_eq!(
        MockKey::text("a\tb\n"),
        [
            MockKey::Char('a'),
            MockKey::Key(KeyName::Tab),
            MockKey::Char('b'),
            MockKey::Key(KeyName::Enter),
        ]
    );
    let os = MockPlatform::new();
    let window = editor(&os);
    let seen = listening(&os);
    os.play([
        MockKey::Char('x'),
        MockKey::Key(KeyName::Space),
        MockKey::Shortcut,
        MockKey::Click,
    ]);
    assert_eq!(os.text(window).as_deref(), Some("x "));
    assert_eq!(take(&seen).len(), 4);
}

#[test]
fn start_stop_and_restart() {
    let os = MockPlatform::default();
    let window = editor(&os);
    assert!(!os.listener_running());
    os.type_text("a"); // no listener: the key still reaches the field
    assert_eq!(os.text(window).as_deref(), Some("a"));

    let seen = listening(&os);
    assert_eq!(
        os.start(Box::new(|_: ListenerEvent| {})),
        Err(PlatformError::AlreadyRunning)
    );
    os.type_text("b");
    os.stop().expect("stops");
    os.stop().expect("stopping twice is fine");
    assert!(!os.listener_running());
    os.type_text("c");
    assert_eq!(take(&seen), [ListenerEvent::char('b', 1)]);

    // Sequence numbers continue across a restart.
    let seen = listening(&os);
    os.type_text("d");
    assert_eq!(take(&seen), [ListenerEvent::char('d', 2)]);
    assert_eq!(os.text(window).as_deref(), Some("abcd"));
}

#[test]
fn the_listener_needs_input_monitoring() {
    let os = MockPlatform::new();
    os.set_permission(PermissionKind::InputMonitoring, Grant::Denied);
    assert_eq!(
        os.start(Box::new(|_: ListenerEvent| {})),
        Err(PlatformError::PermissionDenied(
            PermissionKind::InputMonitoring
        ))
    );
    os.set_permission(PermissionKind::InputMonitoring, Grant::Granted);
    assert_eq!(os.start(Box::new(|_: ListenerEvent| {})), Ok(()));
}

#[test]
fn composition_and_lost_events_are_reported() {
    let os = MockPlatform::new();
    let seen = listening(&os);
    os.set_composing(true);
    os.set_composing(false);
    os.lose_events();
    assert_eq!(
        take(&seen),
        [
            ListenerEvent::Composing { on: true, seq: 1 },
            ListenerEvent::Composing { on: false, seq: 2 },
            ListenerEvent::Overflow,
        ]
    );
}

#[test]
fn the_self_test_finds_a_dead_hook_and_a_restart_repairs_it() {
    let os = MockPlatform::new();
    let window = editor(&os);
    assert_eq!(os.self_test(WAIT), Err(PlatformError::NotRunning));

    let seen = listening(&os);
    assert_eq!(os.self_test(WAIT), Ok(()));
    assert_eq!(os.self_tests(), 1);
    assert!(take(&seen).is_empty(), "the self-test delivers nothing");

    os.kill_hook();
    os.type_text("x");
    assert_eq!(os.text(window).as_deref(), Some("x"), "typing still works");
    assert!(take(&seen).is_empty(), "but the listener is blind");
    assert_eq!(os.self_test(WAIT), Err(PlatformError::Timeout));
    assert!(os.listener_running());

    os.stop().expect("stops");
    let seen = listening(&os);
    assert_eq!(os.self_test(WAIT), Ok(()));
    os.type_text("y");
    assert_eq!(take(&seen).len(), 1);
    assert_eq!(os.self_tests(), 2);
}

// ---------------------------------------------------------------------------------------------
// Hotkeys
// ---------------------------------------------------------------------------------------------

#[test]
fn hotkeys_are_delivered_through_the_listener_and_consumed() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let seen = listening(&os);
    let palette = Chord::parse("Ctrl+Shift+Space").expect("chord");
    let other = Chord::parse("Alt+K").expect("chord");

    os.register(HotkeyId(1), palette).expect("free chord");
    assert_eq!(os.registered_hotkeys(), [(HotkeyId(1), palette)]);
    os.type_text("a");
    assert!(os.press_chord(palette));
    assert!(!os.press_chord(other));
    assert_eq!(
        take(&seen),
        [
            ListenerEvent::char('a', 1),
            ListenerEvent::Hotkey {
                id: HotkeyId(1),
                seq: 2
            },
            ListenerEvent::Boundary {
                kind: BoundaryKind::Shortcut,
                seq: 3
            },
        ]
    );
    assert_eq!(os.text(window).as_deref(), Some("a"));

    os.unregister(HotkeyId(1)).expect("unregisters");
    os.unregister(HotkeyId(1)).expect("unknown ids are fine");
    assert!(os.registered_hotkeys().is_empty());
    assert!(!os.press_chord(palette));
}

#[test]
fn a_taken_chord_cannot_be_registered() {
    let os = MockPlatform::new();
    let chord = Chord::parse("Meta+J").expect("chord");
    let free = Chord::parse("Meta+L").expect("chord");
    os.reserve_chord(chord);
    assert_eq!(
        os.register(HotkeyId(1), chord),
        Err(PlatformError::HotkeyUnavailable)
    );
    os.register(HotkeyId(1), free).expect("free chord");
    assert_eq!(
        os.register(HotkeyId(2), free),
        Err(PlatformError::HotkeyUnavailable),
        "another id holds it"
    );
    // The same id may register again, with the same or another chord.
    os.register(HotkeyId(1), free).expect("same id");
    let moved = Chord::parse("F9").expect("chord");
    os.register(HotkeyId(1), moved).expect("replaces");
    assert_eq!(os.registered_hotkeys(), [(HotkeyId(1), moved)]);
    os.register(HotkeyId(2), free).expect("now free");
}

// ---------------------------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------------------------

#[test]
fn focus_follows_the_windows() {
    let os = MockPlatform::new();
    assert_eq!(os.focused_app(), Err(PlatformError::NoFocus));
    assert_eq!(os.focus_token(), Err(PlatformError::NoFocus));
    assert_eq!(
        os.secure_state(&FocusToken::from_raw(1, 1), WAIT),
        FieldSecurity::Unknown(UnknownReason::NoFocus)
    );

    let editor = os.open_window(MockWindow::new("org.example.editor", 100).text("draft"));
    let own = os.open_window(MockWindow::new("iamb-desktop", 200).own());
    assert_eq!(
        os.focused_app(),
        Err(PlatformError::NoFocus),
        "not focused yet"
    );
    assert_ne!(editor, own);

    os.focus(editor).expect("open");
    assert_eq!(
        os.focused_app(),
        Ok(AppIdentity::new("org.example.editor", 100))
    );
    assert_eq!(os.focus_token(), Ok(editor));
    assert_eq!(editor.pid(), 100);
    assert_eq!(os.text(editor).as_deref(), Some("draft"));
    assert_eq!(os.caret(editor), Some(5));

    os.focus(own).expect("open");
    let app = os.focused_app().expect("focused");
    assert!(app.own);
    assert_eq!(app.app, AppId("iamb-desktop".to_owned()));

    os.close_window(own);
    assert_eq!(os.focused_app(), Err(PlatformError::NoFocus));
    assert_eq!(os.focus(own), Err(PlatformError::FocusLost));
    assert_eq!(os.text(own), None);
    assert_eq!(os.caret(own), None);
    os.close_window(editor);
    os.set_text(editor, "ignored");
    os.set_security(editor, FieldSecurity::Secure);
}

#[test]
fn keys_go_to_the_focused_window_only() {
    let os = MockPlatform::new();
    let a = os.open_window(MockWindow::new("a", 1));
    let b = os.open_window(MockWindow::new("b", 2));
    os.type_text("lost");
    os.focus(a).expect("open");
    os.type_text("one");
    os.focus(b).expect("open");
    os.type_text("two");
    os.set_text(a, "reset");
    assert_eq!(os.text(a).as_deref(), Some("reset"));
    assert_eq!(os.caret(a), Some(5));
    assert_eq!(os.text(b).as_deref(), Some("two"));
}

#[test]
fn secure_state_reports_the_field_scripts_delays_and_elevation() {
    let os = MockPlatform::new();
    let normal = os.open_window(MockWindow::new("editor", 1));
    let password = os.open_window(MockWindow::new("bank", 2).secure());
    let slow = os.open_window(
        MockWindow::new("slow", 3)
            .security(FieldSecurity::Normal)
            .inspect_delay(Duration::from_millis(300)),
    );
    let admin = os.open_window(MockWindow::new("admin-tool", 4).elevated());

    os.focus(normal).expect("open");
    assert_eq!(os.secure_state(&normal, WAIT), FieldSecurity::Normal);
    os.focus(password).expect("open");
    assert_eq!(os.secure_state(&password, WAIT), FieldSecurity::Secure);
    os.focus(slow).expect("open");
    assert_eq!(
        os.secure_state(&slow, Duration::from_millis(100)),
        FieldSecurity::Unknown(UnknownReason::Timeout)
    );
    assert_eq!(
        os.secure_state(&slow, Duration::from_millis(300)),
        FieldSecurity::Normal
    );
    os.focus(admin).expect("open");
    assert_eq!(
        os.secure_state(&admin, WAIT),
        FieldSecurity::Unknown(UnknownReason::Elevated)
    );

    os.focus(normal).expect("open");
    os.script_secure_states([
        FieldSecurity::Secure,
        FieldSecurity::Unknown(UnknownReason::NotInspectable),
    ]);
    assert_eq!(os.secure_state(&normal, WAIT), FieldSecurity::Secure);
    assert_eq!(
        os.secure_state(&normal, WAIT),
        FieldSecurity::Unknown(UnknownReason::NotInspectable)
    );
    assert_eq!(
        os.secure_state(&normal, WAIT),
        FieldSecurity::Normal,
        "script used up"
    );
}

#[test]
fn secure_state_never_answers_for_another_window() {
    let os = MockPlatform::new();
    let editor = os.open_window(MockWindow::new("editor", 1));
    let password = os.open_window(MockWindow::new("bank", 2).secure());
    os.focus(editor).expect("open");
    // The service judges the change to the password window, but the focus has moved on to
    // the editor: the editor's "normal" must not become the password window's verdict.
    assert_eq!(
        os.secure_state(&password, WAIT),
        FieldSecurity::Unknown(UnknownReason::FocusMoved)
    );
    os.focus(password).expect("open");
    assert_eq!(
        os.secure_state(&editor, WAIT),
        FieldSecurity::Unknown(UnknownReason::FocusMoved)
    );
    assert_eq!(os.secure_state(&password, WAIT), FieldSecurity::Secure);
}

#[test]
fn the_watcher_hears_window_and_field_changes() {
    let os = MockPlatform::new();
    let a = os.open_window(MockWindow::new("a", 1));
    let b = os.open_window(MockWindow::new("b", 2));
    os.focus(a).expect("open");

    let seen: Arc<Mutex<Vec<FocusChange>>> = Arc::default();
    let log = Arc::clone(&seen);
    assert!(!os.is_watching());
    os.watch(Box::new(move |change: FocusChange| lock(&log).push(change)))
        .expect("watches");
    assert!(os.is_watching());
    assert_eq!(
        os.watch(Box::new(|_: FocusChange| {})),
        Err(PlatformError::AlreadyRunning)
    );

    os.focus(b).expect("open");
    os.set_security(b, FieldSecurity::Secure); // moved to the password field of b
    os.set_security(a, FieldSecurity::Secure); // a is not focused: nothing to report
    os.unwatch().expect("stops");
    assert!(!os.is_watching());
    os.unwatch().expect("twice is fine");
    os.focus(a).expect("open");

    let tokens: Vec<FocusToken> = lock(&seen).iter().map(|c| c.token).collect();
    assert_eq!(tokens, [a, b, b], "current focus first, then each change");
    assert_eq!(lock(&seen)[1].app, AppIdentity::new("b", 2));

    // Watching with nothing focused reports nothing at once.
    let empty = MockPlatform::new();
    let seen: Arc<Mutex<Vec<FocusChange>>> = Arc::default();
    let log = Arc::clone(&seen);
    empty
        .watch(Box::new(move |change: FocusChange| lock(&log).push(change)))
        .expect("watches");
    assert!(lock(&seen).is_empty());
}

#[test]
fn focus_is_given_back_to_a_token() {
    let os = MockPlatform::new();
    let target = os.open_window(MockWindow::new("editor", 1));
    let form = os.open_window(MockWindow::new("iamb-desktop", 2).own());
    os.focus(target).expect("open");
    let token = os.focus_token().expect("focused");
    os.allow_foreground(2).expect("allowed");
    assert_eq!(os.allowed_foreground(), [2]);
    os.focus(form).expect("open");

    os.refuse_focus_restore(true);
    assert_eq!(os.restore_focus(&token), Err(PlatformError::Timeout));
    assert_eq!(os.focus_token(), Ok(form));
    os.refuse_focus_restore(false);
    assert_eq!(os.restore_focus(&token), Ok(()));
    assert_eq!(os.focus_token(), Ok(target));

    os.close_window(target);
    assert_eq!(os.restore_focus(&token), Err(PlatformError::FocusLost));
}

// ---------------------------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------------------------

#[test]
fn the_clipboard_keeps_a_history_of_ordinary_writes_only() {
    let os = MockPlatform::new();
    assert_eq!(os.read_text().map(|t| t.is_none()), Ok(true));
    assert!(os.snapshot().expect("readable").is_empty());

    os.copy(ClipboardContent::text("copied by the user"));
    let text = os.read_text().expect("readable").expect("has text");
    assert_eq!(text.expose(), "copied by the user");

    let before = os.snapshot().expect("readable");
    assert!(before.is_complete());
    os.write(&ClipboardContent::rich("snippet", "<b>snippet</b>"), true)
        .expect("writable");
    assert_eq!(os.clipboard().html.as_deref(), Some("<b>snippet</b>"));
    os.restore(before).expect("writable");
    assert_eq!(os.clipboard(), ClipboardContent::text("copied by the user"));
    assert_eq!(
        os.clipboard_history(),
        [ClipboardContent::text("copied by the user")],
        "the transient write and the restore left no history entry"
    );

    os.write(&ClipboardContent::text("kept"), false)
        .expect("writable");
    assert_eq!(os.clipboard_history().len(), 2);
}

#[test]
fn a_snapshot_restores_every_flavour() {
    let os = MockPlatform::new();
    let content = ClipboardContent {
        text: Some("plain".to_owned()),
        html: Some("<i>plain</i>".to_owned()),
        files: vec!["/tmp/one.png".into(), "/tmp/two.png".into()],
    };
    os.copy(content.clone());
    let snapshot = os.snapshot().expect("readable");
    assert_eq!(snapshot.flavours().len(), 3);
    os.write(&ClipboardContent::default(), true)
        .expect("writable");
    assert!(os.clipboard().is_empty());
    os.restore(snapshot).expect("writable");
    assert_eq!(os.clipboard(), content);

    // Flavours the mock does not know are ignored.
    let foreign = ClipboardSnapshot::new(
        vec![ClipboardFlavour {
            format: ClipboardFormat::Id(1),
            data: Sensitive::new(b"x".to_vec()),
        }],
        false,
    );
    os.restore(foreign).expect("writable");
    assert!(os.clipboard().is_empty());
}

#[test]
fn a_busy_clipboard_fails_every_call() {
    let os = MockPlatform::new();
    os.copy(ClipboardContent::text("x"));
    os.set_clipboard_busy(true);
    assert!(matches!(os.read_text(), Err(PlatformError::Os { .. })));
    assert!(matches!(os.snapshot(), Err(PlatformError::Os { .. })));
    assert!(matches!(
        os.write(&ClipboardContent::text("y"), true),
        Err(PlatformError::Os { .. })
    ));
    assert!(matches!(
        os.restore(ClipboardSnapshot::default()),
        Err(PlatformError::Os { .. })
    ));
    os.set_clipboard_busy(false);
    assert_eq!(os.clipboard(), ClipboardContent::text("x"));
}

// ---------------------------------------------------------------------------------------------
// Permissions, login item, memory
// ---------------------------------------------------------------------------------------------

#[test]
fn permissions_can_be_denied_and_settings_opened() {
    let os = MockPlatform::new();
    assert!(os.permission_status().all_satisfied());
    os.set_permission(PermissionKind::Accessibility, Grant::Denied);
    assert_eq!(
        os.permission_status().missing(),
        [PermissionKind::Accessibility]
    );
    os.open_settings(PermissionKind::Accessibility)
        .expect("opens");
    assert_eq!(os.opened_settings(), [PermissionKind::Accessibility]);
    os.set_permission(PermissionKind::Accessibility, Grant::NotRequired);
    os.set_permission(PermissionKind::InputMonitoring, Grant::NotRequired);
    assert_eq!(os.permission_status(), PermissionStatus::NOT_REQUIRED);
}

#[test]
fn the_login_item_can_be_toggled() {
    let os = MockPlatform::new();
    assert_eq!(os.login_item_status(), Ok(LoginItemStatus::Disabled));
    os.enable().expect("enables");
    os.enable().expect("twice is fine");
    assert_eq!(os.login_item_status(), Ok(LoginItemStatus::Enabled));
    os.disable().expect("disables");
    assert_eq!(os.login_item_status(), Ok(LoginItemStatus::Disabled));

    os.set_login_item_status(LoginItemStatus::RequiresApproval);
    os.enable().expect("registered, waiting for the user");
    assert_eq!(
        os.login_item_status(),
        Ok(LoginItemStatus::RequiresApproval)
    );

    os.set_login_item_status(LoginItemStatus::NotFound);
    assert!(matches!(os.enable(), Err(PlatformError::Os { .. })));
    os.disable().expect("nothing to remove");
    assert_eq!(os.login_item_status(), Ok(LoginItemStatus::NotFound));
}

#[test]
fn memory_regions_are_locked_once_and_unlocked() {
    let os = MockPlatform::new();
    let handoff = TypedHandoff::new();
    let region = handoff.region();
    os.lock(region).expect("locks");
    os.lock(region).expect("twice is fine");
    assert_eq!(os.locked_regions(), [region]);
    assert_eq!(
        os.lock(MemoryRegion::from_raw(region.addr(), 0)),
        Err(PlatformError::Invalid {
            context: "empty memory region"
        })
    );
    os.unlock(region).expect("unlocks");
    assert!(os.locked_regions().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Injector
// ---------------------------------------------------------------------------------------------

#[test]
fn an_expansion_replaces_the_abbreviation() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("see brb ");
    take(&seen);

    handoff.arm(id(1));
    let request = InjectionRequest::new(
        id(1),
        vec![
            InjectionStep::DeleteBack(4),
            text_step("be right back"),
            text_step(" "),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(window).as_deref(), Some("see be right back "));
    assert_eq!(
        os.injected(),
        [
            Injected::StartMarker(id(1)),
            Injected::DeleteBack(4),
            Injected::TypeText("be right back".to_owned()),
            Injected::TypeText(" ".to_owned()),
            Injected::EndMarker(id(1)),
        ]
    );
    // The listener saw the two markers and none of the output.
    assert_eq!(
        take(&seen),
        [
            ListenerEvent::InjectionStarted {
                expansion: id(1),
                seq: 9
            },
            ListenerEvent::InjectionFinished {
                expansion: id(1),
                seq: 10
            },
        ]
    );
    // 4 deletions, 14 characters and the two markers.
    assert_eq!(os.synthetic_events(), 20);
    assert_eq!(handoff.status(), HandoffStatus::Idle, "zeroed at the end");
    assert_eq!(os.take_injected().len(), 5);
    assert!(os.injected().is_empty());
}

#[test]
fn every_step_kind_reaches_the_field() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let _seen = listening_with(&os, &handoff);
    os.copy(ClipboardContent::text("user's clipboard"));

    handoff.arm(id(2));
    let request = InjectionRequest::new(
        id(2),
        vec![
            text_step("Dear ,"),
            InjectionStep::PressKey {
                key: KeyName::Enter,
                shift: true,
                times: 2,
            },
            InjectionStep::Wait(150),
            InjectionStep::Paste(ClipboardContent::rich("Regards", "<b>Regards</b>")),
            InjectionStep::Wait(50),
            InjectionStep::MoveCaretBack(10),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(window).as_deref(), Some("Dear ,\n\nRegards"));
    assert_eq!(os.caret(window), Some(5), "between the space and the comma");
    assert_eq!(os.waited_ms(), 200);
    assert_eq!(
        os.injected(),
        [
            Injected::StartMarker(id(2)),
            Injected::TypeText("Dear ,".to_owned()),
            Injected::PressKey {
                key: KeyName::Enter,
                shift: true,
                times: 2
            },
            Injected::Wait(150),
            Injected::Paste(ClipboardContent::rich("Regards", "<b>Regards</b>")),
            Injected::Wait(50),
            Injected::MoveCaretBack(10),
            Injected::EndMarker(id(2)),
        ]
    );
    // The paste left the clipboard and its history as they were.
    assert_eq!(os.clipboard(), ClipboardContent::text("user's clipboard"));
    assert_eq!(os.clipboard_history().len(), 1);
}

#[test]
fn an_empty_expansion_still_posts_both_markers() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    handoff.arm(id(3));
    assert_eq!(
        os.inject(&InjectionRequest::new(id(3), Vec::new()), &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(
        os.injected(),
        [Injected::StartMarker(id(3)), Injected::EndMarker(id(3))]
    );
    assert_eq!(take(&seen).len(), 2);
    assert_eq!(os.text(window).as_deref(), Some(""));
}

#[test]
fn text_can_be_forced_through_the_clipboard() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let _seen = listening_with(&os, &handoff);
    handoff.arm(id(1));
    let mut request = InjectionRequest::new(id(1), vec![text_step("long text")])
        .in_app(AppId("org.example.editor".to_owned()));
    request.overrides.text_method = Some(TextMethod::Paste);
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(window).as_deref(), Some("long text"));
    assert_eq!(
        os.injected()[1],
        Injected::Paste(ClipboardContent::text("long text"))
    );
    assert!(os.clipboard().is_empty(), "restored to empty");
    assert!(os.clipboard_history().is_empty());

    // With the override naming key events, text is typed.
    handoff.arm(id(2));
    request.expansion = id(2);
    request.overrides.text_method = Some(TextMethod::Keys);
    os.inject(&request, &handoff);
    assert_eq!(os.injected()[4], Injected::TypeText("long text".to_owned()));
}

#[test]
fn keys_before_the_start_marker_are_deleted_and_typed_again() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("brb ");
    take(&seen);

    handoff.arm(id(1));
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("o"));
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("m"));
    let request = InjectionRequest::new(
        id(1),
        vec![
            InjectionStep::DeleteBack(4),
            text_step("be right back"),
            text_step(" "),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 2 }
    );
    assert_eq!(os.text(window).as_deref(), Some("be right back om"));
    assert_eq!(
        os.injected(),
        [
            Injected::StartMarker(id(1)),
            Injected::DeleteBack(6),
            Injected::TypeText("be right back".to_owned()),
            Injected::TypeText(" ".to_owned()),
            Injected::Retype(2),
            Injected::EndMarker(id(1)),
        ]
    );
    // In the stream: the two real keys, then the start marker, then the end marker.
    let events = take(&seen);
    assert_eq!(events.len(), 4);
    assert!(events[0].is_real_key() && events[1].is_real_key());
    assert!(matches!(events[2], ListenerEvent::InjectionStarted { .. }));
    assert!(matches!(events[3], ListenerEvent::InjectionFinished { .. }));
}

#[test]
fn compensation_without_a_deletion_of_its_own_and_with_a_caret_move() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let _seen = listening_with(&os, &handoff);

    // A palette pick: nothing to delete but the keys typed since.
    handoff.arm(id(1));
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("ab"));
    let request = InjectionRequest::new(id(1), vec![text_step("Hello ")]);
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 2 }
    );
    assert_eq!(os.text(window).as_deref(), Some("Hello ab"));
    assert_eq!(os.injected()[1], Injected::DeleteBack(2));

    // With a caret move, the keys are typed again where the caret ends up.
    os.set_text(window, "sig");
    handoff.arm(id(2));
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("x"));
    let request = InjectionRequest::new(
        id(2),
        vec![
            InjectionStep::DeleteBack(3),
            text_step("Hi , bye"),
            InjectionStep::MoveCaretBack(5),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 1 }
    );
    assert_eq!(os.text(window).as_deref(), Some("Hi x, bye"));
}

#[test]
fn a_boundary_before_the_start_marker_cancels_the_expansion() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("brb ");
    take(&seen);

    handoff.arm(id(1));
    os.interleave(
        InjectionPoint::BeforeStartMarker,
        [MockKey::Char('a'), MockKey::Key(KeyName::Left)],
    );
    let request = InjectionRequest::new(id(1), vec![InjectionStep::DeleteBack(4), text_step("x")]);
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::Boundary)
    );
    assert_eq!(
        os.text(window).as_deref(),
        Some("brb a"),
        "nothing was sent"
    );
    assert_eq!(
        os.injected(),
        [
            Injected::StartMarker(id(1)),
            Injected::Cancelled(id(1), CancelReason::Boundary),
        ]
    );
    // The start marker was posted; no end marker follows.
    let events = take(&seen);
    assert!(matches!(
        events.last(),
        Some(ListenerEvent::InjectionStarted { .. })
    ));
}

#[test]
fn an_expansion_cancelled_before_the_injector_runs_sends_nothing() {
    let os = MockPlatform::new();
    let _window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    handoff.arm(id(1));
    handoff.disturb();
    assert_eq!(
        os.inject(
            &InjectionRequest::new(id(1), vec![text_step("x")]),
            &handoff
        ),
        InjectionOutcome::Cancelled(CancelReason::Disturbed)
    );
    assert_eq!(
        os.injected(),
        [Injected::Cancelled(id(1), CancelReason::Disturbed)]
    );
    assert!(take(&seen).is_empty(), "not even the start marker");

    // Not armed at all: the start marker is posted but cannot seal anything.
    handoff.finish(id(1));
    assert_eq!(
        os.inject(&InjectionRequest::new(id(2), Vec::new()), &handoff),
        InjectionOutcome::Cancelled(CancelReason::NotArmed)
    );
}

#[test]
fn a_missing_start_marker_times_out() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    os.set_marker_timeout(Duration::from_millis(5));

    // No listener at all.
    handoff.arm(id(1));
    let request = InjectionRequest::new(id(1), vec![text_step("x")]);
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::StartMarkerTimeout)
    );

    // A listener whose hook the OS removed.
    let _seen = listening_with(&os, &handoff);
    os.kill_hook();
    handoff.arm(id(2));
    let request = InjectionRequest::new(id(2), vec![text_step("x")]);
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::StartMarkerTimeout)
    );
    assert_eq!(os.text(window).as_deref(), Some(""));
    assert!(InjectionOutcome::Cancelled(CancelReason::StartMarkerTimeout).needs_cancel_event());
}

#[test]
fn keys_between_the_markers_land_where_the_caret_is() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("brb ");
    take(&seen);
    let steps = vec![
        InjectionStep::DeleteBack(4),
        text_step("be right back"),
        text_step(" "),
    ];

    // Right after the start marker: the key is in the field before the deletion, so the
    // deletion takes it and leaves a character of the abbreviation. Not repairable (§4.6).
    handoff.arm(id(1));
    os.interleave(InjectionPoint::AfterStartMarker, MockKey::text("x"));
    assert_eq!(
        os.inject(&InjectionRequest::new(id(1), steps.clone()), &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(window).as_deref(), Some("bbe right back "));
    let events = take(&seen);
    assert!(matches!(events[0], ListenerEvent::InjectionStarted { .. }));
    assert!(events[1].is_real_key(), "the key is between the markers");
    assert!(matches!(events[2], ListenerEvent::InjectionFinished { .. }));

    // Between two steps, and before the end marker.
    os.set_text(window, "brb ");
    handoff.arm(id(2));
    os.interleave(InjectionPoint::BeforeStep(0), MockKey::text("0"));
    os.interleave(InjectionPoint::BeforeStep(2), MockKey::text("2"));
    os.interleave(InjectionPoint::BeforeEndMarker, MockKey::text("!"));
    assert_eq!(
        os.inject(&InjectionRequest::new(id(2), steps), &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(window).as_deref(), Some("bbe right back2 !"));
}

/// Review 2.8, finding 1: the trigger key moved the focus to a password field while the start
/// marker travelled. Nothing of the expansion may reach it.
#[test]
fn a_focus_change_after_the_start_marker_stops_the_expansion_before_its_first_step() {
    let os = MockPlatform::new();
    let form = editor(&os);
    let password = os.open_window(MockWindow::new("org.example.editor", 100).secure());
    os.set_text(password, "hunter22");
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("brb\t");
    take(&seen);

    handoff.arm(id(1));
    os.script_at(InjectionPoint::AfterStartMarker, move |os| {
        os.focus(password).expect("open");
    });
    let request = InjectionRequest::new(
        id(1),
        vec![InjectionStep::DeleteBack(4), text_step("be right back\t")],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::FocusMoved)
    );
    assert_eq!(os.text(password).as_deref(), Some("hunter22"), "untouched");
    assert_eq!(os.text(form).as_deref(), Some("brb\t"));
    assert_eq!(
        os.injected(),
        [
            Injected::StartMarker(id(1)),
            Injected::Cancelled(id(1), CancelReason::FocusMoved),
        ]
    );
    // No end marker: the service answers with `InjectionCancelled`.
    let events = take(&seen);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ListenerEvent::InjectionStarted { .. }));
}

/// Follow-up review of 2.8, N1: a window took the focus between the trigger key and the
/// injector's first look. The request names the trigger's window, so nothing is sent, not
/// even the start marker; the same request in the trigger's window goes ahead.
#[test]
fn a_request_for_another_window_than_the_focused_one_sends_nothing() {
    let os = MockPlatform::new();
    let form = editor(&os);
    let password = os.open_window(MockWindow::new("org.example.bank", 200).secure());
    os.set_text(password, "hunter22");
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("brb ");
    take(&seen);

    handoff.arm(id(1));
    os.script_at(InjectionPoint::BeforeStartMarker, move |os| {
        os.focus(password).expect("open");
    });
    let steps = vec![InjectionStep::DeleteBack(4), text_step("be right back ")];
    let request = InjectionRequest::new(id(1), steps.clone()).for_target(KeyTarget::window(form));
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::FocusMoved)
    );
    assert_eq!(os.text(password).as_deref(), Some("hunter22"), "untouched");
    assert_eq!(
        os.injected(),
        [Injected::Cancelled(id(1), CancelReason::FocusMoved)]
    );
    assert!(take(&seen).is_empty(), "no marker was posted");

    // Back in the trigger's window, the same request is injected.
    os.focus(form).expect("open");
    handoff.finish(id(1));
    handoff.arm(id(2));
    let request = InjectionRequest::new(id(2), steps).for_target(KeyTarget::window(form));
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(form).as_deref(), Some("be right back "));
}

#[test]
fn a_focus_change_during_a_wait_stops_the_steps_after_it() {
    let os = MockPlatform::new();
    let chat = editor(&os);
    let other = os.open_window(MockWindow::new("org.example.bank", 300));
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("sig ");
    take(&seen);

    handoff.arm(id(1));
    os.script_at(InjectionPoint::BeforeStep(3), move |os| {
        os.focus(other).expect("open");
    });
    let request = InjectionRequest::new(
        id(1),
        vec![
            InjectionStep::DeleteBack(4),
            text_step("Regards,"),
            InjectionStep::Wait(500),
            text_step(" Ann"),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::FocusMoved)
    );
    assert_eq!(os.text(chat).as_deref(), Some("Regards,"));
    assert_eq!(
        os.text(other).as_deref(),
        Some(""),
        "nothing went to the other window"
    );
    assert_eq!(
        os.injected().last(),
        Some(&Injected::Cancelled(id(1), CancelReason::FocusMoved))
    );
}

#[test]
fn a_move_to_another_field_of_the_window_stops_the_expansion_too() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("brb ");
    take(&seen);

    handoff.arm(id(1));
    os.script_at(InjectionPoint::BeforeStep(1), move |os| {
        os.set_security(window, FieldSecurity::Secure);
    });
    let request = InjectionRequest::new(
        id(1),
        vec![InjectionStep::DeleteBack(4), text_step("be right back ")],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Cancelled(CancelReason::FocusMoved)
    );
    assert_eq!(os.text(window).as_deref(), Some(""));
}

#[test]
fn the_expansions_own_tab_ends_the_focus_check() {
    let os = MockPlatform::new();
    let form = editor(&os);
    let other = os.open_window(MockWindow::new("org.example.editor", 100));
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.type_text("me ");
    take(&seen);

    // The snippet's own Tab moves to the next field; the guard cannot tell that move from
    // another, so it stops checking after it.
    handoff.arm(id(1));
    os.script_at(InjectionPoint::BeforeStep(3), move |os| {
        os.focus(other).expect("open");
    });
    let request = InjectionRequest::new(
        id(1),
        vec![
            InjectionStep::DeleteBack(3),
            text_step("ann"),
            InjectionStep::PressKey {
                key: KeyName::Tab,
                shift: false,
                times: 1,
            },
            text_step("secret-free"),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(form).as_deref(), Some("ann\t"));
    assert_eq!(os.text(other).as_deref(), Some("secret-free"));
}

#[test]
fn atomic_batches_push_interleaved_keys_behind_the_end_marker() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.set_atomic_batches(true);
    os.type_text("brb ");
    take(&seen);

    handoff.arm(id(1));
    os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text("o"));
    os.interleave(InjectionPoint::BeforeStep(1), MockKey::text("m"));
    os.interleave(InjectionPoint::BeforeEndMarker, MockKey::text("w"));
    let request = InjectionRequest::new(
        id(1),
        vec![
            InjectionStep::DeleteBack(4),
            text_step("be right back"),
            text_step(" "),
        ],
    );
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Finished { retyped: 1 }
    );
    assert_eq!(os.text(window).as_deref(), Some("be right back omw"));
    let events = take(&seen);
    assert!(events[0].is_real_key());
    assert!(matches!(events[1], ListenerEvent::InjectionStarted { .. }));
    assert!(matches!(events[2], ListenerEvent::InjectionFinished { .. }));
    assert!(events[3].is_real_key() && events[4].is_real_key());
    assert_eq!(events.len(), 5);
}

#[test]
fn injection_is_refused_without_accessibility_and_by_elevated_windows() {
    let os = MockPlatform::new();
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    let admin = os.open_window(MockWindow::new("admin-tool", 9).elevated().text("brb "));
    os.focus(admin).expect("open");

    handoff.arm(id(1));
    let request = InjectionRequest::new(id(1), vec![InjectionStep::DeleteBack(4)]);
    let outcome = os.inject(&request, &handoff);
    assert_eq!(
        outcome,
        InjectionOutcome::Failed(PlatformError::InjectionRefused)
    );
    assert!(outcome.needs_cancel_event());
    assert_eq!(os.text(admin).as_deref(), Some("brb "));
    assert!(take(&seen).is_empty());

    let normal = editor(&os);
    os.set_permission(PermissionKind::Accessibility, Grant::Denied);
    assert_eq!(
        os.inject(&request, &handoff),
        InjectionOutcome::Failed(PlatformError::PermissionDenied(
            PermissionKind::Accessibility
        ))
    );
    assert_eq!(os.text(normal).as_deref(), Some(""));
    assert_eq!(
        os.injected(),
        [
            Injected::Failed(id(1), PlatformError::InjectionRefused),
            Injected::Failed(
                id(1),
                PlatformError::PermissionDenied(PermissionKind::Accessibility)
            ),
        ]
    );
}

#[test]
fn a_failing_paste_stops_the_expansion_without_an_end_marker() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let seen = listening_with(&os, &handoff);
    os.set_clipboard_busy(true);
    handoff.arm(id(1));
    let request = InjectionRequest::new(
        id(1),
        vec![
            text_step("before "),
            InjectionStep::Paste(ClipboardContent::text("pasted")),
            text_step(" after"),
        ],
    );
    let outcome = os.inject(&request, &handoff);
    assert!(matches!(
        outcome,
        InjectionOutcome::Failed(PlatformError::Os { .. })
    ));
    assert_eq!(
        os.text(window).as_deref(),
        Some("before "),
        "partial output"
    );
    let events = take(&seen);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ListenerEvent::InjectionStarted { .. }));
    assert!(matches!(os.injected().last(), Some(Injected::Failed(..))));
}

#[test]
fn injection_without_a_focused_window_changes_nothing() {
    let os = MockPlatform::new();
    let handoff = Arc::new(TypedHandoff::new());
    let _seen = listening_with(&os, &handoff);
    handoff.arm(id(1));
    assert_eq!(
        os.inject(
            &InjectionRequest::new(id(1), vec![text_step("x")]),
            &handoff
        ),
        InjectionOutcome::Finished { retyped: 0 }
    );
}

// ---------------------------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------------------------

fn service_endpoint() -> EndpointName {
    EndpointName::new(EndpointKind::Service, "/mock/data/run")
}

fn other_process(os: &MockPlatform, pid: u32) -> MockPlatform {
    os.with_identity(PeerIdentity {
        pid,
        user: os.identity().user.clone(),
        executable: Some("/mock/iamb".into()),
        trusted_binary: true,
    })
}

#[test]
fn an_endpoint_is_exclusive_until_closed() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    assert!(matches!(
        os.connect(&name, PeerPolicy::SameUser, WAIT),
        Err(PlatformError::EndpointNotFound)
    ));
    let listener = os.bind(&name, PeerPolicy::SameUser).expect("free name");
    assert!(matches!(
        other_process(&os, 2).bind(&name, PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    // The other endpoint of the same data root, and the same endpoint of another, are free.
    let sync = os
        .bind(
            &EndpointName::new(EndpointKind::Sync, "/mock/data/run"),
            PeerPolicy::SameUser,
        )
        .expect("free name");
    drop(sync);
    let elsewhere = os
        .bind(
            &EndpointName::new(EndpointKind::Service, "/tmp/test/run"),
            PeerPolicy::SameUser,
        )
        .expect("free name");
    drop(elsewhere);

    listener.close().expect("closes");
    assert!(matches!(listener.accept(), Err(PlatformError::Closed)));
    assert!(matches!(
        os.connect(&name, PeerPolicy::SameUser, WAIT),
        Err(PlatformError::EndpointNotFound)
    ));
    let again = os.bind(&name, PeerPolicy::SameUser).expect("released");
    drop(listener); // dropping the old listener must not release the new one
    assert!(matches!(
        os.bind(&name, PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    drop(again);
    assert!(
        os.bind(&name, PeerPolicy::SameUser).is_ok(),
        "drop releases"
    );
}

#[test]
fn bytes_flow_both_ways_and_each_side_knows_its_peer() {
    let os = MockPlatform::new();
    let cli = other_process(&os, 4242);
    let name = service_endpoint();
    let listener = os.bind(&name, PeerPolicy::InstalledBinary).expect("free");

    let mut client = cli
        .connect(&name, PeerPolicy::InstalledBinary, WAIT)
        .expect("server is trusted");
    let mut server = listener.accept().expect("client is trusted");
    assert_eq!(client.peer(), os.identity());
    assert_eq!(server.peer().pid, 4242);
    assert_eq!(server.peer(), cli.identity());

    client.write_all(b"\x05\x00\x00\x00hello").expect("writes");
    client.flush().expect("flushes");
    let mut frame = [0_u8; 9];
    server.read_exact(&mut frame).expect("reads");
    assert_eq!(&frame[4..], b"hello");

    server.write_all(b"ok").expect("writes");
    let mut small = [0_u8; 1];
    assert_eq!(client.read(&mut small).expect("reads"), 1);
    assert_eq!(client.read(&mut small).expect("reads"), 1);
    assert_eq!(small, *b"k");
    assert_eq!(client.read(&mut []).expect("empty read"), 0);
}

#[test]
fn reads_time_out_and_end_when_the_peer_goes_away() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    let listener = os.bind(&name, PeerPolicy::SameUser).expect("free");
    let mut client = os.connect(&name, PeerPolicy::SameUser, WAIT).expect("ok");
    let mut server = listener.accept().expect("ok");

    client
        .set_read_timeout(Some(Duration::from_millis(5)))
        .expect("sets");
    let mut buf = [0_u8; 8];
    assert_eq!(
        client.read(&mut buf).map_err(|e| e.kind()),
        Err(ErrorKind::TimedOut)
    );
    client.set_read_timeout(None).expect("sets");

    // Data written before the close is still read; then the stream ends.
    server.write_all(b"bye").expect("writes");
    drop(server);
    assert_eq!(client.read(&mut buf).expect("reads"), 3);
    assert_eq!(client.read(&mut buf).expect("end of stream"), 0);
    assert_eq!(
        client.write(b"x").map_err(|e| e.kind()),
        Err(ErrorKind::BrokenPipe)
    );
}

#[test]
fn a_clone_writes_while_another_thread_reads_and_shutdown_unblocks() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    let listener = os.bind(&name, PeerPolicy::SameUser).expect("free");
    let mut client = os.connect(&name, PeerPolicy::SameUser, WAIT).expect("ok");
    let server = listener.accept().expect("ok");

    let mut reader = server.try_clone().expect("clones");
    let mut pusher = server.try_clone().expect("clones");
    assert_eq!(reader.peer(), server.peer());
    let reading = thread::spawn(move || {
        let mut all = Vec::new();
        reader.read_to_end(&mut all).map(|_| all)
    });

    // The server pushes an event while its reader thread is blocked in a read.
    pusher.write_all(b"event").expect("writes");
    let mut event = [0_u8; 5];
    client.read_exact(&mut event).expect("reads");
    assert_eq!(&event, b"event");

    client.write_all(b"request").expect("writes");
    server.shutdown().expect("shuts down");
    assert_eq!(
        reading.join().expect("no panic").expect("read"),
        b"request".to_vec()
    );
    let mut buf = [0_u8; 1];
    assert_eq!(client.read(&mut buf).expect("end of stream"), 0);
}

#[test]
fn accept_blocks_until_a_client_connects_or_the_listener_closes() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    let listener: Arc<dyn LocalListener> =
        Arc::from(os.bind(&name, PeerPolicy::SameUser).expect("free"));

    let accepting = {
        let listener = Arc::clone(&listener);
        thread::spawn(move || listener.accept().map(|stream| stream.peer().pid))
    };
    let client = other_process(&os, 77)
        .connect(&name, PeerPolicy::SameUser, WAIT)
        .expect("ok");
    assert_eq!(accepting.join().expect("no panic"), Ok(77));
    drop(client);

    let accepting = {
        let listener = Arc::clone(&listener);
        thread::spawn(move || listener.accept().map(|_| ()))
    };
    listener.close().expect("closes");
    assert_eq!(
        accepting.join().expect("no panic"),
        Err(PlatformError::Closed)
    );
}

#[test]
fn a_server_rejects_other_users_and_untrusted_binaries() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    let listener = os.bind(&name, PeerPolicy::InstalledBinary).expect("free");

    let stranger = os.with_identity(PeerIdentity {
        pid: 5,
        user: "someone-else".to_owned(),
        executable: None,
        trusted_binary: true,
    });
    let unsigned = os.with_identity(PeerIdentity {
        pid: 6,
        user: os.identity().user.clone(),
        executable: Some("/tmp/fake-iamb".into()),
        trusted_binary: false,
    });

    // Each client accepts the server (it only requires the same user here, or lies about
    // itself); the server refuses both, and they read the end of the stream.
    let mut first = unsigned
        .connect(&name, PeerPolicy::SameUser, WAIT)
        .expect("server passes");
    assert!(matches!(
        listener.accept(),
        Err(PlatformError::PeerRejected(PeerRejection::UntrustedBinary))
    ));
    let mut buf = [0_u8; 1];
    assert_eq!(first.read(&mut buf).expect("closed by the server"), 0);

    // The stranger's own check already fails: the server runs as another user.
    assert!(matches!(
        stranger.connect(&name, PeerPolicy::SameUser, WAIT),
        Err(PlatformError::PeerRejected(PeerRejection::OtherUser))
    ));

    // A server with the development policy still refuses another user.
    drop(listener);
    let listener = stranger.bind(&name, PeerPolicy::SameUser).expect("free");
    assert!(matches!(
        os.connect(&name, PeerPolicy::SameUser, WAIT),
        Err(PlatformError::PeerRejected(PeerRejection::OtherUser))
    ));
    drop(listener);
}

#[test]
fn a_client_refuses_a_squatting_server() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    // Another program of the same user took the name first (ADR-0006).
    let squatter = os.with_identity(PeerIdentity {
        pid: 666,
        user: os.identity().user.clone(),
        executable: Some("/tmp/squatter".into()),
        trusted_binary: false,
    });
    let listener = squatter.bind(&name, PeerPolicy::SameUser).expect("free");
    assert!(matches!(
        os.connect(&name, PeerPolicy::InstalledBinary, WAIT),
        Err(PlatformError::PeerRejected(PeerRejection::UntrustedBinary))
    ));
    // The real service cannot start under that name and says so.
    assert!(matches!(
        os.bind(&name, PeerPolicy::InstalledBinary),
        Err(PlatformError::EndpointInUse)
    ));
    // Nothing reached the squatter: closing shows an empty backlog.
    listener.close().expect("closes");
    assert!(matches!(listener.accept(), Err(PlatformError::Closed)));
}

#[test]
fn clients_waiting_in_the_backlog_are_closed_with_the_listener() {
    let os = MockPlatform::new();
    let name = service_endpoint();
    let listener = os.bind(&name, PeerPolicy::SameUser).expect("free");
    let mut waiting = os.connect(&name, PeerPolicy::SameUser, WAIT).expect("ok");
    drop(listener);
    let mut buf = [0_u8; 1];
    assert_eq!(waiting.read(&mut buf).expect("end of stream"), 0);
}

// ---------------------------------------------------------------------------------------------
// The whole platform
// ---------------------------------------------------------------------------------------------

#[test]
fn the_mock_is_a_platform_and_its_handles_share_one_machine() {
    let os = MockPlatform::new();
    let platform: Arc<dyn Platform> = Arc::new(os.clone());
    let listener: Arc<dyn KeyListener> = platform.clone();
    let injector: Arc<dyn Injector> = platform.clone();
    let focus: Arc<dyn FocusInspector> = platform.clone();
    let clipboard: Arc<dyn ClipboardAccess> = platform.clone();
    let hotkeys: Arc<dyn Hotkeys> = platform.clone();
    let permissions: Arc<dyn Permissions> = platform.clone();
    let login: Arc<dyn LoginItem> = platform.clone();
    let memory: Arc<dyn MemoryLock> = platform.clone();
    let endpoint: Arc<dyn LocalEndpoint> = platform;

    let window = editor(&os);
    let handoff = Arc::new(TypedHandoff::new());
    let sink_handoff = Arc::clone(&handoff);
    listener
        .start(Box::new(move |event: ListenerEvent| {
            sink_handoff.observe(&event);
        }))
        .expect("starts");
    handoff.arm(id(1));
    assert_eq!(
        injector.inject(
            &InjectionRequest::new(id(1), vec![text_step("hi")]),
            &handoff
        ),
        InjectionOutcome::Finished { retyped: 0 }
    );
    assert_eq!(os.text(window).as_deref(), Some("hi"));
    assert_eq!(focus.focus_token(), Ok(window));
    assert!(clipboard.read_text().expect("readable").is_none());
    hotkeys
        .register(HotkeyId(1), Chord::parse("F8").expect("chord"))
        .expect("free");
    assert!(permissions.permission_status().all_satisfied());
    assert_eq!(login.login_item_status(), Ok(LoginItemStatus::Disabled));
    memory.lock(handoff.region()).expect("locks");
    assert!(
        endpoint
            .bind(&service_endpoint(), PeerPolicy::SameUser)
            .is_ok()
    );
    assert_eq!(os.locked_regions().len(), 1);
    assert_eq!(os.registered_hotkeys().len(), 1);

    let shown = format!("{os:?}");
    assert!(shown.starts_with("MockPlatform { identity: PeerIdentity"));
}

#[test]
fn keys_typed_from_two_threads_keep_one_order() {
    let os = MockPlatform::new();
    let window = editor(&os);
    let seen = listening(&os);
    let threads: Vec<_> = ["aaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbb"]
        .into_iter()
        .map(|text| {
            let os = os.clone();
            thread::spawn(move || os.type_text(text))
        })
        .collect();
    for t in threads {
        t.join().expect("no panic");
    }
    // The field and the listener saw the same interleaving.
    let reported: String = take(&seen)
        .iter()
        .map(|event| match event {
            ListenerEvent::Char { ch, .. } => *ch.expose(),
            _ => '?',
        })
        .collect();
    assert_eq!(os.text(window), Some(reported));
    assert_eq!(os.caret(window), Some(40));
}

#[test]
fn configure_records_every_call_and_keeps_the_last_valid_settings() {
    let os = MockPlatform::new();
    assert_eq!(os.injection_settings(), "");
    let good = r#"{"apps": {"editor.exe": {"method": "keys"}}}"#;
    assert_eq!(os.configure(good), Ok(()));
    assert_eq!(os.injection_settings(), good);
    // A macOS-only field is refused in the default (Windows) dialect; the settings stay.
    let mac_only = r#"{"defaults": {"chunk_units": 4}}"#;
    assert!(matches!(
        os.configure(mac_only),
        Err(PlatformError::Invalid { .. })
    ));
    assert_eq!(os.injection_settings(), good);
    os.set_settings_dialect(SettingsDialect::MacOs);
    assert_eq!(os.configure(mac_only), Ok(()));
    assert_eq!(os.injection_settings(), mac_only);
    assert_eq!(os.configure(""), Ok(()));
    assert_eq!(os.injection_settings(), "");
    assert_eq!(os.configured(), [good, mac_only, mac_only, ""]);
}

#[test]
fn an_injector_without_settings_accepts_only_the_empty_string() {
    struct Plain;
    impl Injector for Plain {
        fn inject(&self, _: &InjectionRequest, _: &TypedHandoff) -> InjectionOutcome {
            InjectionOutcome::Finished { retyped: 0 }
        }
    }
    assert_eq!(Plain.configure(""), Ok(()));
    assert_eq!(Plain.configure("{}"), Err(PlatformError::Unsupported));
    assert_eq!(Plain.configure(" "), Err(PlatformError::Unsupported));
}

/// Events with the window each went to.
type Targeted = Arc<Mutex<Vec<(ListenerEvent, Option<KeyTarget>)>>>;

/// A sink that records each event with the window it went to.
struct Targets(Targeted);

impl EventSink for Targets {
    fn deliver(&mut self, event: ListenerEvent) {
        lock(&self.0).push((event, None));
    }

    fn deliver_to(&mut self, event: ListenerEvent, target: KeyTarget) {
        lock(&self.0).push((event, Some(target)));
    }
}

#[test]
fn real_keys_name_the_window_they_went_to() {
    let os = MockPlatform::new();
    let editor = editor(&os);
    let bank = os.open_window(MockWindow::new("org.example.bank", 200).secure());
    let seen = Arc::new(Mutex::new(Vec::new()));
    os.start(Box::new(Targets(Arc::clone(&seen))))
        .expect("listener starts");
    os.type_text("a");
    os.focus(bank).expect("open");
    os.click();
    os.press(KeyName::Backspace);
    os.set_composing(true);
    os.lose_events();
    let seen = std::mem::take(&mut *lock(&seen));
    let targets: Vec<Option<KeyTarget>> = seen.iter().map(|(_, t)| *t).collect();
    assert_eq!(
        targets,
        [
            Some(KeyTarget::window(editor)),
            Some(KeyTarget::window(bank)),
            Some(KeyTarget::window(bank)),
            // Not keys: no target.
            None,
            None,
        ]
    );
    // With nothing focused a key goes nowhere and names no window.
    os.close_window(bank);
    let seen = Arc::new(Mutex::new(Vec::new()));
    os.stop().expect("stops");
    os.start(Box::new(Targets(Arc::clone(&seen))))
        .expect("listener starts");
    os.type_text("b");
    let seen = std::mem::take(&mut *lock(&seen));
    assert!(matches!(
        seen.as_slice(),
        [(ListenerEvent::Char { .. }, None)]
    ));
}

#[test]
fn held_focus_reports_arrive_in_order_when_released() {
    let os = MockPlatform::new();
    let editor = editor(&os);
    let bank = os.open_window(MockWindow::new("org.example.bank", 200));
    let reports = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&reports);
    os.watch(Box::new(move |change: FocusChange| {
        lock(&log).push(change.token);
    }))
    .expect("watching");
    assert_eq!(*lock(&reports), [editor], "the current focus, at once");

    os.hold_focus_reports(true);
    os.hold_focus_reports(true); // holding twice keeps what is held
    os.focus(bank).expect("open");
    os.set_security(bank, FieldSecurity::Secure);
    os.focus(editor).expect("open");
    assert_eq!(*lock(&reports), [editor], "nothing reported while held");
    assert_eq!(os.focus_token(), Ok(editor), "the focus itself moved");

    os.hold_focus_reports(false);
    assert_eq!(*lock(&reports), [editor, bank, bank, editor]);
    os.focus(bank).expect("open");
    assert_eq!(lock(&reports).len(), 5, "reported at once again");
}
