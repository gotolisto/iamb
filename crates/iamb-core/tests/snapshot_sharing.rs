//! A snapshot build shares parsed and expanded bodies between every template it compiles
//! (review 1.9 finding 2). These tests check that the sharing changes nothing a resolve reports
//! and that a library with a shared hub builds in linear time.

use iamb_core::engine::{EngineSettings, Snapshot};
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp};
use iamb_core::template::{FieldSpec, Template, TemplateError};
use proptest::prelude::*;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

fn snippet(n: u32, abbr: &str, body: &str) -> Snippet {
    let mut bytes = [0u8; 16];
    bytes[12..].copy_from_slice(&n.to_be_bytes());
    Snippet::new_defaults(
        SyncMeta::new(Id::from_bytes(bytes), Timestamp(0)),
        Id::from_bytes([0; 16]),
        Abbreviation::new(abbr).expect("abbreviation"),
        "",
        body,
        None,
    )
}

/// The time budget of a test that is meant for release builds; debug builds get more room.
fn budget(release_ms: u64) -> Duration {
    let factor = if cfg!(debug_assertions) { 25 } else { 1 };
    Duration::from_millis(release_ms * factor)
}

/// The library shapes of review 1.9 finding 2 and follow-up finding N1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// n plain leaves, one hub that includes them, n users `Dear all, {snippet abbr=hub}`.
    Plain,
    /// As `Plain`, but every leaf asks for its own field: the hub's form has n fields.
    LeafFields,
    /// As `LeafFields`, and every user has a field of its own (`Dear {field name=who}, …`).
    OwnField,
    /// As `LeafFields`, and every user starts with an `{if}` of its own.
    OwnIf,
    /// The leaves are split between two hubs; every user includes both.
    TwoHubs,
}

/// `leaves` leaves, the hub(s), then `users` users, in that order.
fn hub_library(shape: Shape, leaves: u32, users: u32) -> Vec<Snippet> {
    let mut v = Vec::new();
    let (mut hub, mut hubb) = (String::new(), String::new());
    for i in 0..leaves {
        let leaf = if shape == Shape::Plain {
            "some leaf text".to_owned()
        } else {
            format!("{{field name=f{i}}}")
        };
        v.push(snippet(i, &format!("leaf{i}"), &leaf));
        let to = if shape == Shape::TwoHubs && i % 2 == 1 {
            &mut hubb
        } else {
            &mut hub
        };
        write!(to, "{{snippet abbr=leaf{i}}}").expect("write to a string");
    }
    v.push(snippet(leaves, "hub", &hub));
    if shape == Shape::TwoHubs {
        v.push(snippet(leaves + users + 1, "hubb", &hubb));
    }
    let body = match shape {
        Shape::Plain | Shape::LeafFields => "Dear all, {snippet abbr=hub}",
        Shape::OwnField => "Dear {field name=who}, {snippet abbr=hub}",
        Shape::OwnIf => "{if field=f0}x{endif}{snippet abbr=hub}",
        Shape::TwoHubs => "{snippet abbr=hub}{snippet abbr=hubb}",
    };
    for i in 0..users {
        v.push(snippet(leaves + 1 + i, &format!("user{i}"), body));
    }
    v
}

/// The resident set size of this process in MiB, where the OS reports it (Linux).
fn resident_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

/// Builds `library`, checks the time (200 ms in release) and memory bounds, and returns the
/// snapshot. The memory bound is checked where the OS reports the resident set (Linux); other
/// tests of this binary run alongside, so it leaves generous room.
fn build_within_budget(label: &str, library: Vec<Snippet>) -> Snapshot {
    let len = library.len();
    let before = resident_mib();
    let start = Instant::now();
    let snapshot = Snapshot::build(library, EngineSettings::default());
    let elapsed = start.elapsed();
    let after = resident_mib();
    eprintln!("{label}: {len} snippets built in {elapsed:?}");
    assert!(snapshot.excluded().is_empty(), "{label}");
    assert_eq!(snapshot.matcher().len(), len, "{label}");
    assert!(elapsed < budget(200), "{label}: {elapsed:?}");
    if let (Some(before), Some(after)) = (before, after) {
        let grown = after.saturating_sub(before);
        assert!(grown < 256, "{label}: resident set grew by {grown} MiB");
    }
    snapshot
}

