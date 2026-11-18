//! Privacy guarantees of the engine (architecture §8 rules 1–4, ADR-0012): typed characters,
//! form values and clipboard text never appear in `Debug` output of events, actions,
//! diagnostics or the engine, and `Sensitive` cannot be formatted or serialised.

#[path = "../../iamb-testkit/src/env.rs"]
#[allow(
    dead_code,
    reason = "shared with the testkit crate; not every helper is used here"
)]
mod env;

use env::FakeEnv;
use iamb_core::engine::{
    Action, AppId, BoundaryKind, Engine, EngineSettings, Event, ExpansionId, FormSessionId,
    SecureState, Snapshot,
};
use iamb_core::matcher::KEY_BUFFER_CAPACITY;
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::FormValues;

/// Fails to compile if `$ty` implements `$tr` (see `matcher_privacy.rs`).
macro_rules! assert_not_impl {
    ($ty:ty: $($tr:path),+ $(,)?) => {
        $(
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn some_item() {}
                }
                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
                #[allow(dead_code)]
                struct Invalid;
                impl<T: ?Sized + $tr> AmbiguousIfImpl<Invalid> for T {}
                let _ = <$ty as AmbiguousIfImpl<_>>::some_item;
            };
        )+
    };
}

assert_not_impl!(Sensitive<char>: std::fmt::Debug, std::fmt::Display, serde::Serialize);
assert_not_impl!(Sensitive<String>: std::fmt::Debug, std::fmt::Display, serde::Serialize);
assert_not_impl!(Sensitive<FormValues>: std::fmt::Debug, std::fmt::Display, serde::Serialize);
assert_not_impl!(Event: serde::Serialize, std::fmt::Display, Clone);
assert_not_impl!(Engine: serde::Serialize, Clone);

const fn assert_send_sync<T: Send + Sync>() {}
const fn assert_send<T: Send>() {}

#[test]
fn snapshot_is_shareable_and_engine_can_move_threads() {
    assert_send_sync::<Snapshot>();
    assert_send::<Engine>();
    assert_send::<Event>();
    assert_send::<Action>();
}

const TYPED_CANARY: char = 'Ж';
const FORM_CANARY: &str = "CANARY-FORM-7f3a";
const CLIP_CANARY: &str = "CANARY-CLIP-91c2";
const APP: &str = "editor";

fn snippet(n: u8, abbr: &str, body: &str) -> Snippet {
    Snippet::new_defaults(
        SyncMeta::new(Id::from_bytes([n; 16]), Timestamp(0)),
        Id::from_bytes([0; 16]),
        Abbreviation::new(abbr).expect("abbreviation"),
        "",
        body,
        None,
    )
}

fn assert_clean(what: &str, debug: &str) {
    for canary in [TYPED_CANARY.to_string().as_str(), FORM_CANARY, CLIP_CANARY] {
        assert!(!debug.contains(canary), "{what} leaks {canary:?}: {debug}");
    }
}

#[test]
fn event_debug_never_shows_typed_text_or_values() {
    let values: FormValues = [("Name", FORM_CANARY)].into_iter().collect();
    let events = [
        Event::char(TYPED_CANARY, 1),
        Event::Backspace { seq: 2 },
        Event::Boundary {
            kind: BoundaryKind::Enter,
            seq: 3,
        },
        Event::FocusChanged(AppId(APP.to_owned())),
        Event::SecureState(SecureState::Secure),
        Event::Composing(true),
        Event::Paused(false),
        Event::Overflow,
        Event::InjectionStarted {
            expansion: ExpansionId::from_raw(1),
            seq: 4,
        },
        Event::InjectionFinished {
            expansion: ExpansionId::from_raw(1),
            seq: 5,
        },
        Event::InjectionCancelled {
            expansion: ExpansionId::from_raw(1),
        },
        Event::ExpandRequest {
            snippet: Id::from_bytes([1; 16]),
        },
        Event::FormSubmitted {
            session: FormSessionId::from_raw(1),
            values: Sensitive::new(values),
        },
        Event::FormCancelled {
            session: FormSessionId::from_raw(1),
        },
        Event::ReopenLastForm,
        Event::SnapshotChanged(Snapshot::empty()),
    ];
    for event in &events {
        let debug = format!("{event:?}");
        assert_clean("event", &debug);
        assert!(!debug.is_empty());
    }
    let char_debug = format!("{:?}", events[0]);
    assert!(char_debug.contains("<redacted>"), "{char_debug}");
    let submit_debug = format!("{:?}", events[12]);
    assert!(submit_debug.contains("<redacted>"), "{submit_debug}");
}

