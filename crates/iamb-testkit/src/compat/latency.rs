//! `iamb-compat latency`: the driver of the end-to-end latency measurement
//! (`docs/test-strategy.md` §6, task 2.7).
//!
//! It starts the probe ([`super::probe`], the `iamb-probe` program), and for each trial resets
//! the probe's control, tells the probe what it is about to type and what the control must then
//! hold, types the abbreviation as ordinary unmarked keys through the system's own key sender
//! ([`super::typist`]), and asks for the sample. At the end it takes the probe's report (the
//! `LatencyRecorder` samples) and writes the result in the format of `iamb-bench`
//! (`target/bench-results/<run id>.jsonl`, one line per measurement, `kpi = "e2e-plain-p95"`
//! on the native probe's line), the raw samples, and one line per trial (no text).
//!
//! # The fixture
//!
//! [`library`]: 1,000 plain snippets, the sample of §2.1: 400 short (10–30 characters), 400
//! medium (31–200) and 200 long (201–500), plain ASCII words, a quarter of the medium and long
//! ones with a line break. Abbreviations are `;l` and three letters, immediate trigger, exact
//! case, so the last key typed is the trigger and no abbreviation ends inside another. The
//! library is fixed (no seed): `iamb-compat setup --latency` imports exactly these. The run's
//! `--seed` only decides the order of the trials.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, Stdio};
use std::time::Duration;

use iamb_bench::latency::summarize_sorted;
use iamb_bench::{ResultRecord, RunWriter, SplitMix64};
use serde::Serialize;
use serde_json::Value;

use super::probe::protocol::{Command, Reply, encode, parse_reply};
use super::probe::session::{Failure, Report, Sample};
use super::types::FIXTURE_JSON;
use super::typist::Typist;

/// The seed of the fixture library. Fixed: the imported library and the run must agree.
const LIBRARY_SEED: u64 = 0x1A3B_2027;
/// The default seed of the trial order.
pub const DEFAULT_ORDER_SEED: u64 = 0x0E2E_0027;
/// Snippets in the fixture: the sample of §2.1.
pub const LIBRARY_SIZE: usize = 1000;
/// The KPI the native probe's line carries.
pub const KPI: &str = "e2e-plain-p95";
/// The KPI's limit: p95 under 50 ms.
pub const TARGET_NS: u64 = 50_000_000;

/// The length classes of §2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// At most 30 characters.
    Short,
    /// 31 to 200.
    Medium,
    /// 201 to 500.
    Long,
}

impl Class {
    /// The name in bench ids.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Medium => "medium",
            Self::Long => "long",
        }
    }

    const fn lengths(self) -> (usize, usize) {
        match self {
            Self::Short => (10, 30),
            Self::Medium => (31, 200),
            Self::Long => (201, 500),
        }
    }

    const ALL: [Self; 3] = [Self::Short, Self::Medium, Self::Long];
}

/// One fixture snippet.
#[derive(Clone, PartialEq, Eq)]
pub struct LatencySnippet {
    /// Its number, 0 to 999.
    pub index: usize,
    /// `;l` and three letters.
    pub abbreviation: String,
    /// The plain-text body: what the probe's control must hold.
    pub body: String,
    /// Its length class.
    pub class: Class,
}

impl std::fmt::Debug for LatencySnippet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatencySnippet")
            .field("index", &self.index)
            .field("class", &self.class)
            .field("chars", &self.body.chars().count())
            .finish_non_exhaustive()
    }
}

const WORDS: &[&str] = &[
    "thanks", "for", "your", "message", "regards", "meeting", "tomorrow", "please", "find",
    "attached", "the", "invoice", "schedule", "call", "address", "street", "support", "ticket",
    "number", "order", "delivery", "week", "office", "team", "project", "update", "review",
    "draft", "notes", "agenda", "Monday", "Friday", "42", "2026", "kind", "best", "and", "a",
];

fn abbreviation(index: usize) -> String {
    let letter = |n: usize| char::from(b'a' + u8::try_from(n % 26).unwrap_or(0));
    format!(
        ";l{}{}{}",
        letter(index / 676),
        letter(index / 26),
        letter(index)
    )
}

