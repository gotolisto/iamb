//! Matcher benchmarks (task 1.2): `matcher/build/10000` and `matcher/lookup/10000`.
//!
//! The 10,000-snippet fixture (fixed seed) is turned into matcher entries with a mix of trigger
//! and case modes. Criterion measures build and lookup; then a recorder pass times each lookup
//! individually for the percentile KPI (`docs/test-strategy.md` §2.2: p99 < 50 µs, 90% hits and
//! 10% misses) and writes the JSON-lines result file.
//!
//! A miss is a lookup after one more keystroke into a realistic 20-character mid-word buffer
//! (text from the fixture bodies). A hit is a lookup right after the keystroke that completes an
//! abbreviation (or its delimiter), typed after a space so word-start rules pass.
//!
//! Run: `cargo bench -p iamb-bench --bench matcher -- --quick`

use criterion::{BatchSize, Criterion, criterion_group};
use iamb_bench::{
    DEFAULT_SEED, FixtureSnippet, KPI_LIBRARY_SIZE, LatencyRecorder, ResultRecord, RunWriter,
    SplitMix64, generate_library,
};
use iamb_core::matcher::{Delimiters, KeyBuffer, MatchEntry, Matcher};
use iamb_core::model::{Abbreviation, CaseMode, Id, Trigger};
use std::hint::black_box;
use std::time::Instant;

/// Individually timed lookups per percentile record.
const RECORDED_LOOKUPS: usize = 10_000;
/// Timed builds for the build record.
const RECORDED_BUILDS: usize = 20;
/// Characters in a mid-word buffer state before the measured keystroke.
const MID_WORD_CHARS: usize = 20;
/// KPI target: p99 lookup under 50 µs.
const LOOKUP_TARGET_NS: u64 = 50_000;

/// Turns the fixture into matcher entries. Deterministic: one third immediate (word start off),
/// two thirds delimiter (word start on), as the editor defaults; case modes cycle independently.
fn entries(lib: &[FixtureSnippet]) -> Vec<MatchEntry> {
    lib.iter()
        .enumerate()
        .map(|(i, s)| {
            let trigger = if i % 3 == 0 {
                Trigger::Immediate
            } else {
                Trigger::Delimiter
            };
            let case_mode = match (i / 3) % 3 {
                0 => CaseMode::Insensitive,
                1 => CaseMode::Adaptive,
                _ => CaseMode::Exact,
            };
            let mut id = [0_u8; 16];
            id[8..].copy_from_slice(&u64::try_from(i).unwrap_or(u64::MAX).to_be_bytes());
            MatchEntry {
                snippet: Id::from_bytes(id),
                abbreviation: Abbreviation::new(&s.abbreviation)
                    .expect("fixture abbreviations are valid"),
                trigger,
                case_mode,
                word_start: trigger == Trigger::Delimiter,
            }
        })
        .collect()
}

fn build(entries: Vec<MatchEntry>) -> Matcher {
    Matcher::build(entries, Delimiters::default())
}

/// A prepared buffer state and the keystroke whose lookup is measured.
struct Probe {
    buffer: KeyBuffer,
    expected: Option<Id>,
}

/// `count` mid-word states: 20 characters of body text ending inside a word, then the next
/// letter of that word is pushed. None of them may match.
fn miss_probes(lib: &[FixtureSnippet], matcher: &Matcher, count: usize, seed: u64) -> Vec<Probe> {
    let mut rng = SplitMix64::new(seed);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let body: Vec<char> = lib[rng.index(lib.len())].body.chars().collect();
        if body.len() < MID_WORD_CHARS + 1 {
            continue;
        }
        let end = rng.range_inclusive(MID_WORD_CHARS + 1, body.len());
        let window = &body[end - MID_WORD_CHARS - 1..end];
        // Mid-word: the last two characters are letters.
        if !window[MID_WORD_CHARS - 1..]
            .iter()
            .all(char::is_ascii_alphabetic)
        {
            continue;
        }
        let mut buffer = KeyBuffer::new();
        for &c in window {
            buffer.push(c);
        }
        assert!(matcher.lookup(&buffer).is_none(), "mid-word probe matched");
        out.push(Probe {
            buffer,
            expected: None,
        });
    }
    out
}

/// `count` states that end with a complete abbreviation: a few words, a space, the
/// abbreviation (in a case its mode accepts) and, for delimiter snippets, a delimiter.
fn hit_probes(entries: &[MatchEntry], matcher: &Matcher, count: usize, seed: u64) -> Vec<Probe> {
    const LEADS: &[&str] = &["thanks for the ", "see ", "ok ", "please find "];
    const DELIMS: &[char] = &[' ', '.', ',', ';'];
    let mut rng = SplitMix64::new(seed);
    (0..count)
        .map(|_| {
            let e = &entries[rng.index(entries.len())];
            let mut buffer = KeyBuffer::new();
            for c in LEADS[rng.index(LEADS.len())].chars() {
                buffer.push(c);
            }
            let abbr = e.abbreviation.as_str();
            // Exact snippets as written; the others in upper case half of the time.
            let typed = if e.case_mode != CaseMode::Exact && rng.below(2) == 0 {
                abbr.to_uppercase()
            } else {
                abbr.to_owned()
            };
            for c in typed.chars() {
                buffer.push(c);
            }
            if e.trigger == Trigger::Delimiter {
                buffer.push(DELIMS[rng.index(DELIMS.len())]);
            }
            let got = matcher.lookup(&buffer).map(|m| m.snippet);
            assert_eq!(got, Some(e.snippet), "hit probe did not match its snippet");
            Probe {
                buffer,
                expected: Some(e.snippet),
            }
        })
        .collect()
}

