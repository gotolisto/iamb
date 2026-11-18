//! `bench-baseline` and `bench-compare`: the benchmark regression gate
//! (`docs/test-strategy.md` §3, "Regression rule").
//!
//! A *run* is one directory of `*.jsonl` result files written by `crates/iamb-bench` (searched
//! recursively, so a downloaded workflow artifact can be passed as is). Every bench id appears
//! once per run. For each bench the *gated statistic* is the record's `target_stat` when set,
//! else `p50`.
//!
//! - Baseline value: the median of the gated statistic over the baseline runs (for an even
//!   number of runs, the mean of the two middle values, rounded down to whole nanoseconds).
//! - Spread: maximum minus minimum of the gated statistic over the baseline runs.
//! - Noise floor: `max(2 × spread, 1 µs)`.
//! - A run regresses a bench when its value is more than 25% above the baseline value **and**
//!   more than the noise floor above it (both strict).
//! - The gate fails only when the same bench regresses in both runs given to `bench-compare`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::json::{Object, Value, parse_flat_object, quote};

/// Line format version of the result files this reads and of the baseline file it writes.
const SCHEMA: u64 = 1;
/// Statistics a bench may be gated on (the ones `ResultRecord::target_value` knows).
const STATS: [&str; 4] = ["p50", "p95", "p99", "max"];
/// Gated statistic for a record without `target_stat`.
const DEFAULT_STAT: &str = "p50";
/// Smallest noise floor, in nanoseconds.
pub(crate) const MIN_FLOOR_NS: u64 = 1_000;
/// Number of baseline runs the test strategy asks for; fewer is allowed with a warning.
const RECOMMENDED_RUNS: usize = 10;
/// Baseline used by CI, relative to the repository root.
pub(crate) const CI_BASELINE: &str = "crates/iamb-bench/baselines/ci-ubuntu.jsonl";

pub(crate) const BASELINE_USAGE: &str =
    "usage: cargo xtask bench-baseline [--out <file>] <run-dir> <run-dir>...";
pub(crate) const COMPARE_USAGE: &str =
    "usage: cargo xtask bench-compare [--baseline <file>] <run-1-dir> <run-2-dir>";

/// One bench's result in one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Measurement {
    pub(crate) unit: String,
    pub(crate) target_stat: Option<String>,
    pub(crate) p50: u64,
    pub(crate) p95: u64,
    pub(crate) p99: u64,
    pub(crate) max: u64,
}

impl Measurement {
    fn stat(&self, name: &str) -> Option<u64> {
        match name {
            "p50" => Some(self.p50),
            "p95" => Some(self.p95),
            "p99" => Some(self.p99),
            "max" => Some(self.max),
            _ => None,
        }
    }

    fn gated_stat(&self) -> &str {
        self.target_stat.as_deref().unwrap_or(DEFAULT_STAT)
    }
}

/// All results of one run directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Run {
    /// How the run is named in messages: its directory.
    pub(crate) label: String,
    pub(crate) git_sha: String,
    pub(crate) runner: String,
    pub(crate) benches: BTreeMap<String, Measurement>,
}

/// One line of the baseline file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaselineEntry {
    pub(crate) bench: String,
    pub(crate) stat: String,
    pub(crate) unit: String,
    pub(crate) baseline: u64,
    pub(crate) spread: u64,
    pub(crate) runs: usize,
    pub(crate) git_sha: String,
    pub(crate) runner: String,
}

impl BaselineEntry {
    fn to_line(&self) -> String {
        format!(
            "{{\"schema\":{SCHEMA},\"bench\":{},\"stat\":{},\"unit\":{},\"baseline\":{},\
             \"spread\":{},\"runs\":{},\"git_sha\":{},\"runner\":{}}}",
            quote(&self.bench),
            quote(&self.stat),
            quote(&self.unit),
            self.baseline,
            self.spread,
            self.runs,
            quote(&self.git_sha),
            quote(&self.runner),
        )
    }

    fn from_object(o: &Object) -> Result<Self, String> {
        check_schema(o)?;
        let stat = get_str(o, "stat")?;
        if !STATS.contains(&stat.as_str()) {
            return Err(format!("unknown stat `{stat}`"));
        }
        Ok(Self {
            bench: get_str(o, "bench")?,
            stat,
            unit: get_str(o, "unit")?,
            baseline: get_u64(o, "baseline")?,
            spread: get_u64(o, "spread")?,
            runs: usize::try_from(get_u64(o, "runs")?).map_err(|e| e.to_string())?,
            git_sha: get_str(o, "git_sha")?,
            runner: get_str(o, "runner")?,
        })
    }
}

fn get_str(o: &Object, key: &str) -> Result<String, String> {
    match o.get(key) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(_) => Err(format!("field `{key}` is not a string")),
        None => Err(format!("missing field `{key}`")),
    }
}

