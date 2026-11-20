//! The palette's search through the service (task 3.6): 10,000 snippets imported into a worker on
//! the mock platform, then 400 `Search` requests over IPC, each timed from the request to its
//! answer. This is the service's whole share of "results in under 30 ms at 10,000 snippets"
//! (`docs/test-strategy.md` §2.2, §8.7): the snapshot walk, the ranking, the frames. `iamb-bench`'s
//! `search/query/10000` times the ranking alone and is the gated measurement.
//!
//! The bound applies to an optimised build (`cargo test --release`, how the numbers in §8.7 were
//! taken); a debug build is several times slower and is only held to a loose bound that catches a
//! hang or a quadratic mistake.

mod common;

use std::fmt::Write as _;
use std::time::Instant;

use common::Harness;
use iamb_ipc::{ConflictPolicy, RequestBody, ResponseBody, Role, Secret, TransferFormat};

const WORDS: &[&str] = &[
    "thanks", "order", "invoice", "meeting", "tomorrow", "schedule", "address", "support",
    "ticket", "project", "review", "draft", "attached", "regards",
];

fn library_csv(count: usize) -> String {
    let mut csv = String::from("abbreviation,label,body\n");
    for i in 0..count {
        let _ = writeln!(
            csv,
            ";x{:05},{} {} note,{} {} {} for {} number {i}",
            i + 1,
            WORDS[i % WORDS.len()],
            WORDS[(i / 7) % WORDS.len()],
            WORDS[(i / 3) % WORDS.len()],
            WORDS[(i / 11) % WORDS.len()],
            WORDS[(i / 5) % WORDS.len()],
            WORDS[(i / 13) % WORDS.len()],
        );
    }
    csv
}

#[test]
fn a_search_over_10000_snippets_answers_in_time() {
    let h = Harness::start(&[]);
    let mut ui = h.client(Role::Ui);
    let dir = tempfile::TempDir::new().expect("dir");
    let path = dir.path().join("library.csv");
    std::fs::write(&path, library_csv(10_000)).expect("csv");
    ui.call(RequestBody::Import {
        path,
        format: TransferFormat::Csv,
        on_conflict: ConflictPolicy::Skip,
    })
    .expect("imported");

    let queries: Vec<String> = (0..400)
        .map(|i| match i % 5 {
            // An abbreviation, as typed (prefix and whole).
            0 => format!(";x{:05}", 1 + (i * 17) % 10_000),
            // A word of the label and the body.
            1 => WORDS[i % WORDS.len()].to_owned(),
            // Two words.
            2 => format!(
                "{} {}",
                WORDS[i % WORDS.len()],
                WORDS[(i / 3) % WORDS.len()]
            ),
            // A typo.
            3 => WORDS[i % WORDS.len()]
                .replace("ea", "ae")
                .replace("or", "ro"),
            // Nothing matches: the slowest, every tier and the typo tier run for every snippet.
            _ => format!("zqxjv{i}"),
        })
        .collect();
    let mut ask = |text: &str| {
        let started = Instant::now();
        let answer = ui
            .call(RequestBody::Search {
                text: Secret::new(text.to_owned()),
                limit: 50,
            })
            .expect("search");
        let took = started.elapsed().as_secs_f64() * 1000.0;
        let ResponseBody::SearchResults { hits } = answer else {
            panic!("unexpected {answer:?}");
        };
        assert!(hits.len() <= 50);
        (took, hits.len())
    };
    // Warm up, and check that a word finds a full page of hits.
    assert_eq!(ask("thanks").1, 50);
    let mut times: Vec<f64> = queries.iter().map(|q| ask(q).0).collect();
    times.sort_by(f64::total_cmp);
    let p95 = times[times.len() * 95 / 100];
    eprintln!(
        "search over IPC at 10,000 snippets, 400 queries: p50 {:.1} ms, p95 {p95:.1} ms, max {:.1} ms{}",
        times[times.len() / 2],
        times[times.len() - 1],
        if cfg!(debug_assertions) {
            " (debug build)"
        } else {
            ""
        }
    );
    // The KPI is 30 ms at the 95th percentile; CI holds an optimised build to twice that.
    let bound = if cfg!(debug_assertions) { 600.0 } else { 60.0 };
    assert!(p95 < bound, "p95 {p95:.1} ms is not under {bound} ms");
    h.stop();
}
