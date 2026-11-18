//! Engine benchmark (task 1.6, review 1.9 row 8): `engine/expand/10000`.
//!
//! The time from the last `Char` of an abbreviation (or its delimiter) to the returned actions,
//! on a snapshot of 10,000 snippets: matcher lookup, the compiled template's render and the
//! action list. The characters before the last one are fed untimed. Afterwards the injection is
//! cancelled (untimed) so the next sample starts idle. 10,000 individually timed expansions;
//! `LatencyRecorder` p50/p95/p99 and the JSON-lines writer.
//!
//! There is no KPI target for this stage alone (the end-to-end figure is a lab measurement,
//! task 2.7); the record carries the p99 for tracking. Result: `engine/expand/10000`.
//!
//! Run: `cargo bench -p iamb-bench --bench engine -- --quick`

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group};
use iamb_bench::{
    DEFAULT_SEED, KPI_LIBRARY_SIZE, LatencyRecorder, ResultRecord, RunWriter, SplitMix64,
    generate_library,
};
use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::{
    Action, AppId, Engine, EngineSettings, Environment, Event, SecureState, Snapshot,
};
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp};

/// Individually timed expansions for the percentile record.
const RECORDED_EXPANSIONS: usize = 10_000;

struct Clock;

impl Environment for Clock {
    fn now(&self) -> LocalDateTime {
        LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("valid date")
    }
    fn clipboard_text(&self) -> Option<String> {
        None
    }
}

/// A snapshot of the fixture library (delimiter snippets, the editor defaults) and the
/// abbreviations in it.
fn snapshot() -> (Snapshot, Vec<(Id, String)>) {
    let library = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE);
    let mut abbreviations = Vec::with_capacity(library.len());
    let snippets: Vec<Snippet> = library
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut bytes = [0_u8; 16];
            bytes[8..].copy_from_slice(&u64::try_from(i).unwrap_or(u64::MAX).to_be_bytes());
            let id = Id::from_bytes(bytes);
            abbreviations.push((id, s.abbreviation.clone()));
            Snippet::new_defaults(
                SyncMeta::new(id, Timestamp(0)),
                Id::from_bytes([0; 16]),
                Abbreviation::new(&s.abbreviation).expect("fixture abbreviation"),
                "",
                s.body.clone(),
                None,
            )
        })
        .collect();
    let snapshot = Snapshot::build(snippets, EngineSettings::default());
    assert_eq!(snapshot.len(), KPI_LIBRARY_SIZE);
    assert!(snapshot.excluded().is_empty());
    (snapshot, abbreviations)
}

/// An engine past its first verdict.
fn engine(snapshot: &Snapshot) -> Engine {
    let mut engine = Engine::new(snapshot.clone());
    engine.handle(Event::FocusChanged(AppId("bench".to_owned())), &Clock);
    engine.handle(Event::SecureState(SecureState::Normal), &Clock);
    engine
}

/// The text typed for one sample: a lead-in, the abbreviation and its delimiter.
fn typed(rng: &mut SplitMix64, abbreviation: &str) -> Vec<char> {
    const LEADS: &[&str] = &["thanks for the ", "see ", "ok ", "please find "];
    let mut chars: Vec<char> = LEADS[rng.index(LEADS.len())].chars().collect();
    chars.extend(abbreviation.chars());
    chars.push(' ');
    chars
}

/// Feeds all but the last character, then times the last one. Returns the time and the actions
/// of the expansion, after cancelling the injection.
fn expand_once(engine: &mut Engine, chars: &[char], seq: &mut u64) -> (Duration, Vec<Action>) {
    let (last, head) = chars.split_last().expect("some text");
    for &c in head {
        *seq += 1;
        let out = engine.handle(Event::char(c, *seq), &Clock);
        assert!(out.is_empty(), "expanded before the last character");
    }
    *seq += 1;
    let event = Event::char(*last, *seq);
    let started = Instant::now();
    let actions = black_box(engine.handle(black_box(event), &Clock));
    let took = started.elapsed();
    if let Some(Action::Expanded { expansion, .. }) = actions.last() {
        engine.handle(
            Event::InjectionCancelled {
                expansion: *expansion,
            },
            &Clock,
        );
    }
    (took, actions)
}

fn bench_engine(c: &mut Criterion) {
    let (snapshot, abbreviations) = snapshot();
    let mut engine = engine(&snapshot);
    let mut rng = SplitMix64::new(DEFAULT_SEED);
    let mut seq = 0_u64;
    c.bench_function("engine/expand/10000", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let (_, abbreviation) = &abbreviations[rng.index(abbreviations.len())];
                let chars = typed(&mut rng, abbreviation);
                let (took, _) = expand_once(&mut engine, &chars, &mut seq);
                total += took;
            }
            total
        });
    });
}

criterion_group!(benches, bench_engine);

/// Times each expansion individually and checks that every one expanded the typed snippet.
fn record_percentiles() -> std::io::Result<()> {
    let (snapshot, abbreviations) = snapshot();
    let mut engine = engine(&snapshot);
    let mut rng = SplitMix64::new(DEFAULT_SEED ^ 2);
    let mut seq = 0_u64;
    // Warm up caches and the branch predictor before recording.
    for _ in 0..500 {
        let (_, abbreviation) = &abbreviations[rng.index(abbreviations.len())];
        let chars = typed(&mut rng, abbreviation);
        expand_once(&mut engine, &chars, &mut seq);
    }
    let mut rec = LatencyRecorder::with_capacity(RECORDED_EXPANSIONS);
    for _ in 0..RECORDED_EXPANSIONS {
        let (id, abbreviation) = &abbreviations[rng.index(abbreviations.len())];
        let chars = typed(&mut rng, abbreviation);
        let (took, actions) = expand_once(&mut engine, &chars, &mut seq);
        rec.record(took);
        assert!(
            matches!(actions.last(), Some(Action::Expanded { snippet, .. }) if snippet == id),
            "typing {abbreviation:?} did not expand its snippet"
        );
    }
    let mut writer = RunWriter::in_default_dir("engine");
    let bench = "engine/expand/10000";
    let mut lines = Vec::new();
    if let Some(summary) = rec.summary() {
        let mut record = ResultRecord::new(bench, summary);
        record.note = Some(format!(
            "last Char of an abbreviation + delimiter to the returned actions, {KPI_LIBRARY_SIZE} snippets, \
             delimiter mode, plain-text bodies; samples include Instant overhead"
        ));
        writer.push(&record);
        let samples = writer.write_samples(bench, &rec.sorted_samples())?;
        lines.push((record, samples));
    }
    let path = writer.finish()?;
    report(&lines, &path);
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "a benchmark reports its numbers on stdout"
)]
fn report(lines: &[(ResultRecord, std::path::PathBuf)], path: &std::path::Path) {
    for (r, samples) in lines {
        let s = &r.summary;
        println!(
            "{} (individually timed, n={}): min {} ns, p50 {} ns, p95 {} ns, p99 {} ns, \
             max {} ns",
            r.bench, s.count, s.min, s.p50, s.p95, s.p99, s.max
        );
        println!("  samples: {}", samples.display());
    }
    println!("results: {}", path.display());
}

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
    if let Err(e) = record_percentiles() {
        eprintln!("failed to write bench results: {e}");
        std::process::exit(1);
    }
}