fn get_opt_str(o: &Object, key: &str) -> Result<Option<String>, String> {
    match o.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("field `{key}` is not a string or null")),
    }
}

fn get_u64(o: &Object, key: &str) -> Result<u64, String> {
    match o.get(key) {
        Some(Value::Num(n)) => n
            .parse()
            .map_err(|_| format!("field `{key}` is not a non-negative integer: {n}")),
        Some(_) => Err(format!("field `{key}` is not a number")),
        None => Err(format!("missing field `{key}`")),
    }
}

fn check_schema(o: &Object) -> Result<(), String> {
    let schema = get_u64(o, "schema")?;
    if schema == SCHEMA {
        Ok(())
    } else {
        Err(format!("schema {schema}, expected {SCHEMA}"))
    }
}

/// Median of ascending `sorted`; for an even count, the mean of the two middle values rounded
/// down. `None` when empty.
pub(crate) fn median(sorted: &[u64]) -> Option<u64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    let hi = sorted[n / 2];
    if n % 2 == 1 {
        return Some(hi);
    }
    let lo = sorted[n / 2 - 1];
    Some(lo + (hi - lo) / 2)
}

/// Spread of `values`: maximum minus minimum (0 when empty).
pub(crate) fn spread(values: &[u64]) -> u64 {
    match (values.iter().min(), values.iter().max()) {
        (Some(lo), Some(hi)) => hi - lo,
        _ => 0,
    }
}

/// Noise floor for a bench with this spread: twice the spread, at least [`MIN_FLOOR_NS`].
pub(crate) fn floor(spread: u64) -> u64 {
    spread.saturating_mul(2).max(MIN_FLOOR_NS)
}

/// Whether `value` regresses against `baseline`: more than 25% worse and more than the floor.
pub(crate) fn regresses(baseline: u64, spread: u64, value: u64) -> bool {
    let over_25_percent = u128::from(value) * 4 > u128::from(baseline) * 5;
    over_25_percent && value.saturating_sub(baseline) > floor(spread)
}

/// Parses the result files of one run. `files` holds (file name, contents).
pub(crate) fn parse_run(label: &str, files: &[(String, String)]) -> Result<Run, String> {
    let mut shas = BTreeSet::new();
    let mut runners = BTreeSet::new();
    let mut benches = BTreeMap::new();
    for (name, text) in files {
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let at = || format!("{name} line {}", i + 1);
            let o = parse_flat_object(line).map_err(|e| format!("{}: {e}", at()))?;
            let rec = (|| -> Result<(String, Measurement), String> {
                check_schema(&o)?;
                shas.insert(get_str(&o, "git_sha")?);
                runners.insert(get_str(&o, "runner")?);
                let m = Measurement {
                    unit: get_str(&o, "unit")?,
                    target_stat: get_opt_str(&o, "target_stat")?,
                    p50: get_u64(&o, "p50")?,
                    p95: get_u64(&o, "p95")?,
                    p99: get_u64(&o, "p99")?,
                    max: get_u64(&o, "max")?,
                };
                if m.stat(m.gated_stat()).is_none() {
                    return Err(format!("unknown target_stat `{}`", m.gated_stat()));
                }
                Ok((get_str(&o, "bench")?, m))
            })()
            .map_err(|e| format!("{}: {e}", at()))?;
            if benches.insert(rec.0.clone(), rec.1).is_some() {
                return Err(format!(
                    "run {label}: bench `{}` appears twice (second time in {})",
                    rec.0,
                    at()
                ));
            }
        }
    }
    if benches.is_empty() {
        return Err(format!("run {label}: no results"));
    }
    let one = |set: BTreeSet<String>, what: &str| -> Result<String, String> {
        if set.len() == 1 {
            Ok(set.into_iter().next().unwrap_or_default())
        } else {
            let list: Vec<String> = set.into_iter().collect();
            Err(format!("run {label} mixes {what}: {}", list.join(", ")))
        }
    };
    Ok(Run {
        label: label.to_owned(),
        git_sha: one(shas, "commits")?,
        runner: one(runners, "runners")?,
        benches,
    })
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_jsonl(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// Reads every `*.jsonl` file under `dir` (recursively) as one run.
pub(crate) fn load_run(dir: &Path) -> Result<Run, String> {
    let label = dir.display().to_string();
    let mut paths = Vec::new();
    collect_jsonl(dir, &mut paths).map_err(|e| format!("{label}: {e}"))?;
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{label}: no *.jsonl result files"));
    }
    let mut files = Vec::with_capacity(paths.len());
    for p in paths {
        let text = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        files.push((p.display().to_string(), text));
    }
    parse_run(&label, &files)
}