/// What the snapshot compiled for the snippet at `index` of `library`: the number of form
/// fields, or the error code.
fn form_size(
    snapshot: &Snapshot,
    library: &[Snippet],
    index: usize,
) -> Result<usize, &'static str> {
    snapshot
        .compiled(library[index].meta.id)
        .expect("compiled")
        .as_ref()
        .map(|r| r.form().len())
        .map_err(TemplateError::code)
}

/// Before the fix of finding 2, the plain shape took 4.8 s and 2.4 GiB at 4,001 snippets; at
/// 10,001 snippets the shapes of follow-up finding N1 (a field or an `{if}` in every user, or
/// two hubs) took 10.6 s and 5.5 GiB. Each now builds in linear time: a form has at most 100
/// fields (`E_FORM_TOO_LARGE`), and a form is made from the summaries of the included bodies,
/// never by walking them again.
#[test]
fn shared_hub_library_builds_in_linear_time() {
    for (shape, users, form) in [
        (Shape::Plain, 5000, Ok(0)),
        (Shape::LeafFields, 5000, Err("E_FORM_TOO_LARGE")),
        (Shape::OwnField, 5000, Err("E_FORM_TOO_LARGE")),
        (Shape::OwnIf, 5000, Err("E_FORM_TOO_LARGE")),
        (Shape::TwoHubs, 4999, Err("E_FORM_TOO_LARGE")),
    ] {
        let library = hub_library(shape, 5000, users);
        assert_eq!(library.len(), 10_001);
        let label = format!("{shape:?}");
        let snapshot = build_within_budget(&label, library.clone());
        assert_eq!(form_size(&snapshot, &library, 10_000), form, "{label}");
    }
}

/// The largest forms that resolve, copied into every template: a hub with exactly 100 fields
/// and 10,000 users that each have a field of their own (half of them a new name, so 101
/// fields and `E_FORM_TOO_LARGE`; half of them one of the hub's names, so 100 fields in a form
/// the user owns), and the same with 100 leaves that all ask for one name (a form of one field
/// that a walk would build by entering every leaf).
#[test]
fn largest_forms_build_in_linear_time() {
    let mut library = hub_library(Shape::LeafFields, 100, 0);
    for i in 0..10_000_u32 {
        let own = if i % 2 == 0 {
            "who".to_owned()
        } else {
            format!("f{}", i % 100)
        };
        library.push(snippet(
            101 + i,
            &format!("user{i}"),
            &format!("Dear {{field name={own}}}, {{snippet abbr=hub}}"),
        ));
    }
    let snapshot = build_within_budget("hub of 100 fields", library.clone());
    assert_eq!(form_size(&snapshot, &library, 100), Ok(100));
    assert_eq!(form_size(&snapshot, &library, 101), Err("E_FORM_TOO_LARGE"));
    assert_eq!(form_size(&snapshot, &library, 102), Ok(100));

    let mut library = Vec::new();
    let mut hub = String::new();
    for i in 0..5000_u32 {
        library.push(snippet(i, &format!("leaf{i}"), "{field name=same}"));
        write!(hub, "{{snippet abbr=leaf{i}}}").expect("write to a string");
    }
    library.push(snippet(5000, "hub", &hub));
    for i in 0..5000_u32 {
        library.push(snippet(
            5001 + i,
            &format!("user{i}"),
            "Dear {field name=who}, {snippet abbr=hub}",
        ));
    }
    let snapshot = build_within_budget("one name in 5,000 leaves", library.clone());
    assert_eq!(form_size(&snapshot, &library, 10_000), Ok(2));
}

/// What the snapshot compiled for one snippet, in a comparable form.
fn outcome(
    r: &Result<iamb_core::template::Resolved, TemplateError>,
) -> Result<Vec<FieldSpec>, TemplateError> {
    r.as_ref().map(|r| r.form().to_vec()).map_err(Clone::clone)
}

