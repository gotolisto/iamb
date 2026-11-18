//! Kill-during-write recovery: a child process writes snippets one transaction at a time and
//! reports each commit; the parent kills it mid-stream and checks that the database is intact
//! and that every reported commit is present.
//!
//! The child is this same test binary, re-run with `--exact child_writer --ignored` and
//! `IAMB_STORE_KILL_CHILD_DIR` set, so the scenario works the same on Linux, macOS and Windows.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{Ids, T0, TestResult, folder, id_from, profile, snippet, temp_dir};
use iamb_core::model::Id;
use iamb_store::{ListOptions, Store};

const CHILD_DIR_VAR: &str = "IAMB_STORE_KILL_CHILD_DIR";
const COMMIT_PREFIX: &str = "COMMITTED ";
const RUNS: u32 = 5;
const MIN_COMMITS: usize = 200;

/// The writer routine. Does nothing unless run as the child of `kill_during_write_loses_nothing`.
#[test]
#[ignore = "helper process for kill_during_write_loses_nothing"]
fn child_writer() -> TestResult {
    let Some(dir) = std::env::var_os(CHILD_DIR_VAR) else {
        return Ok(());
    };
    let mut store = Store::open_with_timeout(std::path::Path::new(&dir), Duration::from_secs(5))?;
    let folder_id = store
        .list_folders(ListOptions::LIVE)?
        .first()
        .ok_or("no folder")?
        .meta
        .id;
    let mut out = std::io::stdout().lock();
    // Ids well clear of the parent's; unique abbreviation per snippet.
    for n in 1_000_000u64..2_000_000 {
        let id = id_from(n);
        let mut s = snippet(id, folder_id, &format!("k{n}"));
        s.body = "x".repeat(200);
        store.put_snippet(&s, T0)?;
        // Reported only after the commit returned.
        writeln!(out, "{COMMIT_PREFIX}{id}")?;
        out.flush()?;
    }
    Ok(())
}

/// A small pseudo-random number from the clock, so each run is killed at a different moment.
fn jitter(max: u64) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    u64::from(nanos) % max
}

fn one_run(run: u32) -> Result<usize, Box<dyn std::error::Error>> {
    let root = temp_dir();
    {
        let mut store = Store::open(root.path())?;
        let mut ids = Ids::default();
        let (user, p, f) = (ids.next(), ids.next(), ids.next());
        store.put_profile(&profile(p, user), T0)?;
        store.put_folder(&folder(f, p, None), T0)?;
    }

    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "child_writer",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_DIR_VAR, root.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("no child stdout")?;
    let target = MIN_COMMITS + usize::try_from(jitter(150))?;

    let mut committed: Vec<Id> = Vec::with_capacity(target);
    let mut lines = BufReader::new(stdout).lines();
    while committed.len() < target {
        let Some(line) = lines.next() else {
            break;
        };
        if let Some(id) = line?.strip_prefix(COMMIT_PREFIX) {
            committed.push(Id::from_str(id.trim())?);
        }
    }
    child.kill()?;
    child.wait()?;
    // Lines the child printed between our last read and the kill also count as committed.
    for line in lines {
        let Ok(line) = line else { break };
        if let Some(id) = line.strip_prefix(COMMIT_PREFIX) {
            committed.push(Id::from_str(id.trim())?);
        }
    }
    assert!(
        committed.len() >= target,
        "run {run}: child stopped after {} commits",
        committed.len()
    );

    let store = Store::open_with_timeout(root.path(), Duration::from_secs(5))?;
    assert_eq!(store.integrity_check()?, vec!["ok".to_owned()], "run {run}");
    let present: std::collections::HashSet<Id> = store
        .list_snippets(ListOptions::LIVE)?
        .into_iter()
        .map(|s| s.meta.id)
        .collect();
    let missing: Vec<&Id> = committed
        .iter()
        .filter(|id| !present.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "run {run}: {} committed snippets lost: {missing:?}",
        missing.len()
    );
    // Every snippet has its outbox entry (written in the same transaction).
    let outbox = store.outbox_read(usize::MAX)?;
    assert_eq!(
        outbox.len(),
        present.len() + 2,
        "run {run}: outbox matches rows"
    );
    Ok(committed.len())
}

#[test]
fn kill_during_write_loses_nothing() -> TestResult {
    if std::env::var_os(CHILD_DIR_VAR).is_some() {
        return Ok(());
    }
    let mut total = 0;
    for run in 0..RUNS {
        total += one_run(run)?;
    }
    let mut err = std::io::stderr().lock();
    writeln!(
        err,
        "kill recovery: {RUNS} runs, {total} reported commits verified"
    )?;
    Ok(())
}