fn body(rng: &mut SplitMix64, class: Class) -> String {
    let (lo, hi) = class.lengths();
    let target = rng.range_inclusive(lo, hi);
    let mut text = String::new();
    while text.len() < target {
        if !text.is_empty() {
            text.push(if rng.below(9) == 0 { ',' } else { ' ' });
            if text.ends_with(',') {
                text.push(' ');
            }
        }
        text.push_str(WORDS[rng.index(WORDS.len())]);
    }
    text.truncate(target);
    // No space at either end, so the length class holds after any trimming by a surface.
    while text.ends_with([' ', ',']) {
        text.pop();
        text.push('.');
    }
    if class != Class::Short
        && rng.below(4) == 0
        && let Some(space) = text[text.len() / 2..].find(' ')
    {
        let at = text.len() / 2 + space;
        text.replace_range(at..=at, "\n");
    }
    text
}

/// The fixture library: always the same 1,000 snippets.
pub fn library() -> Vec<LatencySnippet> {
    let mut rng = SplitMix64::new(LIBRARY_SEED);
    (0..LIBRARY_SIZE)
        .map(|index| {
            let class = match index {
                0..400 => Class::Short,
                400..800 => Class::Medium,
                _ => Class::Long,
            };
            LatencySnippet {
                index,
                abbreviation: abbreviation(index),
                body: body(&mut rng, class),
                class,
            }
        })
        .collect()
}

/// The fixture library in the import format of `iamb-store` (the shape of `compat.json`): one
/// folder "Latency", plain snippets, immediate trigger, exact case.
pub fn library_json() -> Result<String, String> {
    let mut doc: Value =
        serde_json::from_str(FIXTURE_JSON).map_err(|e| format!("compat.json: {e}"))?;
    let folder_id = "01a1ef1b-0000-7000-8000-f00000000002";
    let template_folder = doc["folders"][0].clone();
    let template = doc["snippets"][0].clone();
    let mut folder = template_folder;
    folder["id"] = Value::from(folder_id);
    folder["name"] = Value::from("Latency");
    let snippets: Vec<Value> = library()
        .into_iter()
        .map(|s| {
            let mut v = template.clone();
            v["id"] = Value::from(format!("01a1ef1b-0000-7000-8000-6{:011x}", s.index));
            v["abbreviation"] = Value::from(s.abbreviation);
            v["body"] = Value::from(s.body);
            v["label"] = Value::from(format!("Latency {} {:04}", s.class.as_str(), s.index));
            v["folder_id"] = Value::from(folder_id);
            v["kind"] = Value::from("plain");
            v["trigger"] = Value::from("immediate");
            v["case_mode"] = Value::from("exact");
            v["word_start"] = Value::from(false);
            v
        })
        .collect();
    doc["folders"] = Value::from(vec![folder]);
    doc["snippets"] = Value::from(snippets);
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

/// One trial of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// Its id: trial number and snippet number, `t0001-s0417`.
    pub cell: String,
    /// The snippet.
    pub snippet: usize,
    /// Its class.
    pub class: Class,
}

/// `n` trials in the proportions of §2.1 (40% short, 40% medium, 20% long), each class's
/// snippets in a seeded order (repeating when `n` is above 1,000), the whole run shuffled.
pub fn plan(n: usize, seed: u64) -> Vec<Planned> {
    let lib = library();
    let mut rng = SplitMix64::new(seed);
    let short = n * 2 / 5;
    let medium = n * 2 / 5;
    let counts = [short, medium, n - short - medium];
    let mut picks = Vec::with_capacity(n);
    for (class, count) in Class::ALL.into_iter().zip(counts) {
        let mut pool: Vec<usize> = lib
            .iter()
            .filter(|s| s.class == class)
            .map(|s| s.index)
            .collect();
        shuffle(&mut pool, &mut rng);
        picks.extend(pool.iter().cycle().take(count).map(|&i| (i, class)));
    }
    shuffle(&mut picks, &mut rng);
    picks
        .into_iter()
        .enumerate()
        .map(|(t, (snippet, class))| Planned {
            cell: format!("t{:04}-s{snippet:04}", t + 1),
            snippet,
            class,
        })
        .collect()
}

fn shuffle<T>(items: &mut [T], rng: &mut SplitMix64) {
    for i in (1..items.len()).rev() {
        let j = rng.index(i + 1);
        items.swap(i, j);
    }
}

/// A channel to the probe: one command, one reply.
pub trait ProbeLink {
    /// Sends `command` and returns the probe's reply.
    fn call(&mut self, command: &Command) -> Result<Reply, String>;
}

