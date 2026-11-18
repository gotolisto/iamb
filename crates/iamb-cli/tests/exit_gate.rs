//! Phase 1 exit gate: the command-line tool prints the correct expansion for any abbreviation in
//! a 10,000-snippet library.

#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use iamb_cli::expand::{DryEnv, expand, personal_profile, snapshot_from_store};
use iamb_core::datetime::LocalDateTime;
use iamb_core::model::{
    Abbreviation, CaseMode, Folder, FolderDefaults, Id, Profile, ProfileKind, Snippet, SyncMeta,
    Timestamp, Trigger,
};
use iamb_core::template::FormValues;
use iamb_store::Store;
use tempfile::TempDir;

const COUNT: usize = 10_000;
const NOW: &str = "2026-03-18T14:05:09";

fn id(tag: u8, n: usize) -> Id {
    let mut b = [0u8; 16];
    b[0] = tag;
    b[8..].copy_from_slice(&(n as u64).to_be_bytes());
    Id::from_bytes(b)
}

fn abbr(i: usize) -> String {
    format!("ab{i:05}")
}

/// What to type for snippet `i` and what the field must hold afterwards.
fn case(i: usize) -> (String, String) {
    match i % 5 {
        0 => (format!("{} ", abbr(i)), format!("plain body {i} ")),
        1 => (format!("{} ", abbr(i)), format!("2026-03-18 14:05 #{i} ")),
        2 => (
            format!("{} ", abbr(i)),
            format!("plain body {} / nested {i} ", i - 2),
        ),
        3 => (abbr(i), format!("immediate {i}")),
        _ => (format!("Ab{i:05} "), format!("Adaptive body {i} ")),
    }
}

fn build_library(dir: &Path) {
    let mut store = Store::open(dir).unwrap();
    let now = Timestamp(1_000);
    let profile = id(1, 0);
    let root = id(2, 0);
    store
        .write(|w| {
            w.put_profile(
                &Profile {
                    meta: SyncMeta::new(profile, now),
                    kind: ProfileKind::Personal,
                    name: "Personal".to_owned(),
                    owner_user_id: id(3, 0),
                },
                now,
            )?;
            w.put_folder(
                &Folder {
                    meta: SyncMeta::new(root, now),
                    profile_id: profile,
                    parent_id: None,
                    name: "Snippets".to_owned(),
                    position: 0,
                    member_permission: None,
                    defaults: FolderDefaults::default(),
                },
                now,
            )?;
            for i in 0..COUNT {
                let body = match i % 5 {
                    0 => format!("plain body {i}"),
                    1 => format!("{{date fmt=\"%Y-%m-%d\"}} {{time}} #{i}"),
                    2 => format!("{{snippet abbr=\"{}\"}} / nested {i}", abbr(i - 2)),
                    3 => format!("immediate {i}"),
                    _ => format!("adaptive body {i}"),
                };
                let mut s = Snippet::new_defaults(
                    SyncMeta::new(id(4, i), now),
                    root,
                    Abbreviation::new(&abbr(i)).unwrap(),
                    "",
                    body,
                    None,
                );
                match i % 5 {
                    3 => {
                        s.trigger = Trigger::Immediate;
                        s.word_start = false;
                        s.case_mode = CaseMode::Exact;
                    }
                    4 => s.case_mode = CaseMode::Adaptive,
                    _ => {}
                }
                w.put_snippet(&s, now)?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn ten_thousand_snippet_library_expands_correctly() {
    let dir = TempDir::new().unwrap();
    let t = Instant::now();
    build_library(dir.path());
    eprintln!("exit gate: built {COUNT} snippets in {:?}", t.elapsed());

    // (b) every snippet, in-process, through the code path of `iamb expand`.
    let store = Store::open(dir.path()).unwrap();
    let profile = personal_profile(&store).unwrap().unwrap();
    let t = Instant::now();
    let snapshot = snapshot_from_store(&store, profile).unwrap();
    let build_time = t.elapsed();
    drop(store);
    let env = DryEnv {
        now: LocalDateTime::parse_iso(NOW).unwrap(),
        clipboard: None,
    };
    let values = FormValues::new();
    let t = Instant::now();
    let mut worst = Duration::ZERO;
    for i in 0..COUNT {
        let (typed, expected) = case(i);
        let one = Instant::now();
        let out = expand(&snapshot, &typed, &env, &values);
        worst = worst.max(one.elapsed());
        assert_eq!(out.text, expected, "snippet {i} typed {typed:?}");
        assert_eq!(out.expansions, 1, "snippet {i}");
        assert!(out.diagnostics.is_empty(), "snippet {i}");
    }
    eprintln!(
        "exit gate: snapshot built in {build_time:?}; {COUNT} expansions in-process in {:?} (slowest {worst:?})",
        t.elapsed()
    );

    // (a) a deterministic sample of 200 through the real binary.
    let t = Instant::now();
    let mut kinds = [0usize; 5];
    for k in 0..200 {
        let i = (k * 49 + k % 13) % COUNT;
        kinds[i % 5] += 1;
        let (typed, expected) = case(i);
        let out = Command::new(env!("CARGO_BIN_EXE_iamb"))
            .arg("--data-dir")
            .arg(dir.path())
            .args(["expand", &typed, "--now", NOW])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "snippet {i}");
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{expected}\n"),
            "snippet {i} typed {typed:?}"
        );
    }
    assert!(
        kinds.iter().all(|&n| n > 0),
        "sample covers every kind: {kinds:?}"
    );
    eprintln!("exit gate: 200 binary runs in {:?}", t.elapsed());
}