/// Requires every run to share one value of `key`, naming each run's value otherwise.
fn same_across(runs: &[Run], what: &str, key: fn(&Run) -> &str) -> Result<String, String> {
    let first = runs.first().map(key).unwrap_or_default();
    if runs.iter().all(|r| key(r) == first) {
        return Ok(first.to_owned());
    }
    let list: Vec<String> = runs
        .iter()
        .map(|r| format!("{} ({})", r.label, key(r)))
        .collect();
    Err(format!("runs mix {what}: {}", list.join(", ")))
}

/// Builds the baseline from several runs of one commit on one runner.
pub(crate) fn build_baseline(runs: &[Run]) -> Result<Vec<BaselineEntry>, String> {
    if runs.len() < 2 {
        return Err("a baseline needs at least two runs to measure spread".to_owned());
    }
    let git_sha = same_across(runs, "commits", |r| &r.git_sha)?;
    let runner = same_across(runs, "runners", |r| &r.runner)?;
    if git_sha == "unknown" {
        return Err(
            "the runs do not name their commit (git_sha is `unknown`); set GITHUB_SHA or IAMB_GIT_SHA"
                .to_owned(),
        );
    }
    let all: BTreeSet<&String> = runs.iter().flat_map(|r| r.benches.keys()).collect();
    let mut missing = Vec::new();
    for bench in &all {
        let absent: Vec<&str> = runs
            .iter()
            .filter(|r| !r.benches.contains_key(*bench))
            .map(|r| r.label.as_str())
            .collect();
        if !absent.is_empty() {
            missing.push(format!("`{bench}` is missing from {}", absent.join(", ")));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "every bench must be in every run: {}",
            missing.join("; ")
        ));
    }
    let mut out = Vec::with_capacity(all.len());
    for bench in all {
        let ms: Vec<&Measurement> = runs.iter().filter_map(|r| r.benches.get(bench)).collect();
        let stat = ms[0].gated_stat().to_owned();
        let unit = ms[0].unit.clone();
        if let Some(m) = ms.iter().find(|m| m.gated_stat() != stat || m.unit != unit) {
            return Err(format!(
                "`{bench}`: runs disagree on the gated statistic or unit ({stat} {unit} vs {} {})",
                m.gated_stat(),
                m.unit
            ));
        }
        let mut values: Vec<u64> = ms.iter().filter_map(|m| m.stat(&stat)).collect();
        values.sort_unstable();
        out.push(BaselineEntry {
            bench: bench.clone(),
            baseline: median(&values).unwrap_or_default(),
            spread: spread(&values),
            stat,
            unit,
            runs: runs.len(),
            git_sha: git_sha.clone(),
            runner: runner.clone(),
        });
    }
    Ok(out)
}

/// Parses a baseline file; all entries must come from one runner.
pub(crate) fn parse_baseline(name: &str, text: &str) -> Result<Vec<BaselineEntry>, String> {
    let mut out: Vec<BaselineEntry> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = format!("{name} line {}", i + 1);
        let entry = parse_flat_object(line)
            .and_then(|o| BaselineEntry::from_object(&o))
            .map_err(|e| format!("{at}: {e}"))?;
        if out.iter().any(|e| e.bench == entry.bench) {
            return Err(format!("{at}: bench `{}` appears twice", entry.bench));
        }
        if let Some(first) = out.first()
            && first.runner != entry.runner
        {
            return Err(format!(
                "{at}: baseline mixes runners {} and {}",
                first.runner, entry.runner
            ));
        }
        out.push(entry);
    }
    if out.is_empty() {
        return Err(format!("{name}: empty baseline"));
    }
    Ok(out)
}

/// How one bench fared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Not a regression in either run.
    Ok,
    /// Regressed in this run (1 or 2) only: passes.
    OneRun(u8),
    /// Regressed in both runs: fails the gate.
    Regression,
    /// Not in the baseline: reported, never gated.
    New,
}

/// One row of the comparison table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) bench: String,
    pub(crate) stat: String,
    pub(crate) baseline: Option<u64>,
    pub(crate) floor: Option<u64>,
    pub(crate) run1: Option<u64>,
    pub(crate) run2: Option<u64>,
    pub(crate) verdict: Verdict,
}

/// Result of comparing two runs against a baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) baseline_sha: String,
    pub(crate) baseline_runs: usize,
    pub(crate) run_sha: String,
    pub(crate) rows: Vec<Row>,
}

impl Report {
    pub(crate) fn regressions(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.verdict == Verdict::Regression)
            .count()
    }
}

