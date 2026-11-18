//! Result files: one JSON-lines file per run under `target/bench-results/`.
//!
//! Each line is one measurement (see `docs/test-strategy.md`, "Result format"). The JSON is
//! written by hand so the harness needs no serialisation dependency.

use crate::latency::LatencySummary;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Version of the line format. Bump when a field changes meaning.
pub const SCHEMA_VERSION: u32 = 1;

/// One measurement, written as one JSON line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRecord {
    /// Benchmark id, `<area>/<operation>/<size>`, e.g. `matcher/lookup/10000`.
    pub bench: String,
    /// KPI or component-target id this measures, e.g. `matcher-lookup-p99`; `None` for
    /// placeholders and diagnostics.
    pub kpi: Option<String>,
    /// Summary of the samples, in nanoseconds.
    pub summary: LatencySummary,
    /// Pass threshold in nanoseconds for the statistic named by `target_stat`, if any.
    pub target_ns: Option<u64>,
    /// Which statistic the target applies to: `p50`, `p95`, `p99` or `max`.
    pub target_stat: Option<&'static str>,
    /// Free-form note, e.g. "includes ~20 ns timer overhead per sample".
    pub note: Option<String>,
}

impl ResultRecord {
    /// Creates a record with no KPI, target or note.
    #[must_use]
    pub fn new(bench: impl Into<String>, summary: LatencySummary) -> Self {
        Self {
            bench: bench.into(),
            kpi: None,
            summary,
            target_ns: None,
            target_stat: None,
            note: None,
        }
    }

    /// Value of the statistic named by `target_stat`, if both are set.
    #[must_use]
    pub fn target_value(&self) -> Option<u64> {
        let s = &self.summary;
        match self.target_stat? {
            "p50" => Some(s.p50),
            "p95" => Some(s.p95),
            "p99" => Some(s.p99),
            "max" => Some(s.max),
            _ => None,
        }
    }

    /// Renders the record as one JSON object (no trailing newline).
    #[must_use]
    pub fn to_json_line(&self, run: &RunMeta) -> String {
        let s = &self.summary;
        let mut out = String::with_capacity(320);
        let _ = write!(
            out,
            "{{\"schema\":{SCHEMA_VERSION},\"run_id\":{},\"timestamp_ms\":{},\"git_sha\":{},\
             \"os\":{},\"arch\":{},\"runner\":{},\"bench\":{},\"kpi\":{},\"unit\":\"ns\",\
             \"count\":{},\"min\":{},\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{},\"mean\":{},\
             \"target_stat\":{},\"target_ns\":{},\"pass\":{},\"note\":{}}}",
            json_str(&run.run_id),
            run.timestamp_ms,
            json_str(&run.git_sha),
            json_str(std::env::consts::OS),
            json_str(std::env::consts::ARCH),
            json_str(&run.runner),
            json_str(&self.bench),
            json_opt_str(self.kpi.as_deref()),
            s.count,
            s.min,
            s.p50,
            s.p95,
            s.p99,
            s.max,
            s.mean,
            json_opt_str(self.target_stat),
            self.target_ns
                .map_or_else(|| "null".to_owned(), |t| t.to_string()),
            match (self.target_value(), self.target_ns) {
                (Some(v), Some(t)) => (v < t).to_string(),
                _ => "null".to_owned(),
            },
            json_opt_str(self.note.as_deref()),
        );
        out
    }
}

/// Metadata shared by every line of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    /// Unique id of the run: `<timestamp_ms>-<bench file>`.
    pub run_id: String,
    /// Wall-clock start of the run, Unix milliseconds.
    pub timestamp_ms: u128,
    /// Commit measured: `GITHUB_SHA`, or `IAMB_GIT_SHA`, or `unknown`.
    pub git_sha: String,
    /// Where it ran: `IAMB_BENCH_RUNNER` (e.g. `ci-ubuntu`, `lab-win11-01`), or `local`.
    pub runner: String,
}

impl RunMeta {
    /// Builds run metadata for the bench file `name` from the clock and environment.
    #[must_use]
    pub fn from_env(name: &str) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            run_id: format!("{timestamp_ms}-{name}"),
            timestamp_ms,
            git_sha: env("GITHUB_SHA")
                .or_else(|| env("IAMB_GIT_SHA"))
                .unwrap_or_else(|| "unknown".to_owned()),
            runner: env("IAMB_BENCH_RUNNER").unwrap_or_else(|| "local".to_owned()),
        }
    }
}

/// Writes the result file of one run, plus optional raw sample files.
#[derive(Debug)]
pub struct RunWriter {
    meta: RunMeta,
    dir: PathBuf,
    lines: Vec<String>,
}