/// The probe as a child process, talking JSON lines on its standard input and output.
#[derive(Debug)]
pub struct ProbeProcess {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl ProbeProcess {
    /// Starts `program` with `args`; its standard error goes to this process's.
    pub fn spawn(program: &Path, args: &[String]) -> Result<Self, String> {
        let mut child = Process::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot start the probe {} ({e}); build it with `cargo build --release -p iamb-testkit --features probe --bin iamb-probe` or pass --probe",
                    program.display()
                )
            })?;
        let input = child
            .stdin
            .take()
            .ok_or("no standard input for the probe")?;
        let output = child
            .stdout
            .take()
            .ok_or("no standard output for the probe")?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output),
        })
    }
}

impl ProbeLink for ProbeProcess {
    fn call(&mut self, command: &Command) -> Result<Reply, String> {
        writeln!(self.input, "{}", encode(command))
            .and_then(|()| self.input.flush())
            .map_err(|e| format!("the probe stopped listening: {e}"))?;
        let mut line = String::new();
        let n = self
            .output
            .read_line(&mut line)
            .map_err(|e| format!("cannot read the probe's answer: {e}"))?;
        if n == 0 {
            return Err("the probe exited (see its message above)".to_owned());
        }
        parse_reply(line.trim_end())
    }
}

impl Drop for ProbeProcess {
    fn drop(&mut self) {
        // After `quit` the probe exits by itself; give it a moment, then make sure.
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ok(reply: Reply, what: &str) -> Result<Option<bool>, String> {
    match reply {
        Reply::Ok { focused } => Ok(focused),
        Reply::Error { message } => Err(format!("{what}: {message}")),
        other => Err(format!("{what}: unexpected answer {other:?}")),
    }
}

/// How the trials go.
#[derive(Debug, Clone)]
pub struct DriveOptions {
    /// Pause between trials.
    pub gap: Duration,
    /// After a match, how long the probe checks the text stays right.
    pub settle: Duration,
    /// How long the probe waits for the last key of a trial.
    pub key_wait: Duration,
}

/// One trial and its sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialResult {
    /// The plan.
    pub planned: Planned,
    /// The probe's sample.
    pub sample: Sample,
    /// The probe's control had the focus after the reset.
    pub focused: bool,
}

/// A finished run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Every trial.
    pub trials: Vec<TrialResult>,
    /// The probe's report.
    pub report: Report,
}

/// Runs `plan` against the probe behind `link`, typing with `typist`. `progress` gets one line
/// every 50 trials and one per failure (cell ids and numbers only).
pub fn drive(
    link: &mut dyn ProbeLink,
    typist: &mut dyn Typist,
    plan: &[Planned],
    opts: &DriveOptions,
    pause: &dyn Fn(Duration),
    progress: &mut dyn FnMut(&str),
) -> Result<Outcome, String> {
    let lib = library();
    let mut trials = Vec::with_capacity(plan.len());
    let settle_ms = u64::try_from(opts.settle.as_millis()).unwrap_or(u64::MAX);
    let key_wait_ms = u64::try_from(opts.key_wait.as_millis()).unwrap_or(u64::MAX);
    for (n, planned) in plan.iter().enumerate() {
        let snippet = lib
            .get(planned.snippet)
            .ok_or_else(|| format!("{}: no such snippet", planned.cell))?;
        let focused = ok(link.call(&Command::Reset)?, "reset")?.unwrap_or(false);
        ok(
            link.call(&Command::Expect {
                cell: planned.cell.clone(),
                typed: snippet.abbreviation.clone(),
                expected: snippet.body.clone(),
                key_wait_ms,
            })?,
            "expect",
        )?;
        typist
            .type_text(&snippet.abbreviation)
            .map_err(|e| format!("{}: typing failed: {e}", planned.cell))?;
        let sample = match link.call(&Command::Wait { settle_ms })? {
            Reply::Sample { sample } => sample,
            Reply::Error { message } => return Err(format!("{}: {message}", planned.cell)),
            other => return Err(format!("{}: unexpected answer {other:?}", planned.cell)),
        };
        if !sample.pass || sample.stable == Some(false) {
            progress(&format!(
                "{}: {}{}",
                planned.cell,
                sample
                    .failure
                    .map_or("matched, then changed", |f| f.as_str()),
                if focused {
                    ""
                } else {
                    " (the probe did not have the focus)"
                }
            ));
        }
        trials.push(TrialResult {
            planned: planned.clone(),
            sample,
            focused,
        });
        if (n + 1) % 50 == 0 {
            let failed = trials.iter().filter(|t| !t.sample.pass).count();
            progress(&format!(
                "{} of {} trials, {failed} failed",
                n + 1,
                plan.len()
            ));
        }
        pause(opts.gap);
    }
    let report = match link.call(&Command::Report)? {
        Reply::Report { report } => report,
        other => return Err(format!("report: unexpected answer {other:?}")),
    };
    Ok(Outcome { trials, report })
}

