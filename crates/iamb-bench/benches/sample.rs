//! PLACEHOLDER baseline benchmark.
//!
//! Measures building a `HashMap` from the 10,000-snippet fixture and looking abbreviations up
//! in it. It exists to prove the harness end to end (fixture, criterion, latency recorder,
//! JSON-lines result file) before the real matcher exists. Task 1.2 replaces it with
//! `benches/matcher.rs` (`matcher/lookup/10000`, target p99 < 50 µs).
//!
//! Run: `cargo bench -p iamb-bench --bench sample -- --quick`

use criterion::{BatchSize, Criterion, criterion_group};
use iamb_bench::fixture::lookup_keys;
use iamb_bench::{
    DEFAULT_SEED, FixtureSnippet, KPI_LIBRARY_SIZE, LatencyRecorder, ResultRecord, RunWriter,
    generate_library,
};
use std::collections::HashMap;
use std::hint::black_box;

/// Number of individually timed lookups for the percentile record.
const RECORDED_LOOKUPS: usize = 10_000;

fn build_map(lib: &[FixtureSnippet]) -> HashMap<&str, &str> {
    lib.iter()
        .map(|s| (s.abbreviation.as_str(), s.body.as_str()))
        .collect()
}

fn bench_placeholder(c: &mut Criterion) {
    let lib = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE);
    let keys = lookup_keys(&lib, DEFAULT_SEED, 1024);

    c.bench_function("placeholder/hashmap_build/10000", |b| {
        b.iter(|| build_map(black_box(&lib)));
    });

    let map = build_map(&lib);
    let mut i = 0_usize;
    c.bench_function("placeholder/hashmap_lookup/10000", |b| {
        b.iter_batched(
            || {
                i = (i + 1) % keys.len();
                keys[i].as_str()
            },
            |k| map.get(black_box(k)).map(|body| body.len()),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_placeholder);

/// Times each lookup individually and writes the percentile record to the results file.
fn record_percentiles() -> std::io::Result<()> {
    let lib = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE);
    let map = build_map(&lib);
    let keys = lookup_keys(&lib, DEFAULT_SEED ^ 1, RECORDED_LOOKUPS);

    // Warm up caches and the branch predictor before recording.
    for k in &keys {
        black_box(map.get(k.as_str()));
    }
    let mut rec = LatencyRecorder::with_capacity(keys.len());
    for k in &keys {
        rec.time(|| black_box(map.get(black_box(k.as_str()))));
    }

    let Some(summary) = rec.summary() else {
        return Ok(());
    };
    let bench = "placeholder/hashmap_lookup/10000";
    let mut record = ResultRecord::new(bench, summary);
    // Same pass rule the real matcher bench will use (p99 < 50 µs), so the pipeline is
    // exercised; no KPI id because this is not the matcher.
    record.target_stat = Some("p99");
    record.target_ns = Some(50_000);
    record.note = Some("placeholder until task 1.2; samples include Instant overhead".to_owned());

    let mut writer = RunWriter::in_default_dir("sample");
    writer.push(&record);
    let samples = writer.write_samples(bench, &rec.sorted_samples())?;
    let path = writer.finish()?;

    report(bench, &record, &path, &samples);
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "a benchmark reports its numbers on stdout"
)]
fn report(bench: &str, r: &ResultRecord, path: &std::path::Path, samples: &std::path::Path) {
    let s = &r.summary;
    println!(
        "{bench} (individually timed, n={}): min {} ns, p50 {} ns, p95 {} ns, p99 {} ns, \
         max {} ns",
        s.count, s.min, s.p50, s.p95, s.p99, s.max
    );
    println!("results: {}", path.display());
    println!("samples: {}", samples.display());
}

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
    if let Err(e) = record_percentiles() {
        eprintln!("failed to write bench results: {e}");
        std::process::exit(1);
    }
}
