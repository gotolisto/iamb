//! Delivery KPI: 10,000 snippets load in under 100 ms (task 1.5).
//!
//! Run the release figure with `cargo test -p iamb-store --release --locked load`.

mod common;

use std::io::Write;
use std::time::{Duration, Instant};

use common::{Ids, T0, TestResult, folder, profile, snippet, snippet_tag, tag, temp_dir};
use iamb_store::Store;

const SNIPPETS: usize = 10_000;
const FOLDERS: usize = 50;
const TAGS: usize = 20;
const RUNS: usize = 5;

#[test]
fn load_10000_snippets() -> TestResult {
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

    // Open as the worker would, then time the read.
    let store = Store::open(root.path())?;
    let mut best = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let lib = store.load_library()?;
        let took = started.elapsed();
        best = best.min(took);
        assert_eq!(lib.snippets.len(), SNIPPETS);
        assert_eq!(lib.folders.len(), FOLDERS);
        assert_eq!(lib.tags.len(), TAGS);
        assert_eq!(lib.snippet_tags.len(), SNIPPETS / 2);
    }
    let limit = if cfg!(debug_assertions) {
        Duration::from_secs(1)
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
        "load_library: {SNIPPETS} snippets in {FOLDERS} folders, best of {RUNS}: {:.2} ms ({profile}, limit {} ms)",
        best.as_secs_f64() * 1000.0,
        limit.as_millis()
    )?;
    assert!(best < limit, "load_library took {best:?}, limit {limit:?}");
    Ok(())
}