/// Compares two consecutive runs of one commit against `base`.
pub(crate) fn compare(base: &[BaselineEntry], r1: &Run, r2: &Run) -> Result<Report, String> {
    let pair = [r1.clone(), r2.clone()];
    let run_sha = same_across(&pair, "commits", |r| &r.git_sha)?;
    let runner = same_across(&pair, "runners", |r| &r.runner)?;
    let base_runner = base.first().map_or("", |e| e.runner.as_str());
    if runner != base_runner {
        return Err(format!(
            "the runs are from runner `{runner}` but the baseline is for `{base_runner}`"
        ));
    }
    let mut missing = Vec::new();
    for e in base {
        let absent: Vec<&str> = [r1, r2]
            .into_iter()
            .filter(|r| !r.benches.contains_key(&e.bench))
            .map(|r| r.label.as_str())
            .collect();
        if !absent.is_empty() {
            missing.push(format!(
                "`{}` is missing from {}",
                e.bench,
                absent.join(", ")
            ));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "every baseline bench must be in both runs: {}",
            missing.join("; ")
        ));
    }
    let mut rows = Vec::new();
    for e in base {
        let (m1, m2) = (&r1.benches[&e.bench], &r2.benches[&e.bench]);
        if m1.unit != e.unit || m2.unit != e.unit {
            return Err(format!(
                "`{}`: unit differs from the baseline's `{}`",
                e.bench, e.unit
            ));
        }
        let (v1, v2) = (m1.stat(&e.stat), m2.stat(&e.stat));
        let hit = |v: Option<u64>| v.is_some_and(|v| regresses(e.baseline, e.spread, v));
        let verdict = match (hit(v1), hit(v2)) {
            (true, true) => Verdict::Regression,
            (true, false) => Verdict::OneRun(1),
            (false, true) => Verdict::OneRun(2),
            (false, false) => Verdict::Ok,
        };
        rows.push(Row {
            bench: e.bench.clone(),
            stat: e.stat.clone(),
            baseline: Some(e.baseline),
            floor: Some(floor(e.spread)),
            run1: v1,
            run2: v2,
            verdict,
        });
    }
    let known: BTreeSet<&str> = base.iter().map(|e| e.bench.as_str()).collect();
    let new: BTreeSet<&String> = r1
        .benches
        .keys()
        .chain(r2.benches.keys())
        .filter(|b| !known.contains(b.as_str()))
        .collect();
    for bench in new {
        let (m1, m2) = (r1.benches.get(bench), r2.benches.get(bench));
        let stat = m1.or(m2).map_or(DEFAULT_STAT, Measurement::gated_stat);
        rows.push(Row {
            bench: bench.clone(),
            stat: stat.to_owned(),
            baseline: None,
            floor: None,
            run1: m1.and_then(|m| m.stat(stat)),
            run2: m2.and_then(|m| m.stat(stat)),
            verdict: Verdict::New,
        });
    }
    Ok(Report {
        baseline_sha: base.first().map(|e| e.git_sha.clone()).unwrap_or_default(),
        baseline_runs: base.first().map_or(0, |e| e.runs),
        run_sha,
        rows,
    })
}

/// Formats nanoseconds for a table: `850 ns`, `12.34 µs`, `4.56 ms`.
pub(crate) fn fmt_ns(ns: u64) -> String {
    if ns < 1_000 {
        format!("{ns} ns")
    } else if ns < 1_000_000 {
        format!("{}.{:02} µs", ns / 1_000, ns % 1_000 / 10)
    } else {
        format!("{}.{:02} ms", ns / 1_000_000, ns % 1_000_000 / 10_000)
    }
}

/// Change from `base` to `value` as a signed percentage with one decimal, e.g. `+31.2%`.
pub(crate) fn fmt_change(base: u64, value: u64) -> String {
    if base == 0 {
        return "n/a".to_owned();
    }
    let permille = (i128::from(value) - i128::from(base)) * 1000 / i128::from(base);
    let sign = if permille < 0 { '-' } else { '+' };
    let a = permille.abs();
    format!("{sign}{}.{}%", a / 10, a % 10)
}

fn opt_ns(v: Option<u64>) -> String {
    v.map_or_else(|| "missing".to_owned(), fmt_ns)
}

