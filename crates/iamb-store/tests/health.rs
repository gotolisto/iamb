//! Library health against fixture libraries with planted problems (task 3.10).
//!
//! `tests/fixtures/health/*.json` holds one library per kind of problem, and one mixed library
//! of realistic size. Each file lists its snippets (with the usage they had) and, under
//! `expect`, exactly the problems that were planted, one line each:
//!
//! | Line | Meaning |
//! | --- | --- |
//! | `error <key> <CODE>` with optional ` ref=<abbr>` and ` in=<abbr>` | will not expand: template error; `ref` names the snippet a broken `{snippet}` points at, `in` the included snippet that holds the mistake |
//! | `taken <key> kept=<key>` | will not expand: another snippet holds the abbreviation |
//! | `never_reached <key> by=<key>` | `by` expands immediately and hides `key` |
//! | `unused <key> last_day=<n>` or `unused <key> never` | not used for 90 days |
//!
//! The test fails when a planted problem is missed (detection below 100%) and when anything
//! that was not planted is reported (a false alarm), and prints the detection rate of every
//! file. A file with no `expect` lines is a library that must come out clean.

#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::format_push_string,
    reason = "a test that prints the detection rate and the timing it measured, in percent"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use iamb_core::engine::{EngineSettings, Snapshot};
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp, Trigger};
use iamb_store::Usage;
use iamb_store::health::{self, Findings, Why};
use serde_json::Value;

const DAY: i64 = 86_400_000;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/health")
}