/// What the result files say, per bench line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The bench id.
    pub bench: String,
    /// The record's KPI.
    pub kpi: Option<String>,
    /// Trials.
    pub count: usize,
    /// p95, ns.
    pub p95: u64,
    /// p95 under [`TARGET_NS`].
    pub pass: bool,
}

/// Where the files went and what they say.
#[derive(Debug, Clone)]
pub struct Written {
    /// The JSON-lines result.
    pub result: PathBuf,
    /// One line per trial (no text).
    pub trials: PathBuf,
    /// The bench lines.
    pub lines: Vec<Line>,
}

#[derive(Serialize)]
struct TrialLine<'a> {
    cell: &'a str,
    snippet: usize,
    class: Class,
    chars: usize,
    pass: bool,
    stable: Option<bool>,
    focused: bool,
    latency_ns: u64,
    failure: Option<&'static str>,
}

/// Writes the run into `dir`: `<run id>.jsonl` (the `iamb-bench` result format), the sorted
/// samples of each line under `<run id>.samples/`, and `<run id>.trials.jsonl`. `surface` is
/// `native` or `browser`; only the native line carries the KPI.
pub fn write_results(outcome: &Outcome, surface: &str, dir: &Path) -> Result<Written, String> {
    let lib = library();
    let mut writer = RunWriter::new("e2e-latency", dir.to_path_buf());
    let prefix = if surface == "native" {
        "e2e/plain".to_owned()
    } else {
        format!("e2e/plain-{surface}")
    };
    let n = outcome.trials.len();
    let passed = outcome.trials.iter().filter(|t| t.sample.pass).count();
    let stable = outcome
        .trials
        .iter()
        .filter(|t| t.sample.pass && t.sample.stable != Some(false))
        .count();
    let mut groups: Vec<(String, Option<String>, Vec<u64>, String)> = Vec::new();
    groups.push((
        format!("{prefix}/{n}"),
        (surface == "native").then(|| KPI.to_owned()),
        outcome.report.sorted_ns.clone(),
        format!(
            "{surface} probe; {n} trials, {passed} matched within 2 s, {} failed (counted as 2 s), {stable} still matched after the settle time; T0 and T1 in the probe's process",
            n - passed
        ),
    ));
    for class in Class::ALL {
        let mut v: Vec<u64> = outcome
            .trials
            .iter()
            .filter(|t| t.planned.class == class)
            .map(|t| t.sample.latency_ns)
            .collect();
        if v.is_empty() {
            continue;
        }
        v.sort_unstable();
        let (lo, hi) = class.lengths();
        groups.push((
            format!("{prefix}-{}/{}", class.as_str(), v.len()),
            None,
            v,
            format!(
                "{surface} probe; {} bodies of {lo}-{hi} characters",
                class.as_str()
            ),
        ));
    }
    let mut lines = Vec::new();
    for (bench, kpi, sorted, note) in groups {
        let Some(summary) = summarize_sorted(&sorted) else {
            continue;
        };
        let mut record = ResultRecord::new(bench.clone(), summary);
        record.kpi.clone_from(&kpi);
        record.target_stat = Some("p95");
        record.target_ns = Some(TARGET_NS);
        record.note = Some(note);
        writer.push(&record);
        writer
            .write_samples(&bench, &sorted)
            .map_err(|e| format!("cannot write the samples: {e}"))?;
        lines.push(Line {
            bench,
            kpi,
            count: summary.count,
            p95: summary.p95,
            pass: summary.p95 < TARGET_NS,
        });
    }
    let trials_path = dir.join(format!("{}.trials.jsonl", writer.meta().run_id));
    let mut text = String::new();
    for t in &outcome.trials {
        let chars = lib
            .get(t.planned.snippet)
            .map_or(0, |s| s.body.chars().count());
        text.push_str(&encode(&TrialLine {
            cell: &t.planned.cell,
            snippet: t.planned.snippet,
            class: t.planned.class,
            chars,
            pass: t.sample.pass,
            stable: t.sample.stable,
            focused: t.focused,
            latency_ns: t.sample.latency_ns,
            failure: t.sample.failure.map(Failure::as_str),
        }));
        text.push('\n');
    }
    let result = writer
        .finish()
        .map_err(|e| format!("cannot write the result in {}: {e}", dir.display()))?;
    std::fs::write(&trials_path, text)
        .map_err(|e| format!("cannot write {}: {e}", trials_path.display()))?;
    Ok(Written {
        result,
        trials: trials_path,
        lines,
    })
}

