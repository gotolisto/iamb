//! Store benchmark (task 1.5, review 1.9 row 8): `store/load/10000`.
//!
//! The KPI (`docs/test-strategy.md` §2.2: "10,000 snippets load from `SQLite` < 100 ms") is the
//! time to open the store (warm OS cache), read the whole library and build the engine snapshot
//! from the loaded snippets in a stable order: the same definition as the test
//! `open_load_and_build_10000_snippets` in `crates/iamb-store/tests/load.rs`. Criterion measures
//! it; then 30 individually timed runs give the p95 the KPI is judged on.
//!
//! There is no `store/search/10000`: `iamb-store` has no search function. Palette search
//! (task 3.6) will add it together with its bench.
//!
//! Run: `cargo bench -p iamb-bench --bench store -- --quick`

use std::path::{Path, PathBuf};
use std::time::Instant;

use criterion::{Criterion, criterion_group};
use iamb_bench::{
    DEFAULT_SEED, KPI_LIBRARY_SIZE, LatencyRecorder, ResultRecord, RunWriter, generate_library,
};
use iamb_core::engine::{EngineSettings, Snapshot};
use iamb_core::model::{
    Abbreviation, Folder, FolderDefaults, Id, Profile, ProfileKind, Snippet, SyncMeta, Timestamp,
};
use iamb_store::Store;
use std::hint::black_box;

/// Timed runs for the percentile record (the strategy's 30 runs, p95).
const RECORDED_RUNS: usize = 30;
/// Folders the snippets are spread over.
const FOLDERS: usize = 50;
/// KPI target: p95 under 100 ms (lab); CI allows 200 ms.
const LOAD_TARGET_NS: u64 = 100_000_000;
const NOW: Timestamp = Timestamp(1_763_000_000_000);

fn id(n: u64) -> Id {
    Id::v7(n, [0; 10])
}

/// A data directory under the target directory holding the fixture library, removed on drop.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("store-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::open(&dir).expect("open the fixture store");
        let library = generate_library(DEFAULT_SEED, KPI_LIBRARY_SIZE);
        store
            .write(|w| {
                let (user, profile) = (id(1), id(2));
                w.put_profile(
                    &Profile {
                        meta: SyncMeta::new(profile, NOW),
                        kind: ProfileKind::Personal,
                        name: "Bench".to_owned(),
                        owner_user_id: user,
                    },
                    NOW,
                )?;
                let mut folders = Vec::with_capacity(FOLDERS);
                for i in 0..FOLDERS {
                    let folder = id(10 + u64::try_from(i).unwrap_or(0));
                    // Half the folders are nested one level under the first ten.
                    let parent = (i >= FOLDERS / 2).then(|| folders[i % 10]);
                    w.put_folder(
                        &Folder {
                            meta: SyncMeta::new(folder, NOW),
                            profile_id: profile,
                            parent_id: parent,
                            name: format!("Folder {i}"),
                            position: 0,
                            member_permission: None,
                            defaults: FolderDefaults::default(),
                        },
                        NOW,
                    )?;
                    folders.push(folder);
                }
                for (i, s) in library.iter().enumerate() {
                    let snippet = Snippet::new_defaults(
                        SyncMeta::new(id(1_000 + u64::try_from(i).unwrap_or(0)), NOW),
                        folders[i % FOLDERS],
                        Abbreviation::new(&s.abbreviation).expect("fixture abbreviation"),
                        format!("Snippet {i}"),
                        s.body.clone(),
                        None,
                    );
                    w.put_snippet(&snippet, NOW)?;
                }
                Ok(())
            })
            .expect("fill the fixture store");
        drop(store);
        Self { dir }
    }

    /// Open + `load_library` + `Snapshot::build`, as the KPI defines it.
    fn load(&self) -> Snapshot {
        let store = Store::open(&self.dir).expect("open");
        let mut snippets = store.load_library().expect("load").snippets;
        snippets.sort_by_key(|s| (s.meta.created_at, s.meta.id));
        Snapshot::build(snippets, EngineSettings::default())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn bench_store(c: &mut Criterion) {
    let fixture = Fixture::new();
    c.bench_function("store/load/10000", |b| b.iter(|| black_box(fixture.load())));
}

criterion_group!(benches, bench_store);

/// Times 30 runs individually and writes the percentile record.
fn record_percentiles() -> std::io::Result<()> {
    let fixture = Fixture::new();
    // One warm-up run: the OS cache is warm and the allocator has seen the sizes.
    assert_eq!(fixture.load().len(), KPI_LIBRARY_SIZE);
    let mut rec = LatencyRecorder::with_capacity(RECORDED_RUNS);
    for _ in 0..RECORDED_RUNS {
        let started = Instant::now();
        let snapshot = fixture.load();
        rec.record(started.elapsed());
        assert_eq!(snapshot.len(), KPI_LIBRARY_SIZE);
        assert!(snapshot.excluded().is_empty());
    }
    let mut writer = RunWriter::in_default_dir("store");
    let bench = "store/load/10000";
    let mut lines = Vec::new();
    if let Some(summary) = rec.summary() {
        let mut record = ResultRecord::new(bench, summary);
        record.kpi = Some("store-load-p95".to_owned());
        record.target_stat = Some("p95");
        record.target_ns = Some(LOAD_TARGET_NS);
        record.note = Some(format!(
            "open + load_library + Snapshot::build, {KPI_LIBRARY_SIZE} snippets in {FOLDERS} \
             folders, {RECORDED_RUNS} runs, warm cache"
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
fn report(lines: &[(ResultRecord, PathBuf)], path: &Path) {
    for (r, samples) in lines {
        let s = &r.summary;
        let verdict = match (r.target_value(), r.target_ns) {
            (Some(v), Some(t)) if v < t => " [pass: p95 < target]",
            (Some(_), Some(_)) => " [FAIL: p95 >= target]",
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