impl RunWriter {
    /// Starts a run named after the bench file (e.g. `sample`), writing into `dir`.
    #[must_use]
    pub fn new(name: &str, dir: PathBuf) -> Self {
        Self {
            meta: RunMeta::from_env(name),
            dir,
            lines: Vec::new(),
        }
    }

    /// Starts a run writing into [`default_results_dir`].
    #[must_use]
    pub fn in_default_dir(name: &str) -> Self {
        Self::new(name, default_results_dir())
    }

    /// Run metadata.
    #[must_use]
    pub const fn meta(&self) -> &RunMeta {
        &self.meta
    }

    /// Adds one record to the run.
    pub fn push(&mut self, record: &ResultRecord) {
        self.lines.push(record.to_json_line(&self.meta));
    }

    /// Writes ascending samples, one integer per line, to
    /// `<dir>/<run_id>.samples/<bench with '/' replaced by '_'>.txt`, for re-analysis.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from creating the directory or writing the file.
    pub fn write_samples(&self, bench: &str, sorted: &[u64]) -> io::Result<PathBuf> {
        let dir = self.dir.join(format!("{}.samples", self.meta.run_id));
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.txt", bench.replace('/', "_")));
        let mut text = String::with_capacity(sorted.len() * 8);
        for v in sorted {
            let _ = writeln!(text, "{v}");
        }
        fs::write(&path, text)?;
        Ok(path)
    }

    /// Writes `<dir>/<run_id>.jsonl` and returns its path.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from creating the directory or writing the file.
    pub fn finish(self) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{}.jsonl", self.meta.run_id));
        let mut f = fs::File::create(&path)?;
        for line in &self.lines {
            writeln!(f, "{line}")?;
        }
        f.flush()?;
        Ok(path)
    }
}

/// Where result files go: `IAMB_BENCH_RESULTS_DIR`, else `$CARGO_TARGET_DIR/bench-results`,
/// else `<workspace>/target/bench-results`.
#[must_use]
pub fn default_results_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("IAMB_BENCH_RESULTS_DIR") {
        return PathBuf::from(d);
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            // crates/iamb-bench -> workspace root.
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            manifest
                .ancestors()
                .nth(2)
                .unwrap_or(manifest)
                .join("target")
        },
        PathBuf::from,
    );
    target.join("bench-results")
}

fn json_opt_str(s: Option<&str>) -> String {
    s.map_or_else(|| "null".to_owned(), json_str)
}

/// Quotes and escapes a string as a JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::{ResultRecord, RunMeta, RunWriter, SystemTime, UNIX_EPOCH, json_str};
    use crate::latency::summarize_sorted;

    fn meta() -> RunMeta {
        RunMeta {
            run_id: "1-sample".to_owned(),
            timestamp_ms: 1,
            git_sha: "abc".to_owned(),
            runner: "local".to_owned(),
        }
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }

    #[test]
    fn record_renders_expected_fields() {
        let summary = summarize_sorted(&[10, 20, 30]).expect("non-empty");
        let mut r = ResultRecord::new("placeholder/hashmap_lookup/10000", summary);
        r.target_stat = Some("p99");
        r.target_ns = Some(50_000);
        let line = r.to_json_line(&meta());
        assert!(line.starts_with("{\"schema\":1,\"run_id\":\"1-sample\""));
        assert!(line.contains("\"bench\":\"placeholder/hashmap_lookup/10000\""));
        assert!(line.contains("\"kpi\":null"));
        assert!(line.contains("\"p50\":20,\"p95\":30,\"p99\":30,\"max\":30,\"mean\":20"));
        assert!(line.contains("\"target_stat\":\"p99\",\"target_ns\":50000,\"pass\":true"));
        assert!(line.ends_with("\"note\":null}"));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn writer_creates_one_jsonl_file_per_run() {
        // Unique per run: a process id alone is reused, and a stale directory from a killed run
        // could hold files with the same run id.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("iamb-bench-test-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = RunWriter::new("unit", dir.clone());
        let summary = summarize_sorted(&[1, 2]).expect("non-empty");
        w.push(&ResultRecord::new("a/b/1", summary));
        w.push(&ResultRecord::new("a/c/1", summary));
        let samples = w.write_samples("a/b/1", &[1, 2]).expect("samples written");
        let path = w.finish().expect("file written");
        let text = std::fs::read_to_string(&path).expect("readable");
        assert_eq!(text.lines().count(), 2);
        assert_eq!(
            std::fs::read_to_string(samples).expect("readable"),
            "1\n2\n"
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
