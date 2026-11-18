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

/// n leaves, one hub that includes all of them, n snippets that each include the hub: the
/// review's shape. With `fields`, every leaf asks for its own field, so the hub's form has n
/// fields and every user's form is the hub's.
fn hub_library(n: u32, fields: bool) -> Vec<Snippet> {
    let mut v = Vec::new();
    let mut hub = String::new();
    for i in 0..n {
        let leaf = if fields {
            format!("{{field name=f{i}}}")
        } else {
            "some leaf text".to_owned()
        };
        v.push(snippet(i, &format!("leaf{i}"), &leaf));
        write!(hub, "{{snippet abbr=leaf{i}}}").expect("write to a string");
    }
    v.push(snippet(n, "hub", &hub));
    for i in 0..n {
        v.push(snippet(
            n + 1 + i,
            &format!("user{i}"),
            "Dear all, {snippet abbr=hub}",
        ));
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

/// Before the fix this shape took 4.8 s and 2.4 GiB at 4,001 snippets (review 1.9 finding 2);
/// now it is linear in both. The memory bound is checked where the OS reports the resident set
/// (Linux); other tests of this binary run alongside, so it leaves generous room.
#[test]
fn shared_hub_library_builds_in_linear_time() {
    // Without fields, and with a field in every leaf (the forms are shared too: 2,000 users
    // each owning a copy of a 2,000-field form took 1.6 s and 700 MiB).
    for fields in [false, true] {
        let library = hub_library(2000, fields);
        assert_eq!(library.len(), 4001);
        let user = library[4000].meta.id;
        let before = resident_mib();
        let start = Instant::now();
        let snapshot = Snapshot::build(library, EngineSettings::default());
        let elapsed = start.elapsed();
        let after = resident_mib();
        assert!(snapshot.excluded().is_empty());
        assert_eq!(snapshot.matcher().len(), 4001);
        let form = snapshot
            .compiled(user)
            .expect("compiled")
            .as_ref()
            .expect("resolves");
        assert_eq!(form.form().len(), if fields { 2000 } else { 0 });
        assert!(elapsed < budget(200), "fields {fields}: {elapsed:?}");
        if let (Some(before), Some(after)) = (before, after) {
            let grown = after.saturating_sub(before);
            assert!(
                grown < 256,
                "fields {fields}: resident set grew by {grown} MiB"
            );
        }
    }
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