/// The KPI mix: 90% hits, 10% misses, in a seeded order.
fn mixed_probes(
    lib: &[FixtureSnippet],
    entries: &[MatchEntry],
    matcher: &Matcher,
    count: usize,
    seed: u64,
) -> Vec<Probe> {
    let misses = count / 10;
    let mut hits = hit_probes(entries, matcher, count - misses, seed).into_iter();
    let mut miss = miss_probes(lib, matcher, misses, seed ^ 0xA5A5).into_iter();
    let mut rng = SplitMix64::new(seed ^ 0x0F0F);
    let mut out = Vec::with_capacity(count);
    let (mut h_left, mut m_left) = (count - misses, misses);
    while h_left + m_left > 0 {
        // Draw without replacement so the mix is exact.
        let take_miss = rng.index(h_left + m_left) < m_left;
        let next = if take_miss {
            m_left -= 1;
            miss.next()
        } else {
            h_left -= 1;
            hits.next()
        };
        out.extend(next);
    }
    out
}

fn bench_matcher(c: &mut Criterion) {
    let lib = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE);
    let entries = entries(&lib);

    c.bench_function("matcher/build/10000", |b| {
        b.iter_batched(|| entries.clone(), build, BatchSize::LargeInput);
    });

    let matcher = build(entries.clone());
    for (name, probes) in [
        (
            "matcher/lookup_miss/10000",
            miss_probes(&lib, &matcher, 1024, DEFAULT_SEED),
        ),
        (
            "matcher/lookup_hit/10000",
            hit_probes(&entries, &matcher, 1024, DEFAULT_SEED),
        ),
    ] {
        let mut i = 0_usize;
        c.bench_function(name, |b| {
            b.iter(|| {
                i = (i + 1) % probes.len();
                matcher.lookup(black_box(&probes[i].buffer))
            });
        });
    }
}

criterion_group!(benches, bench_matcher);

/// Times each lookup individually; checks every result.
fn record_lookups(matcher: &Matcher, probes: &[Probe]) -> LatencyRecorder {
    // Warm up caches and the branch predictor before recording.
    for p in probes {
        black_box(matcher.lookup(&p.buffer));
    }
    let mut rec = LatencyRecorder::with_capacity(probes.len());
    for p in probes {
        let got = rec.time(|| matcher.lookup(black_box(&p.buffer)));
        assert_eq!(got.map(|m| m.snippet), p.expected, "lookup result changed");
    }
    rec
}

/// Runs the recorder passes and writes the results file.
fn record_percentiles() -> std::io::Result<()> {
    let lib = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE);
    let entries = entries(&lib);

    let mut build_rec = LatencyRecorder::with_capacity(RECORDED_BUILDS);
    let mut matcher = None;
    for _ in 0..RECORDED_BUILDS {
        let input = entries.clone();
        let start = Instant::now();
        let m = build(black_box(input));
        build_rec.record(start.elapsed());
        matcher = Some(m);
    }
    let matcher = matcher.expect("at least one build");
    assert!(
        matcher.duplicates().is_empty(),
        "fixture has unique abbreviations"
    );

    let seed = DEFAULT_SEED ^ 1;
    let mixed = record_lookups(
        &matcher,
        &mixed_probes(&lib, &entries, &matcher, RECORDED_LOOKUPS, seed),
    );
    let misses = record_lookups(
        &matcher,
        &miss_probes(&lib, &matcher, RECORDED_LOOKUPS, seed),
    );
    let hits = record_lookups(
        &matcher,
        &hit_probes(&entries, &matcher, RECORDED_LOOKUPS, seed),
    );

    let overhead = "samples include Instant overhead (~20-40 ns)";
    let mut writer = RunWriter::in_default_dir("matcher");
    let mut lines = Vec::new();
    for (bench, rec, kpi, target, note) in [
        (
            "matcher/build/10000",
            &build_rec,
            None,
            None,
            format!("{RECORDED_BUILDS} builds from 10,000 entries; input clone not timed"),
        ),
        (
            "matcher/lookup/10000",
            &mixed,
            Some("matcher-lookup-p99"),
            Some(LOOKUP_TARGET_NS),
            format!("90% hits, 10% mid-word misses; {overhead}"),
        ),
        (
            "matcher/lookup_miss/10000",
            &misses,
            None,
            Some(LOOKUP_TARGET_NS),
            format!("one keystroke into a 20-char mid-word buffer, no match; {overhead}"),
        ),
        (
            "matcher/lookup_hit/10000",
            &hits,
            None,
            Some(LOOKUP_TARGET_NS),
            format!("keystroke completing an abbreviation or its delimiter; {overhead}"),
        ),
    ] {
        let Some(summary) = rec.summary() else {
            continue;
        };
        let mut record = ResultRecord::new(bench, summary);
        record.kpi = kpi.map(str::to_owned);
        if let Some(t) = target {
            record.target_stat = Some("p99");
            record.target_ns = Some(t);
        }
        record.note = Some(note);
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
        let verdict = match (r.target_value(), r.target_ns) {
            (Some(v), Some(t)) if v < t => " [pass: p99 < target]",
            (Some(_), Some(_)) => " [FAIL: p99 >= target]",
            _ => "",
        };
        println!(
            "{} (individually timed, n={}): min {} ns, p50 {} ns, p95 {} ns, p99 {} ns, \
             max {} ns{verdict}",
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
