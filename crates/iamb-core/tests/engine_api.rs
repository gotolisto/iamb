//! Unit tests of the engine API beyond the scripted sessions: the snapshot, `dry_run`, the
//! state accessors and the small public types.

#[path = "../../iamb-testkit/src/env.rs"]
#[allow(
    dead_code,
    reason = "shared with the testkit crate; not every helper is used here"
)]
mod env;

use env::FakeEnv;
use iamb_core::engine::{
    Action, AppId, BoundaryKind, Diagnostic, Engine, EngineSettings, Environment, Event,
    ExclusionReason, ExpansionId, FormSessionId, InjectionState, Reason, SecureState, Snapshot,
    SuspendReason, dry_run,
};
use iamb_core::matcher::Delimiters;
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp, Trigger};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::{ErrorKind, FormValues, SnippetSource};

fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

fn snippet(n: u8, abbr: &str, body: &str) -> Snippet {
    Snippet::new_defaults(
        SyncMeta::new(id(n), Timestamp(0)),
        id(0),
        Abbreviation::new(abbr).expect("abbreviation"),
        "",
        body,
        None,
    )
}

fn texts(actions: &[Action]) -> String {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::TypeText { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn snapshot_reference_and_matchable_sets() {
    let mut deleted = snippet(9, "gone", "x");
    deleted.meta.deleted_at = Some(Timestamp(1));
    let mut disabled = snippet(3, "off", "disabled body");
    disabled.enabled = false;
    let mut disabled_bad = snippet(8, "offbad", "{time fmt=\"%Q\"}");
    disabled_bad.enabled = false;
    let snapshot = Snapshot::build(
        [
            snippet(1, "BRB", "first"),
            snippet(2, "brb", "loser"),
            disabled,
            disabled_bad,
            snippet(4, "bad", "{date"),
            snippet(1, "again", "same id twice"),
            deleted,
            snippet(5, "οδος", "final sigma"),
            snippet(6, "οδοσ", "medial sigma"),
            snippet(7, "a", "short"),
        ],
        EngineSettings::default(),
    );
    assert_eq!(snapshot.len(), 8);
    assert!(!snapshot.is_empty());
    assert!(
        snapshot.snippet(id(9)).is_none(),
        "deleted snippets are skipped"
    );
    assert_eq!(
        snapshot.snippet(id(1)).map(|s| s.body.as_str()),
        Some("first"),
        "a repeated id keeps the first"
    );
    assert!(
        snapshot.snippet(id(2)).is_some(),
        "losers stay in the reference set"
    );
    assert!(
        snapshot.compiled(id(2)).is_none(),
        "losers are not compiled"
    );
    assert!(matches!(snapshot.compiled(id(3)), Some(Ok(_))));
    assert!(matches!(snapshot.compiled(id(8)), Some(Err(_))));
    assert!(snapshot.compiled(id(99)).is_none());

    let reasons: Vec<(Id, String)> = snapshot
        .excluded()
        .iter()
        .map(|e| {
            let r = match &e.reason {
                ExclusionReason::Duplicate { kept } => format!("dup of {:?}", kept.as_bytes()[0]),
                ExclusionReason::Template(t) => t.code().to_owned(),
            };
            (e.snippet, r)
        })
        .collect();
    assert_eq!(
        reasons,
        vec![
            (id(2), "dup of 1".to_owned()),
            (id(4), "E_UNCLOSED_MACRO".to_owned()),
            (id(6), "dup of 5".to_owned()),
        ]
    );
    // The matchable set: BRB, οδος, a.
    assert_eq!(snapshot.matcher().len(), 3);

    // {snippet} lookups: case-sensitive, losers never resolve, disabled ones do.
    assert_eq!(snapshot.body("BRB"), Some("first"));
    assert_eq!(snapshot.body("brb"), None);
    assert_eq!(snapshot.body("Brb"), None);
    assert_eq!(snapshot.body("off"), Some("disabled body"));
    assert_eq!(snapshot.body("missing"), None);

    let debug = format!("{snapshot:?}");
    assert!(debug.contains("snippets: 8"), "{debug}");
    assert!(Snapshot::default().is_empty());
    assert_eq!(snapshot.settings(), &EngineSettings::default());
}

#[test]
fn snapshot_reports_shadowing() {
    let mut ad = snippet(1, "ad", "advert");
    ad.trigger = Trigger::Immediate;
    let snapshot = Snapshot::build(
        [ad, snippet(2, "add", "address")],
        EngineSettings::default(),
    );
    let shadowed = snapshot.shadowed();
    assert_eq!(shadowed.len(), 1);
    assert_eq!((shadowed[0].by, shadowed[0].hidden), (id(1), id(2)));
}

#[test]
fn snapshot_is_cheap_to_clone() {
    let a = Snapshot::build([snippet(1, "brb", "x")], EngineSettings::default());
    let b = a.clone();
    assert!(std::ptr::eq(a.matcher(), b.matcher()));
}

#[test]
fn dry_run_expands_every_abbreviation() {
    let snapshot = Snapshot::build(
        [
            snippet(1, "brb", "be right back"),
            snippet(2, "omw", "on my way"),
            snippet(3, "dear", r#"Dear {field name="Name"},"#),
        ],
        EngineSettings::default(),
    );
    let env = FakeEnv::default();
    let actions = dry_run(&snapshot, "brb omw.", &env, None);
    assert_eq!(texts(&actions), "be right back on my way.");
    assert_eq!(
        actions
            .iter()
            .filter(|a| matches!(a, Action::Expanded { .. }))
            .count(),
        2
    );

    let values: FormValues = [("Name", "Kim")].into_iter().collect();
    let actions = dry_run(&snapshot, "dear brb ", &env, Some(&values));
    assert!(matches!(actions[0], Action::ShowForm { .. }));
    assert_eq!(texts(&actions), "Dear Kim, be right back ");

    // Without values the form stays open and later matches are dropped.
    let actions = dry_run(&snapshot, "dear brb ", &env, None);
    assert_eq!(actions.len(), 2);
    assert!(matches!(actions[0], Action::ShowForm { .. }));
    assert!(matches!(
        &actions[1],
        Action::Diagnostic(Diagnostic { reason: Reason::FormOpen, snippet: Some(s), .. }) if *s == id(1)
    ));

    assert!(dry_run(&snapshot, "", &env, None).is_empty());
}

#[test]
fn environment_is_read_once_per_expansion() {
    let snapshot = Snapshot::build(
        [
            snippet(1, "c", "{clipboard}{clipboard} {date}{time}"),
            snippet(2, "p", "plain"),
        ],
        EngineSettings::default(),
    );
    let env = FakeEnv::default().with_clipboard("x");
    let actions = dry_run(&snapshot, "c p ", &env, None);
    assert_eq!(texts(&actions), "xx 2026-03-1814:05 plain ");
    assert_eq!(env.now_calls(), 2);
    assert_eq!(env.clipboard_calls(), 1);
}

#[test]
fn engine_state_accessors() {
    let snapshot = Snapshot::build(
        [
            snippet(1, "brb", "be right back"),
            snippet(2, "dear", r#"Dear {field name="Name"},"#),
        ],
        EngineSettings::default(),
    );
    let env = FakeEnv::default();
    let mut engine = Engine::new(snapshot);
    assert!(engine.is_suspended());
    assert_eq!(engine.injection_state(), InjectionState::Idle);
    assert_eq!(engine.snapshot().len(), 2);
    engine.handle(Event::SecureState(SecureState::Normal), &env);
    assert!(!engine.is_suspended());

    let mut out = Vec::new();
    for ch in "brb ".chars() {
        out.extend(engine.handle(Event::char(ch, 0), &env));
    }
    let Some(Action::Expanded { expansion, .. }) = out.last().cloned() else {
        panic!("no expansion: {out:?}");
    };
    assert_eq!(engine.injection_state(), InjectionState::Pending(expansion));
    engine.handle(Event::InjectionStarted { expansion, seq: 9 }, &env);
    assert_eq!(
        engine.injection_state(),
        InjectionState::Injecting(expansion)
    );
    engine.handle(Event::InjectionFinished { expansion, seq: 10 }, &env);
    assert_eq!(engine.injection_state(), InjectionState::Idle);

    let shown = engine.handle(Event::ExpandRequest { snippet: id(2) }, &env);
    let Some(Action::ShowForm { session, .. }) = shown.first() else {
        panic!("no form");
    };
    let session = *session;
    assert_eq!(engine.open_form(), Some(session));
    assert!(!engine.remembers_form_values(id(2)));
    let values: FormValues = [("Name", "Kim")].into_iter().collect();
    engine.handle(
        Event::FormSubmitted {
            session,
            values: Sensitive::new(values),
        },
        &env,
    );
    assert!(engine.remembers_form_values(id(2)));
    engine.forget_form_values();
    assert!(!engine.remembers_form_values(id(2)));
    assert!(format!("{engine:?}").contains("Engine"));
}

#[test]
fn custom_delimiters_and_sentinel_guard() {
    // NUL is a delimiter here, so no placeholder is pushed after an expansion.
    let settings = EngineSettings {
        delimiters: Delimiters::new([' ', '\0']),
        ..EngineSettings::default()
    };
    let mut zz = snippet(1, "zz", "foo");
    zz.trigger = Trigger::Immediate;
    let snapshot = Snapshot::build([zz], settings);
    let env = FakeEnv::default();
    let mut engine = Engine::new(snapshot);
    engine.handle(Event::SecureState(SecureState::Normal), &env);
    engine.handle(Event::char('z', 0), &env);
    let out = engine.handle(Event::char('z', 1), &env);
    assert!(matches!(out.last(), Some(Action::Expanded { .. })));
    assert_eq!(engine.buffered_len(), 0);
}

#[test]
fn small_types() {
    assert_eq!(BoundaryKind::Enter.as_delimiter(), Some('\n'));
    assert_eq!(BoundaryKind::Tab.as_delimiter(), Some('\t'));
    assert_eq!(BoundaryKind::Click.as_delimiter(), None);
    assert_eq!(FormSessionId::from_raw(7).raw(), 7);
    assert_eq!(ExpansionId::from_raw(8).raw(), 8);

    let reasons = [
        (Reason::Template(ErrorKind::DateRange), "E_DATE_RANGE"),
        (Reason::FormOpen, "form_open"),
        (Reason::UnknownFormSession, "unknown_form_session"),
        (Reason::NoLastForm, "no_last_form"),
        (Reason::UnknownSnippet, "unknown_snippet"),
        (Reason::InjectionBusy, "injection_busy"),
        (Reason::TypedDuringInjection, "typed_during_injection"),
        (Reason::InjectionCancelled, "injection_cancelled"),
        (
            Reason::UnexpectedInjectionEvent,
            "unexpected_injection_event",
        ),
        (
            Reason::Suspended(SuspendReason::SecureField),
            "suspended_secure_field",
        ),
        (
            Reason::Suspended(SuspendReason::ExcludedApp),
            "suspended_excluded_app",
        ),
        (
            Reason::Suspended(SuspendReason::UnknownFocus),
            "suspended_unknown_focus",
        ),
    ];
    for (reason, code) in reasons {
        assert_eq!(reason.code(), code);
        assert_eq!(reason.to_string(), code);
    }
    let d = Diagnostic::new(Reason::FormOpen)
        .with_snippet(id(1))
        .with_expansion(ExpansionId::from_raw(2));
    assert_eq!(d.snippet, Some(id(1)));
    assert_eq!(d.expansion, Some(ExpansionId::from_raw(2)));
    assert_eq!(d.app, None);

    let s: Sensitive<String> = String::from("x").into();
    assert_eq!(s.clone().into_inner(), "x");
    assert!(Sensitive::<String>::default().expose().is_empty());
    assert!(s == Sensitive::new(String::from("x")));
    assert_eq!(AppId("a".into()), AppId("a".into()));

    for action in [
        Action::DeleteBack(1),
        Action::MoveCaretBack(2),
        Action::Wait(3),
        Action::PressKey {
            key: iamb_core::template::KeyName::Tab,
            shift: true,
            times: 2,
        },
    ] {
        assert!(!format!("{action:?}").is_empty());
    }
}

#[test]
fn fake_env_counts_calls() {
    let env = FakeEnv::default();
    assert_eq!(env.clipboard_text(), None);
    env.set_clipboard(Some("c".into()));
    assert_eq!(env.clipboard_text().as_deref(), Some("c"));
    let _ = env.now();
    assert_eq!((env.now_calls(), env.clipboard_calls()), (1, 2));
}

#[test]
fn palette_expansion_while_suspended_leaves_the_buffer_empty() {
    // Found by the property test: the placeholder for "a letter precedes the caret" must not be
    // pushed while suspended, when the buffer has to stay empty.
    let snapshot = Snapshot::build([snippet(1, "w", "word")], EngineSettings::default());
    let env = FakeEnv::default();
    let mut engine = Engine::new(snapshot);
    assert!(engine.is_suspended());
    let out = engine.handle(Event::ExpandRequest { snippet: id(1) }, &env);
    assert_eq!(texts(&out), "word");
    assert_eq!(engine.buffered_len(), 0);
    engine.handle(Event::char('x', 0), &env);
    assert_eq!(engine.buffered_len(), 0);
}
