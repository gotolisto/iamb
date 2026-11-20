//! Palette search benchmark (task 3.6): `search/query/10000`.
//!
//! The 10,000-snippet fixture (fixed seed) gets a label (its first three words, capitalised)
//! and up to two of forty tags. Criterion measures one query; then a recorder pass times 500
//! queries from a fixed mix, each over the whole library: abbreviation prefixes, whole words
//! of the bodies, two- and three-letter fragments, words with two letters swapped (typos), two
//! words, and queries that match nothing (the slowest case: every tier and the typo tier run
//! for every snippet). The KPI is p95 under 30 ms (`docs/test-strategy.md` §2.2).
//!
//! Run: `cargo bench -p iamb-bench --bench search -- --quick`

use criterion::{Criterion, criterion_group};
use iamb_bench::{
    DEFAULT_SEED, FixtureSnippet, KPI_LIBRARY_SIZE, LatencyRecorder, ResultRecord, RunWriter,
    SplitMix64, generate_library,
};
use iamb_core::model::Id;
use iamb_core::search::{Doc, search};
use std::hint::black_box;

/// Queries in the recorded pass.
const RECORDED_QUERIES: usize = 500;
/// KPI target: p95 under 30 ms.
const QUERY_TARGET_NS: u64 = 30_000_000;
/// The palette asks for at most this many hits.
const LIMIT: usize = 50;

struct Library {
    snippets: Vec<(Id, FixtureSnippet, String, Vec<Id>)>,
    tags: Vec<(Id, String)>,
}

fn id(n: u64, tag: bool) -> Id {
    let mut bytes = [0_u8; 16];
    bytes[8..].copy_from_slice(&n.to_be_bytes());
    bytes[0] = u8::from(tag);
    Id::from_bytes(bytes)
}

fn library() -> Library {
    let mut rng = SplitMix64::new(DEFAULT_SEED ^ 0x005E_A2C4);
    let tags: Vec<(Id, String)> = (0..40_u64)
        .map(|i| {
            (
                id(i, true),
                format!("tag{}", ["red", "blue", "work", "home"][(i % 4) as usize])
                    + &i.to_string(),
            )
        })
        .collect();
    let snippets = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE)
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let label = s
                .body
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            let n_tags = rng.index(3);
            let tag_ids = (0..n_tags).map(|_| tags[rng.index(tags.len())].0).collect();
            (id(i as u64, false), s, label, tag_ids)
        })
        .collect();
    Library { snippets, tags }
}

/// The fixed mix of queries.
fn queries(lib: &Library, count: usize) -> Vec<String> {
    let mut rng = SplitMix64::new(DEFAULT_SEED ^ 0x0BEE);
    let words = |rng: &mut SplitMix64| -> String {
        let body = &lib.snippets[rng.index(lib.snippets.len())].1.body;
        let all: Vec<&str> = body
            .split(|c: char| !c.is_ascii_alphabetic())
            .filter(|w| w.len() >= 4)
            .collect();
        all[rng.index(all.len())].to_owned()
    };
    (0..count)
        .map(|i| match i % 10 {
            0..=2 => {
                let a = &lib.snippets[rng.index(lib.snippets.len())].1.abbreviation;
                a[..rng.range_inclusive(2, a.len().min(5))].to_owned()
            }
            3..=5 => words(&mut rng),
            6 => {
                let w = words(&mut rng);
                w[..rng.range_inclusive(2, 3)].to_owned()
            }
            7 => {
                let mut w: Vec<char> = words(&mut rng).chars().collect();
                let at = rng.index(w.len() - 1);
                w.swap(at, at + 1);
                w.into_iter().collect()
            }
            8 => format!("{} {}", words(&mut rng), words(&mut rng)),
            _ => format!("zqxjv{}", rng.below(1000)),
        })
        .collect()
}

fn run(lib: &Library, query: &str) -> usize {
    let names = |t: Id| {
        lib.tags
            .iter()
            .find(|(i, _)| *i == t)
            .map(|(_, n)| n.as_str())
    };
    search(
        query,
        LIMIT,
        lib.snippets.iter().map(|(id, s, label, tags)| Doc {
            id: *id,
            abbreviation: &s.abbreviation,
            label,
            body: &s.body,
            tags,
        }),
        &names,
    )
    .len()
}

fn bench_search(c: &mut Criterion) {
    let lib = library();
    let queries = queries(&lib, 64);
    let mut i = 0_usize;
    c.bench_function("search/query/10000", |b| {
        b.iter(|| {
            i = (i + 1) % queries.len();
            run(&lib, black_box(&queries[i]))
        });
    });
}

criterion_group!(benches, bench_search);

/// `IAMB_SEARCH_BUDGET_MS`: fail when p95 is not under this (CI sets twice the target).
fn budget_ns() -> Option<u64> {
    let ms: u64 = std::env::var("IAMB_SEARCH_BUDGET_MS").ok()?.parse().ok()?;
    Some(ms * 1_000_000)
}

fn record() -> std::io::Result<()> {
    let lib = library();
    let queries = queries(&lib, RECORDED_QUERIES);
    for q in &queries {
        black_box(run(&lib, q));
    }
    let mut rec = LatencyRecorder::with_capacity(queries.len());
    for q in &queries {
        rec.time(|| run(&lib, black_box(q)));
    }
    let mut writer = RunWriter::in_default_dir("search");
    let mut over_budget = false;
    let bench = "search/query/10000";
    if let Some(summary) = rec.summary() {
        let mut record = ResultRecord::new(bench, summary);
        record.kpi = Some("palette-results-p95".to_owned());
        record.target_stat = Some("p95");
        record.target_ns = Some(QUERY_TARGET_NS);
        record.note = Some(
            "500 queries over 10,000 snippets, limit 50: abbreviation prefixes, words, \
             fragments, typos, two words and misses"
                .to_owned(),
        );
        writer.push(&record);
        let samples = writer.write_samples(bench, &rec.sorted_samples())?;
        report(&record, &samples);
        over_budget = budget_ns().is_some_and(|b| summary.p95 >= b);
    }
    let path = writer.finish()?;
    eprintln!("results: {}", path.display());
    if over_budget {
        eprintln!("search/query/10000: p95 is over IAMB_SEARCH_BUDGET_MS");
        std::process::exit(1);
    }
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "a benchmark reports its numbers on stdout"
)]
fn report(r: &ResultRecord, samples: &std::path::Path) {
    let s = &r.summary;
    let verdict = match (r.target_value(), r.target_ns) {
        (Some(v), Some(t)) if v < t => " [pass: p95 < target]",
        (Some(_), Some(_)) => " [FAIL: p95 >= target]",
        _ => "",
    };
    println!(
        "{} (individually timed, n={}): min {} us, p50 {} us, p95 {} us, p99 {} us, max {} us{verdict}",
        r.bench,
        s.count,
        s.min / 1000,
        s.p50 / 1000,
        s.p95 / 1000,
        s.p99 / 1000,
        s.max / 1000
    );
    println!("  samples: {}", samples.display());
}

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
    if let Err(e) = record() {
        eprintln!("failed to write bench results: {e}");
        std::process::exit(1);
    }
}