fn id_of(n: usize) -> Id {
    let mut b = [0_u8; 16];
    b[8..].copy_from_slice(&u64::try_from(n + 1).unwrap_or(0).to_be_bytes());
    Id::from_bytes(b)
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn day_of(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

struct Loaded {
    snippets: Vec<Snippet>,
    keys: Vec<String>,
    usage: Vec<Usage>,
    since: Option<Timestamp>,
    now: Timestamp,
    expect: BTreeSet<String>,
}

fn load(file: &Value) -> Loaded {
    let list = file["snippets"].as_array().expect("snippets");
    let mut snippets = Vec::new();
    let mut keys = Vec::new();
    let mut usage = Vec::new();
    for (n, s) in list.iter().enumerate() {
        let key = str_of(s, "key").expect("key").to_owned();
        let created = Timestamp(day_of(s, "created_day").unwrap_or(0) * DAY);
        let mut snippet = Snippet::new_defaults(
            SyncMeta::new(id_of(n), created),
            id_of(10_000),
            Abbreviation::new(str_of(s, "abbr").expect("abbr")).expect("abbreviation"),
            str_of(s, "label").unwrap_or(&key),
            str_of(s, "body").unwrap_or("text"),
            None,
        );
        if str_of(s, "trigger") == Some("immediate") {
            snippet.trigger = Trigger::Immediate;
            snippet.word_start = false;
        }
        snippet.enabled = s.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        if let Some(u) = s.get("used").filter(|u| !u.is_null()) {
            usage.push(Usage {
                snippet_id: id_of(n),
                count: u["count"].as_u64().unwrap_or(1),
                chars_saved: 0,
                last_used: Some(Timestamp(day_of(u, "day").expect("day") * DAY)),
            });
        }
        keys.push(key);
        snippets.push(snippet);
    }
    Loaded {
        snippets,
        keys,
        usage,
        since: day_of(file, "usage_reset_day").map(|d| Timestamp(d * DAY)),
        now: Timestamp(day_of(file, "now_day").expect("now_day") * DAY),
        expect: file["expect"]
            .as_array()
            .expect("expect")
            .iter()
            .map(|e| e.as_str().expect("line").to_owned())
            .collect(),
    }
}

/// What the library reports, in the grammar of the fixture files.
fn lines(l: &Loaded, found: &Findings) -> BTreeSet<String> {
    let key = |id: Id| {
        let at = l
            .snippets
            .iter()
            .position(|s| s.meta.id == id)
            .expect("known snippet");
        l.keys[at].clone()
    };
    let abbr = |id: Id| {
        let at = l
            .snippets
            .iter()
            .position(|s| s.meta.id == id)
            .expect("known snippet");
        l.snippets[at].abbreviation.as_str().to_owned()
    };
    let mut out = BTreeSet::new();
    for e in &found.excluded {
        out.insert(match &e.why {
            Why::Duplicate { kept } => format!("taken {} kept={}", key(e.snippet), key(*kept)),
            Why::Template {
                code,
                reference,
                included,
            } => {
                let mut line = format!("error {} {code}", key(e.snippet));
                if let Some(r) = reference {
                    line.push_str(&format!(" ref={r}"));
                }
                if let Some(i) = included {
                    line.push_str(&format!(" in={i}"));
                }
                line
            }
        });
    }
    for s in &found.shadowed {
        let _ = abbr(s.by);
        out.insert(format!("never_reached {} by={}", key(s.hidden), key(s.by)));
    }
    for u in &found.unused {
        out.insert(match u.last_used {
            Some(t) => format!("unused {} last_day={}", key(u.snippet), t.0 / DAY),
            None => format!("unused {} never", key(u.snippet)),
        });
    }
    out
}

#[test]
fn every_planted_problem_is_found_and_nothing_else() {
    let mut files: Vec<PathBuf> = fs::read_dir(dir())
        .expect("fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 8,
        "one fixture per problem kind and a mixed library"
    );

    let (mut planted, mut detected, mut false_alarms) = (0_usize, 0_usize, 0_usize);
    let mut failures = Vec::new();
    for path in &files {
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let file: Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("JSON");
        let l = load(&file);
        let snapshot = Snapshot::build(l.snippets.iter().cloned(), EngineSettings::default());
        let found = health::analyse(&snapshot, l.snippets.iter(), &l.usage, l.since, l.now);
        let got = lines(&l, &found);

        let missed: Vec<&String> = l.expect.difference(&got).collect();
        let extra: Vec<&String> = got.difference(&l.expect).collect();
        let hit = l.expect.len() - missed.len();
        println!(
            "{name}: {} snippets, {hit}/{} planted problems detected ({:.0}%), {} not planted reported",
            l.snippets.len(),
            l.expect.len(),
            if l.expect.is_empty() {
                100.0
            } else {
                100.0 * hit as f64 / l.expect.len() as f64
            },
            extra.len()
        );
        planted += l.expect.len();
        detected += hit;
        false_alarms += extra.len();
        if !missed.is_empty() || !extra.is_empty() {
            failures.push(format!("{name}: missed {missed:?}; not planted {extra:?}"));
        }
    }
    let rate = 100.0 * detected as f64 / planted.max(1) as f64;
    println!(
        "detection: {detected}/{planted} planted problems ({rate:.1}%), {false_alarms} false alarms"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!((rate - 100.0).abs() < f64::EPSILON);
}

#[test]
fn the_sidebar_count_is_the_number_of_snippets_reported_once() {
    let file: Value =
        serde_json::from_str(&fs::read_to_string(dir().join("mixed.json")).expect("mixed fixture"))
            .expect("JSON");
    let l = load(&file);
    let snapshot = Snapshot::build(l.snippets.iter().cloned(), EngineSettings::default());
    let found = health::analyse(&snapshot, l.snippets.iter(), &l.usage, l.since, l.now);
    let keys: BTreeSet<&str> = l
        .expect
        .iter()
        .map(|e| e.split(' ').nth(1).expect("key"))
        .collect();
    assert_eq!(found.affected(), keys.len());
}

/// 10,000 snippets, a tenth of them with a problem: health must stay fast. The snapshot is
/// built once per change of the library already; this times what `Health` adds to it.
#[test]
fn health_over_ten_thousand_snippets_is_fast() {
    use std::time::Instant;
    let now = Timestamp(2000 * DAY);
    let mut snippets = Vec::new();
    let mut usage = Vec::new();
    for n in 0..10_000_usize {
        let body = match n % 100 {
            // A missing reference, a nested one, a cycle and a parse error.
            0 => "{snippet abbr=\";nothing\"}".to_owned(),
            1 => format!("{{snippet abbr=\";s{:05}\"}}", n - 1),
            2 => format!("{{snippet abbr=\";s{n:05}\"}}"),
            3 => "oops {date".to_owned(),
            _ if n > 100 && n % 7 == 0 => format!("see {{snippet abbr=\";s{:05}\"}}", n - 100 + 4),
            _ => "Thanks for your order.".to_owned(),
        };
        let mut s = Snippet::new_defaults(
            SyncMeta::new(id_of(n), Timestamp(DAY)),
            id_of(20_000),
            Abbreviation::new(&match n % 100 {
                50 => format!(";x{n}"),
                51 => format!(";x{}y", n - 1),
                _ => format!(";s{n:05}"),
            })
            .expect("abbreviation"),
            format!("Snippet {n}"),
            body,
            None,
        );
        if n % 100 == 50 {
            // An immediate snippet whose abbreviation starts the next one's.
            s.trigger = Trigger::Immediate;
            s.word_start = false;
        }
        // Nine in ten were used lately, so the rest are reported unused.
        if n % 10 != 9 {
            usage.push(Usage {
                snippet_id: id_of(n),
                count: 3,
                chars_saved: 0,
                last_used: Some(Timestamp(1990 * DAY)),
            });
        }
        snippets.push(s);
    }
    let built = Instant::now();
    let snapshot = Snapshot::build(snippets.iter().cloned(), EngineSettings::default());
    let build_ms = built.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let found = health::analyse(&snapshot, snippets.iter(), &usage, None, now);
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "health over {} snippets: {ms:.1} ms ({} will not expand, {} never reached, {} unused); the snapshot build, once per change, takes {build_ms:.0} ms",
        snippets.len(),
        found.excluded.len(),
        found.shadowed.len(),
        found.unused.len()
    );
    assert!(found.excluded.len() >= 300 && !found.unused.is_empty());
    assert!(ms < 1000.0, "health took {ms:.0} ms");
}
