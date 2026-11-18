//! Delivery KPI: 10,000 snippets load in under 100 ms (task 1.5), measured as the test strategy
//! defines it: open the store, load the library and build the engine snapshot, p95 of 30 runs.
//! The limit is 100 ms in release builds and a generous 5 s in debug builds.
//!
//! Run the release figure with `cargo test -p iamb-store --release --locked load`.

mod common;

use std::io::Write;
use std::time::{Duration, Instant};

use common::{Ids, T0, TestResult, folder, profile, snippet, snippet_tag, tag, temp_dir};
use iamb_core::engine::{EngineSettings, Snapshot};
use iamb_store::Store;

const SNIPPETS: usize = 10_000;
const FOLDERS: usize = 50;
const TAGS: usize = 20;
const RUNS: usize = 30;

#[test]
fn open_load_and_build_10000_snippets() -> TestResult {
    let root = temp_dir();
    {
        let mut store = Store::open(root.path())?;
        let mut ids = Ids::default();
        store.write(|w| {
            let (user, p) = (ids.next(), ids.next());
            w.put_profile(&profile(p, user), T0)?;
            let mut folders = Vec::with_capacity(FOLDERS);
            for i in 0..FOLDERS {
                let f = ids.next();
                // Half the folders are nested one level under the first ten.
                let parent = (i >= FOLDERS / 2).then(|| folders[i % 10]);
                w.put_folder(&folder(f, p, parent), T0)?;
                folders.push(f);
            }
            let mut tags = Vec::with_capacity(TAGS);
            for i in 0..TAGS {
                let t = ids.next();
                w.put_tag(&tag(t, p, &format!("tag{i}")), T0)?;
                tags.push(t);
            }
            for i in 0..SNIPPETS {
                let s = ids.next();
                let mut row = snippet(s, folders[i % FOLDERS], &format!("ab{i}"));
                row.body = format!("Snippet number {i}. ").repeat(8);
                w.put_snippet(&row, T0)?;
                if i % 2 == 0 {
                    w.put_snippet_tag(&snippet_tag(ids.next(), s, tags[i % TAGS]), T0)?;
                }
            }
            Ok(())
        })?;
    }

    // The KPI as docs/test-strategy.md defines it: open the store, read everything, build the
    // engine snapshot from the loaded snippets in a stable order. 30 runs, p95.
    let mut times = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let store = Store::open(root.path())?;
        let lib = store.load_library()?;
        let mut snippets = lib.snippets;
        snippets.sort_by_key(|s| (s.meta.created_at, s.meta.id));
        let snapshot = Snapshot::build(snippets, EngineSettings::default());
        let took = started.elapsed();
        times.push(took);
        assert_eq!(snapshot.len(), SNIPPETS);
        assert!(snapshot.excluded().is_empty());
        assert_eq!(lib.folders.len(), FOLDERS);
        assert_eq!(lib.tags.len(), TAGS);
        assert_eq!(lib.snippet_tags.len(), SNIPPETS / 2);
    }
    times.sort();
    // Nearest-rank 95th percentile: the 29th of 30 sorted values.
    let p95 = times[(RUNS * 95).div_ceil(100) - 1];
    let limit = if cfg!(debug_assertions) {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(100)
    };
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    writeln!(
        std::io::stderr().lock(),
        "store/load/{SNIPPETS} (open + load_library + Snapshot::build): {SNIPPETS} snippets in {FOLDERS} folders, {RUNS} runs: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms ({profile}, limit {} ms)",
        times[RUNS / 2].as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        times[RUNS - 1].as_secs_f64() * 1000.0,
        limit.as_millis()
    )?;
    assert!(p95 < limit, "p95 was {p95:?}, limit {limit:?}");
    Ok(())
}