/// Renders the report as a Markdown table plus a one-line result.
pub(crate) fn render(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Baseline: {} runs of `{}`. Runs compared: `{}`.\n",
        report.baseline_runs, report.baseline_sha, report.run_sha
    );
    out.push_str("| bench | baseline | run 1 | run 2 | change | floor | verdict |\n");
    out.push_str("| --- | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for r in &report.rows {
        let change = match r.baseline {
            Some(b) => [r.run1, r.run2]
                .iter()
                .map(|v| v.map_or_else(|| "n/a".to_owned(), |v| fmt_change(b, v)))
                .collect::<Vec<_>>()
                .join(" / "),
            None => "n/a".to_owned(),
        };
        let verdict = match r.verdict {
            Verdict::Ok => "ok".to_owned(),
            Verdict::OneRun(n) => format!("slower in run {n} only: passes"),
            Verdict::Regression => "**REGRESSION**".to_owned(),
            Verdict::New => "new, not gated".to_owned(),
        };
        let _ = writeln!(
            out,
            "| `{}` {} | {} | {} | {} | {} | {} | {} |",
            r.bench,
            r.stat,
            r.baseline.map_or_else(|| "none".to_owned(), fmt_ns),
            opt_ns(r.run1),
            opt_ns(r.run2),
            change,
            r.floor.map_or_else(|| "n/a".to_owned(), fmt_ns),
            verdict,
        );
    }
    let n = report.regressions();
    let _ = writeln!(
        out,
        "\n{}",
        if n == 0 {
            "Result: pass. No bench is more than 25% and more than its noise floor slower than \
             baseline in both runs."
                .to_owned()
        } else {
            format!(
                "Result: FAIL. {n} bench(es) more than 25% and more than the noise floor slower \
                 than baseline in both runs."
            )
        }
    );
    out
}

/// Outcome of `bench-compare`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// No baseline file yet: nothing is gated.
    NotArmed,
    Pass,
    Fail,
}

fn append_summary(summary: Option<&Path>, text: &str) -> Result<(), String> {
    let Some(path) = summary else {
        return Ok(());
    };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn emit(out: &mut dyn Write, text: &str) -> Result<(), String> {
    out.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

/// `bench-compare [--baseline <file>] <run-1-dir> <run-2-dir>`. Relative paths are taken from
/// the current directory; the default baseline is [`CI_BASELINE`] under `root`. Writes the table
/// to `out` and appends it to `summary` when given.
pub(crate) fn compare_cmd(
    args: &[String],
    root: &Path,
    summary: Option<&Path>,
    out: &mut dyn Write,
) -> Result<Gate, String> {
    let mut baseline = root.join(CI_BASELINE);
    let mut dirs = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--baseline" => {
                baseline = PathBuf::from(it.next().ok_or_else(|| COMPARE_USAGE.to_owned())?);
            }
            s if s.starts_with('-') => return Err(COMPARE_USAGE.to_owned()),
            s => dirs.push(PathBuf::from(s)),
        }
    }
    let [d1, d2] = dirs.as_slice() else {
        return Err(COMPARE_USAGE.to_owned());
    };
    let title = "### Benchmark regression gate\n\n";
    if !baseline.exists() {
        let msg = format!(
            "{title}The regression gate is not armed yet: no baseline at `{}`. Nothing is \
             gated; see docs/test-strategy.md §3 to create one.\n",
            baseline.display()
        );
        emit(out, &msg)?;
        append_summary(summary, &msg)?;
        return Ok(Gate::NotArmed);
    }
    let result = (|| {
        let name = baseline.display().to_string();
        let text = fs::read_to_string(&baseline).map_err(|e| format!("{name}: {e}"))?;
        let base = parse_baseline(&name, &text)?;
        compare(&base, &load_run(d1)?, &load_run(d2)?)
    })();
    match result {
        Ok(report) => {
            let text = format!("{title}{}", render(&report));
            emit(out, &text)?;
            append_summary(summary, &text)?;
            Ok(if report.regressions() == 0 {
                Gate::Pass
            } else {
                Gate::Fail
            })
        }
        Err(e) => {
            append_summary(summary, &format!("{title}Error: {e}\n"))?;
            Err(e)
        }
    }
}