const N: usize = 20;

/// One piece of the generated body of snippet `i`: text, a `{snippet}` (mostly of a later
/// snippet, so the library forms deep chains that hit `E_TOO_DEEP` in different contexts; now
/// and then of any snippet, which can make a cycle, or of a missing one), a field or an `{if}`.
fn arb_piece(i: usize) -> impl Strategy<Value = String> {
    let later = (i + 1).min(N - 1)..(i + 3).min(N);
    prop_oneof![
        6 => later.prop_map(|j| format!("{{snippet abbr=s{j}}}")),
        1 => (0..N).prop_map(|j| format!("{{snippet abbr=s{j}}}")),
        1 => Just("{snippet abbr=missing}".to_owned()),
        1 => Just("x".to_owned()),
        1 => prop_oneof![Just("text"), Just("area")].prop_map(|t| format!("{{field name=F type={t}}}")),
        1 => Just("{if field=G is=a}y{endif}".to_owned()),
        1 => Just("{if field=G}z{endif}".to_owned()),
        1 => Just("{field name=G type=choice options=\"a|b\"}".to_owned()),
    ]
}

fn arb_bodies() -> impl Strategy<Value = Vec<String>> {
    (0..N)
        .map(|i| proptest::collection::vec(arb_piece(i), 0..4).prop_map(|p| p.concat()))
        .collect::<Vec<_>>()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Every template the snapshot compiled reports exactly what parsing and resolving it on
    /// its own against the same snapshot reports: the same form, or the same error with the
    /// same snippet and position, whatever was compiled before it.
    #[test]
    fn shared_compilation_matches_a_standalone_resolve(
        bodies in arb_bodies(),
        order in Just((0..N).collect::<Vec<_>>()).prop_shuffle(),
    ) {
        let library: Vec<Snippet> = order
            .iter()
            .map(|&i| snippet(u32::try_from(i).expect("small"), &format!("s{i}"), &bodies[i]))
            .collect();
        let snapshot = Snapshot::build(library.clone(), EngineSettings::default());
        for s in &library {
            let shared = snapshot.compiled(s.meta.id).expect("compiled");
            let alone = Template::parse(&s.body).and_then(|t| t.resolve(&snapshot));
            prop_assert_eq!(outcome(shared), outcome(&alone), "{}: {}", s.abbreviation.as_str(), s.body);
        }
    }
}

/// `y` is first linked from the shared cache below `x` (expanded cleanly while compiling `w`),
/// then reached again five levels down, where it no longer fits. A resolve on its own walks `x`
/// and has seen `y`, so it reports the depth error at the inclusion in `p5`; the snapshot must
/// say the same, not walk into `y` and report a deeper snippet.
#[test]
fn shared_body_reached_again_too_deep_reports_the_inclusion() {
    let library = vec![
        snippet(1, "w", "{snippet abbr=x}"),
        snippet(2, "x", "{snippet abbr=y}"),
        snippet(3, "y", "{snippet abbr=c1}"),
        snippet(4, "c1", "{snippet abbr=c2}"),
        snippet(5, "c2", "{snippet abbr=c3}"),
        snippet(6, "c3", "leaf"),
        snippet(7, "p1", "{snippet abbr=p2}"),
        snippet(8, "p2", "{snippet abbr=p3}"),
        snippet(9, "p3", "{snippet abbr=p4}"),
        snippet(10, "p4", "{snippet abbr=p5}"),
        snippet(11, "p5", "{snippet abbr=y}"),
        snippet(12, "r", "{snippet abbr=x}{snippet abbr=p1}"),
    ];
    let snapshot = Snapshot::build(library.clone(), EngineSettings::default());
    let r = &library[11];
    let alone = Template::parse(&r.body)
        .and_then(|t| t.resolve(&snapshot))
        .expect_err("too deep");
    assert_eq!(alone.snippet(), Some("p5"));
    let shared = snapshot.compiled(r.meta.id).expect("compiled");
    assert_eq!(outcome(shared), Err(alone));
}