/// The abbreviation → body table, for simulations and checks.
pub fn bodies_by_abbreviation() -> HashMap<String, String> {
    library()
        .into_iter()
        .map(|s| (s.abbreviation, s.body))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::compat::probe::serve::{Probe, ServeOptions, Surface, answer};
    use crate::compat::probe::session::TIMEOUT_NS;

    #[test]
    fn the_library_has_the_sample_of_the_kpi() {
        let lib = library();
        assert_eq!(lib.len(), 1000);
        assert_eq!(lib, library(), "fixed");
        let mut seen = std::collections::HashSet::new();
        for s in &lib {
            let n = s.body.chars().count();
            let (lo, hi) = s.class.lengths();
            assert!((lo..=hi).contains(&n), "{} has {n} chars", s.index);
            assert!(s.body.is_ascii() && !s.body.starts_with(' ') && !s.body.ends_with(' '));
            assert_eq!(s.abbreviation.len(), 5);
            assert!(s.abbreviation.starts_with(";l"));
            assert!(seen.insert(s.abbreviation.clone()), "unique");
        }
        let count = |c| lib.iter().filter(|s| s.class == c).count();
        assert_eq!(
            (
                count(Class::Short),
                count(Class::Medium),
                count(Class::Long)
            ),
            (400, 400, 200)
        );
        assert!(lib.iter().any(|s| s.body.contains('\n')));
        assert!(!format!("{:?}", lib[0]).contains(&lib[0].body));
    }

    #[test]
    fn the_plan_keeps_the_proportions_and_the_seed_decides_the_order() {
        let full = plan(1000, 7);
        assert_eq!(full.len(), 1000);
        let mut snippets: Vec<usize> = full.iter().map(|p| p.snippet).collect();
        snippets.sort_unstable();
        assert_eq!(snippets, (0..1000).collect::<Vec<_>>(), "each snippet once");
        assert_eq!(full, plan(1000, 7));
        assert_ne!(full, plan(1000, 8));
        let small = plan(10, 1);
        let count = |c| small.iter().filter(|p| p.class == c).count();
        assert_eq!(
            (
                count(Class::Short),
                count(Class::Medium),
                count(Class::Long)
            ),
            (4, 4, 2)
        );
        assert_eq!(small[0].cell.len(), "t0001-s0000".len());
        assert_eq!(plan(1200, 1).len(), 1200);
    }

    #[test]
    fn the_library_json_has_every_snippet() {
        let json: Value = serde_json::from_str(&library_json().unwrap()).unwrap();
        let snippets = json["snippets"].as_array().unwrap();
        assert_eq!(snippets.len(), 1000);
        assert_eq!(snippets[17]["abbreviation"], Value::from(abbreviation(17)));
        assert_eq!(snippets[17]["trigger"], Value::from("immediate"));
        assert_eq!(json["folders"][0]["name"], Value::from("Latency"));
    }

    // ---- a whole run against a simulated machine -----------------------------------------

    struct Field;

    impl Surface for Field {
        fn kind(&self) -> &'static str {
            "native"
        }
        fn reset(&self) -> Result<bool, String> {
            Ok(true)
        }
    }

    struct Local {
        probe: Arc<Probe>,
    }

    impl ProbeLink for Local {
        fn call(&mut self, command: &Command) -> Result<Reply, String> {
            Ok(answer(
                &self.probe,
                &Field,
                command.clone(),
                &ServeOptions::default(),
            ))
        }
    }

    /// Plays the keyboard, the probe window and an expander that takes `delay` and does not
    /// know the abbreviation `broken`.
    struct Machine {
        probe: Arc<Probe>,
        bodies: HashMap<String, String>,
        broken: String,
        delay: Duration,
    }

    impl Typist for Machine {
        fn type_text(&mut self, text: &str) -> Result<(), String> {
            let mut field = String::new();
            for ch in text.chars() {
                field.push(ch);
                self.probe.key_at(Instant::now(), ch);
                self.probe.text_at(Instant::now(), &field);
            }
            if text != self.broken {
                let body = self.bodies[text].clone();
                let probe = Arc::clone(&self.probe);
                let delay = self.delay;
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    probe.text_at(Instant::now(), "");
                    probe.text_at(Instant::now(), &body.replace('\n', "\r"));
                });
            }
            Ok(())
        }
        fn select_all_copy(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn clear_field(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_run_against_a_simulated_machine_writes_the_bench_result() {
        let probe = Arc::new(Probe::new(Duration::ZERO));
        let trials = plan(10, 3);
        let lib = library();
        let broken = lib[trials[4].snippet].abbreviation.clone();
        let mut machine = Machine {
            probe: Arc::clone(&probe),
            bodies: bodies_by_abbreviation(),
            broken,
            delay: Duration::from_millis(3),
        };
        let mut link = Local { probe };
        let mut lines = Vec::new();
        let outcome = drive(
            &mut link,
            &mut machine,
            &trials,
            &DriveOptions {
                gap: Duration::ZERO,
                settle: Duration::from_millis(20),
                key_wait: Duration::from_secs(2),
            },
            &|_| {},
            &mut |l: &str| lines.push(l.to_owned()),
        )
        .unwrap();
        assert_eq!(outcome.trials.len(), 10);
        let failed: Vec<&TrialResult> = outcome.trials.iter().filter(|t| !t.sample.pass).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].planned, trials[4]);
        assert_eq!(failed[0].sample.failure, Some(Failure::Timeout));
        assert_eq!(failed[0].sample.latency_ns, TIMEOUT_NS);
        for t in outcome.trials.iter().filter(|t| t.sample.pass) {
            assert!(t.sample.latency_ns >= 3_000_000, "{}", t.sample.latency_ns);
            assert_eq!(t.sample.stable, Some(true));
        }
        assert_eq!(lines.len(), 1, "one line for the failure: {lines:?}");
        assert!(lines[0].starts_with(&trials[4].cell));

        let dir = tempfile::tempdir().unwrap();
        let written = write_results(&outcome, "native", dir.path()).unwrap();
        let text = std::fs::read_to_string(&written.result).unwrap();
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["bench"], Value::from("e2e/plain/10"));
        assert_eq!(first["kpi"], Value::from(KPI));
        assert_eq!(first["count"], Value::from(10));
        assert_eq!(first["target_stat"], Value::from("p95"));
        assert_eq!(first["target_ns"], Value::from(TARGET_NS));
        // p95 of 10 is the largest: the 2 s failure.
        assert_eq!(first["p95"], Value::from(TIMEOUT_NS));
        assert_eq!(first["pass"], Value::from(false));
        assert_eq!(text.lines().count(), 4, "the run and three classes");
        assert!(text.contains("\"bench\":\"e2e/plain-short/4\",\"kpi\":null"));
        let trials_text = std::fs::read_to_string(&written.trials).unwrap();
        assert_eq!(trials_text.lines().count(), 10);
        for t in &trials {
            let s = &lib[t.snippet];
            let words: Vec<&str> = s.body.split_whitespace().filter(|w| w.len() > 6).collect();
            for w in words {
                assert!(!trials_text.contains(w), "no body text in the trials file");
            }
            assert!(!trials_text.contains(&s.abbreviation));
            assert!(!text.contains(&s.abbreviation));
        }
        let samples = dir
            .path()
            .join(format!(
                "{}.samples",
                written.result.file_stem().unwrap().to_str().unwrap()
            ))
            .join("e2e_plain_10.txt");
        assert_eq!(
            std::fs::read_to_string(samples).unwrap().lines().count(),
            10
        );
        assert_eq!(written.lines[0].count, 10);
        assert!(!written.lines[0].pass);

        let browser = write_results(&outcome, "browser", dir.path()).unwrap();
        let browser_text = std::fs::read_to_string(browser.result).unwrap();
        assert!(browser_text.contains("\"bench\":\"e2e/plain-browser/10\",\"kpi\":null"));
    }
}