#[test]
fn actions_and_engine_debug_never_show_values_or_clipboard() {
    let snapshot = Snapshot::build(
        [
            snippet(1, "cb", "{clipboard}"),
            snippet(2, "fm", r#"{field name="Name"}"#),
        ],
        EngineSettings::default(),
    );
    let env = FakeEnv::default().with_clipboard(CLIP_CANARY);
    let mut engine = Engine::new(snapshot);
    let mut all = Vec::new();
    all.extend(engine.handle(Event::FocusChanged(AppId(APP.to_owned())), &env));
    all.extend(engine.handle(Event::SecureState(SecureState::Normal), &env));
    for ch in "cb ".chars() {
        all.extend(engine.handle(Event::char(ch, 0), &env));
    }
    let Some(Action::Expanded { expansion, .. }) = all.last().cloned() else {
        panic!("expected an expansion");
    };
    for event in [
        Event::InjectionStarted { expansion, seq: 0 },
        Event::char(TYPED_CANARY, 0),
        Event::InjectionFinished { expansion, seq: 0 },
    ] {
        all.extend(engine.handle(event, &env));
    }
    for ch in "fm ".chars() {
        all.extend(engine.handle(Event::char(ch, 0), &env));
    }
    let session = engine.open_form().expect("form open");
    let values: FormValues = [("Name", FORM_CANARY)].into_iter().collect();
    all.extend(engine.handle(
        Event::FormSubmitted {
            session,
            values: Sensitive::new(values),
        },
        &env,
    ));
    all.extend(engine.handle(Event::ReopenLastForm, &env));
    all.extend(engine.handle(Event::char(TYPED_CANARY, 0), &env));

    // The canaries really are in the actions...
    assert!(
        all.iter()
            .any(|a| matches!(a, Action::TypeText { text, .. } if text.contains(CLIP_CANARY)))
    );
    assert!(all.iter().any(
        |a| matches!(a, Action::ShowForm { last_values: Some(v), .. } if v.expose().get("Name") == Some(FORM_CANARY))
    ));
    assert!(all.iter().any(|a| matches!(a, Action::Diagnostic(_))));
    // ...but never in their Debug output, nor in the engine's.
    assert_clean("actions", &format!("{all:?}"));
    assert_clean("engine", &format!("{engine:?}"));
    let paste = Action::Paste {
        plain: CLIP_CANARY.to_owned(),
        markdown_html: FORM_CANARY.to_owned(),
        attachments: Vec::new(),
    };
    assert_clean("paste", &format!("{paste:?}"));
}

#[test]
fn nothing_typed_while_suspended_is_stored() {
    let mut engine = Engine::new(Snapshot::build(
        [snippet(1, "brb", "be right back")],
        EngineSettings::default(),
    ));
    let env = FakeEnv::default();
    for ch in "brb ".chars() {
        assert!(engine.handle(Event::char(ch, 0), &env).is_empty());
        assert_eq!(engine.buffered_len(), 0);
    }
    engine.handle(Event::SecureState(SecureState::Secure), &env);
    for ch in "brb ".chars() {
        assert!(engine.handle(Event::char(ch, 0), &env).is_empty());
        assert_eq!(engine.buffered_len(), 0);
    }
    assert_eq!(env.now_calls(), 0);
}

/// The service locks the keystroke buffer's allocation against swapping (architecture §4.1):
/// the range the engine reports covers a whole buffer and stays the same across events,
/// expansions, overflow, a snapshot change and a move of the engine value.
#[test]
fn buffer_region_is_non_empty_and_stable() {
    let snapshot = Snapshot::build(
        [
            snippet(1, "brb", "be right back"),
            snippet(2, "sig", r#"Regards, {field name="Name"}"#),
        ],
        EngineSettings::default(),
    );
    let env = FakeEnv::default();
    let mut engine = Engine::new(snapshot.clone());
    let region = engine.buffer_region();
    let (address, len) = region;
    assert_ne!(address, 0);
    assert!(
        len >= KEY_BUFFER_CAPACITY * std::mem::size_of::<char>(),
        "{len} bytes cannot hold {KEY_BUFFER_CAPACITY} characters"
    );
    assert_eq!(address % std::mem::align_of::<char>(), 0);

    let mut events = vec![
        Event::FocusChanged(AppId(APP.to_owned())),
        Event::SecureState(SecureState::Normal),
    ];
    events.extend("x brb ".chars().map(|ch| Event::char(ch, 0)));
    events.extend([
        Event::InjectionStarted {
            expansion: ExpansionId::from_raw(1),
            seq: 0,
        },
        Event::InjectionFinished {
            expansion: ExpansionId::from_raw(1),
            seq: 0,
        },
        Event::Backspace { seq: 0 },
        Event::Boundary {
            kind: BoundaryKind::Enter,
            seq: 0,
        },
        Event::Overflow,
        Event::SnapshotChanged(snapshot),
    ]);
    events.extend("sig ".chars().map(|ch| Event::char(ch, 0)));
    events.push(Event::FormCancelled {
        session: FormSessionId::from_raw(1),
    });
    // More characters than the buffer holds, so the oldest fall off.
    events.extend(std::iter::repeat_n('a', KEY_BUFFER_CAPACITY * 2).map(|ch| Event::char(ch, 0)));
    for event in events {
        engine.handle(event, &env);
        assert_eq!(engine.buffer_region(), region);
    }
    assert!(engine.buffered_len() > 0);

    let moved = Box::new(engine);
    assert_eq!(
        moved.buffer_region(),
        region,
        "moving the engine moves no typed text"
    );
    let moved = *moved;
    assert_eq!(moved.buffer_region(), region);
}