/// `bench-baseline [--out <file>] <run-dir>...`. Writes the baseline to `--out`, else to
/// `crates/iamb-bench/baselines/<runner>.jsonl` under `root`, and prints what it wrote.
pub(crate) fn baseline_cmd(
    args: &[String],
    root: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut dest = None;
    let mut dirs = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                dest = Some(PathBuf::from(
                    it.next().ok_or_else(|| BASELINE_USAGE.to_owned())?,
                ));
            }
            s if s.starts_with('-') => return Err(BASELINE_USAGE.to_owned()),
            s => dirs.push(PathBuf::from(s)),
        }
    }
    if dirs.is_empty() {
        return Err(BASELINE_USAGE.to_owned());
    }
    let runs = dirs
        .iter()
        .map(|d| load_run(d))
        .collect::<Result<Vec<_>, _>>()?;
    let entries = build_baseline(&runs)?;
    let dest = dest.unwrap_or_else(|| {
        root.join("crates/iamb-bench/baselines")
            .join(format!("{}.jsonl", entries[0].runner))
    });
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut text = String::new();
    for e in &entries {
        text.push_str(&e.to_line());
        text.push('\n');
    }
    fs::write(&dest, text).map_err(|e| format!("{}: {e}", dest.display()))?;

    let mut msg = String::new();
    let _ = writeln!(
        msg,
        "| bench | stat | baseline (median) | spread (max - min) | floor |\n| --- | --- | ---: | ---: | ---: |"
    );
    for e in &entries {
        let _ = writeln!(
            msg,
            "| `{}` | {} | {} | {} | {} |",
            e.bench,
            e.stat,
            fmt_ns(e.baseline),
            fmt_ns(e.spread),
            fmt_ns(floor(e.spread))
        );
    }
    let _ = writeln!(
        msg,
        "\nwrote {} benches from {} runs of {} on {} to {}",
        entries.len(),
        runs.len(),
        entries[0].git_sha,
        entries[0].runner,
        dest.display()
    );
    if runs.len() < RECOMMENDED_RUNS {
        let _ = writeln!(
            msg,
            "note: docs/test-strategy.md asks for {RECOMMENDED_RUNS} runs; this baseline has {}",
            runs.len()
        );
    }
    emit(out, &msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(stat: Option<&str>, v: u64) -> Measurement {
        Measurement {
            unit: "ns".to_owned(),
            target_stat: stat.map(str::to_owned),
            p50: v,
            p95: v,
            p99: v,
            max: v,
        }
    }

    fn run(label: &str, sha: &str, benches: &[(&str, u64)]) -> Run {
        Run {
            label: label.to_owned(),
            git_sha: sha.to_owned(),
            runner: "ci-ubuntu".to_owned(),
            benches: benches
                .iter()
                .map(|&(b, v)| (b.to_owned(), m(Some("p99"), v)))
                .collect(),
        }
    }

    fn entry(bench: &str, baseline: u64, spread: u64) -> BaselineEntry {
        BaselineEntry {
            bench: bench.to_owned(),
            stat: "p99".to_owned(),
            unit: "ns".to_owned(),
            baseline,
            spread,
            runs: 10,
            git_sha: "base".to_owned(),
            runner: "ci-ubuntu".to_owned(),
        }
    }

    fn line(
        sha: &str,
        runner: &str,
        bench: &str,
        stat: Option<&str>,
        p50: u64,
        p99: u64,
    ) -> String {
        format!(
            "{{\"schema\":1,\"run_id\":\"1-x\",\"timestamp_ms\":1,\"git_sha\":\"{sha}\",\
             \"os\":\"linux\",\"arch\":\"x86_64\",\"runner\":\"{runner}\",\"bench\":\"{bench}\",\
             \"kpi\":null,\"unit\":\"ns\",\"count\":3,\"min\":1,\"p50\":{p50},\"p95\":{p99},\
             \"p99\":{p99},\"max\":{p99},\"mean\":{p50},\"target_stat\":{},\"target_ns\":null,\
             \"pass\":null,\"note\":\"a \\\"note\\\"\"}}",
            stat.map_or_else(|| "null".to_owned(), |s| format!("\"{s}\""))
        )
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let d =
            std::env::temp_dir().join(format!("xtask-bench-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&d).expect("temp dir");
        d
    }

    #[test]
    fn median_and_spread() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[7]), Some(7));
        assert_eq!(median(&[1, 2, 9]), Some(2));
        assert_eq!(median(&[1, 2, 4, 9]), Some(3));
        assert_eq!(median(&[1, 2, 5, 9]), Some(3), "even count rounds down");
        assert_eq!(spread(&[]), 0);
        assert_eq!(spread(&[5, 3, 9, 4]), 6);

        let runs: Vec<Run> = [100, 130, 90, 110, 120]
            .iter()
            .enumerate()
            .map(|(i, &v)| run(&format!("r{i}"), "abc", &[("a/b/1", v)]))
            .collect();
        let base = build_baseline(&runs).expect("valid");
        assert_eq!(base.len(), 1);
        assert_eq!(
            (base[0].baseline, base[0].spread, base[0].runs),
            (110, 40, 5)
        );
        assert_eq!(
            (base[0].stat.as_str(), base[0].git_sha.as_str()),
            ("p99", "abc")
        );
    }

    #[test]
    fn gated_stat_defaults_to_p50() {
        let mk = |label: &str, p50: u64| Run {
            benches: [(
                "a/b/1".to_owned(),
                Measurement {
                    p99: 999,
                    ..m(None, p50)
                },
            )]
            .into(),
            ..run(label, "abc", &[])
        };
        let base = build_baseline(&[mk("r1", 10), mk("r2", 20), mk("r3", 30)]).expect("valid");
        assert_eq!(
            (base[0].stat.as_str(), base[0].baseline, base[0].spread),
            ("p50", 20, 20)
        );
    }

    #[test]
    fn floor_is_at_least_one_microsecond() {
        assert_eq!(floor(0), 1_000);
        assert_eq!(floor(499), 1_000);
        assert_eq!(floor(500), 1_000);
        assert_eq!(floor(501), 1_002);
        assert_eq!(floor(10_000), 20_000);
        // 100 ns baseline, 10x slower but only 900 ns more: under the 1 µs floor.
        assert!(!regresses(100, 0, 1_000));
        assert!(regresses(100, 0, 1_101));
    }

    #[test]
    fn regression_needs_both_conditions() {
        // Exactly 25% worse is not "more than 25%".
        assert!(!regresses(100_000, 0, 125_000));
        assert!(regresses(100_000, 0, 125_001));
        // Over the floor but under 25%.
        assert!(!regresses(100_000, 0, 120_000));
    }

    #[test]
    fn regression_in_one_run_only_passes() {
        let base = [entry("a/b/1", 100_000, 1_000)];
        let r1 = run("r1", "pr", &[("a/b/1", 140_000)]);
        let r2 = run("r2", "pr", &[("a/b/1", 101_000)]);
        let rep = compare(&base, &r1, &r2).expect("valid");
        assert_eq!(rep.rows[0].verdict, Verdict::OneRun(1));
        assert_eq!(rep.regressions(), 0);
        let rep = compare(&base, &r2, &r1).expect("valid");
        assert_eq!(rep.rows[0].verdict, Verdict::OneRun(2));
        assert_eq!(rep.regressions(), 0);
    }

    #[test]
    fn regression_in_both_runs_fails() {
        let base = [
            entry("a/b/1", 100_000, 1_000),
            entry("a/c/1", 100_000, 1_000),
        ];
        let r1 = run("r1", "pr", &[("a/b/1", 140_000), ("a/c/1", 100_500)]);
        let r2 = run("r2", "pr", &[("a/b/1", 130_000), ("a/c/1", 99_000)]);
        let rep = compare(&base, &r1, &r2).expect("valid");
        assert_eq!(rep.rows[0].verdict, Verdict::Regression);
        assert_eq!(rep.rows[1].verdict, Verdict::Ok);
        assert_eq!(rep.regressions(), 1);
        let table = render(&rep);
        assert!(table.contains("| `a/b/1` p99 | 100.00 µs | 140.00 µs | 130.00 µs | +40.0% / +30.0% | 2.00 µs | **REGRESSION** |"), "{table}");
        assert!(table.contains("Result: FAIL"));
    }

    #[test]
    fn slowdown_of_30_percent_under_the_floor_passes() {
        // Spread 2 µs -> floor 4 µs. 10 µs -> 13 µs is +30% but only 3 µs.
        let base = [entry("a/b/1", 10_000, 2_000)];
        let r = run("r", "pr", &[("a/b/1", 13_000)]);
        let rep = compare(&base, &r, &r).expect("valid");
        assert_eq!(rep.rows[0].verdict, Verdict::Ok);
        assert_eq!(rep.rows[0].floor, Some(4_000));
        // The same slowdown at 100 µs is over the floor and fails.
        let base = [entry("a/b/1", 100_000, 2_000)];
        let r = run("r", "pr", &[("a/b/1", 130_000)]);
        assert_eq!(compare(&base, &r, &r).expect("valid").regressions(), 1);
    }

    #[test]
    fn new_bench_is_reported_and_passes() {
        let base = [entry("a/b/1", 100_000, 0)];
        let r1 = run("r1", "pr", &[("a/b/1", 100_000), ("a/new/1", 9_000_000)]);
        let r2 = run("r2", "pr", &[("a/b/1", 100_000)]);
        let rep = compare(&base, &r1, &r2).expect("valid");
        assert_eq!(rep.regressions(), 0);
        let new = &rep.rows[1];
        assert_eq!((new.bench.as_str(), new.verdict), ("a/new/1", Verdict::New));
        assert_eq!((new.run1, new.run2), (Some(9_000_000), None));
        assert!(
            render(&rep).contains(
                "| `a/new/1` p99 | none | 9.00 ms | missing | n/a | n/a | new, not gated |"
            )
        );
    }

    #[test]
    fn missing_bench_is_an_error() {
        let base = [entry("a/b/1", 100_000, 0), entry("a/c/1", 100_000, 0)];
        let r1 = run("r1", "pr", &[("a/b/1", 100_000), ("a/c/1", 100_000)]);
        let r2 = run("r2", "pr", &[("a/b/1", 100_000)]);
        let err = compare(&base, &r1, &r2).expect_err("missing bench");
        assert!(err.contains("`a/c/1` is missing from r2"), "{err}");

        let runs = [
            run("r1", "abc", &[("a/b/1", 1), ("a/c/1", 1)]),
            run("r2", "abc", &[("a/b/1", 1)]),
            run("r3", "abc", &[("a/b/1", 1)]),
        ];
        let err = build_baseline(&runs).expect_err("missing bench");
        assert!(err.contains("`a/c/1` is missing from r2, r3"), "{err}");
    }

    #[test]
    fn mixed_commits_are_refused() {
        let runs = [
            run("r1", "abc", &[("a/b/1", 1)]),
            run("r2", "def", &[("a/b/1", 1)]),
        ];
        let err = build_baseline(&runs).expect_err("mixed commits");
        assert!(err.contains("mix commits: r1 (abc), r2 (def)"), "{err}");

        let mut other = run("r2", "abc", &[("a/b/1", 1)]);
        other.runner = "local".to_owned();
        let err = build_baseline(&[run("r1", "abc", &[("a/b/1", 1)]), other]).expect_err("runners");
        assert!(err.contains("mix runners"), "{err}");

        let files = [
            (
                "a.jsonl".to_owned(),
                line("abc", "ci-ubuntu", "a/b/1", None, 1, 2),
            ),
            (
                "b.jsonl".to_owned(),
                line("def", "ci-ubuntu", "a/c/1", None, 1, 2),
            ),
        ];
        let err = parse_run("dir", &files).expect_err("mixed within one run");
        assert!(err.contains("run dir mixes commits: abc, def"), "{err}");

        let base = [entry("a/b/1", 1, 0)];
        let err = compare(
            &base,
            &run("r1", "x", &[("a/b/1", 1)]),
            &run("r2", "y", &[("a/b/1", 1)]),
        )
        .expect_err("runs of two commits");
        assert!(err.contains("mix commits"), "{err}");
    }

    #[test]
    fn missing_baseline_is_not_armed() {
        let dir = temp_dir("unarmed");
        let summary = dir.join("summary.md");
        let args = [
            "--baseline".to_owned(),
            dir.join("none.jsonl").display().to_string(),
            "run-1".to_owned(),
            "run-2".to_owned(),
        ];
        let mut out = Vec::new();
        let gate = compare_cmd(&args, &dir, Some(&summary), &mut out).expect("not an error");
        assert_eq!(gate, Gate::NotArmed);
        let text = String::from_utf8(out).expect("utf-8");
        assert!(text.contains("not armed yet"), "{text}");
        assert_eq!(fs::read_to_string(&summary).expect("summary written"), text);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn end_to_end_baseline_then_compare() {
        let dir = temp_dir("e2e");
        let write_run = |name: &str, sha: &str, lookup: u64| {
            let d = dir.join(name);
            fs::create_dir_all(d.join("nested")).expect("dir");
            fs::write(
                d.join("1-matcher.jsonl"),
                line(
                    sha,
                    "ci-ubuntu",
                    "matcher/lookup/10000",
                    Some("p99"),
                    1,
                    lookup,
                ) + "\n",
            )
            .expect("write");
            fs::write(
                d.join("nested/2-store.jsonl"),
                line(sha, "ci-ubuntu", "store/load/10000", None, 5_000_000, 9) + "\n",
            )
            .expect("write");
            fs::write(d.join("ignored.txt"), "not json").expect("write");
            d.display().to_string()
        };
        let base_dirs: Vec<String> = [2_000, 2_100, 1_900]
            .iter()
            .enumerate()
            .map(|(i, &v)| write_run(&format!("base-{i}"), "main", v))
            .collect();
        let baseline = dir.join("baselines/ci-ubuntu.jsonl");
        let mut args = vec!["--out".to_owned(), baseline.display().to_string()];
        args.extend(base_dirs);
        let mut out = Vec::new();
        baseline_cmd(&args, &dir, &mut out).expect("baseline written");
        let text = fs::read_to_string(&baseline).expect("baseline readable");
        let entries = parse_baseline("b", &text).expect("baseline parses");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            text.lines().next().expect("one line"),
            "{\"schema\":1,\"bench\":\"matcher/lookup/10000\",\"stat\":\"p99\",\"unit\":\"ns\",\
             \"baseline\":2000,\"spread\":200,\"runs\":3,\"git_sha\":\"main\",\"runner\":\"ci-ubuntu\"}"
        );

        let ok = |v| {
            vec![
                "--baseline".to_owned(),
                baseline.display().to_string(),
                write_run("pr-1", "pr", v),
                write_run("pr-2", "pr", v),
            ]
        };
        let mut out = Vec::new();
        assert_eq!(
            compare_cmd(&ok(2_500), &dir, None, &mut out),
            Ok(Gate::Pass)
        );
        let mut out = Vec::new();
        assert_eq!(
            compare_cmd(&ok(3_500), &dir, None, &mut out),
            Ok(Gate::Fail)
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
