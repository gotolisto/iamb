//! The results file: JSON lines, append-only, so a run can stop and resume.
//!
//! One record per line. A cell can have several records (a re-run); the latest one counts.
//! A cell's record never holds the field's text, except `observed` after a failure the tester
//! agreed to share.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::apps::Os;

/// How a cell was run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The harness typed and read back.
    Auto,
    /// The person typed and judged.
    Manual,
}

/// What an exception record resolves a failure with (matrix section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// A per-app override in the strategy table.
    Override,
    /// A documented limitation.
    Limitation,
    /// An upstream or Iamb issue.
    Issue,
}

impl Resolution {
    /// The word used in tables.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Limitation => "limitation",
            Self::Issue => "issue",
        }
    }
}

/// The machine and the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInfo {
    /// Unix seconds.
    pub ts: u64,
    /// The matrix.
    pub os: Os,
    /// OS name and build.
    pub os_build: String,
    /// Model, CPU, RAM.
    pub machine: String,
    /// Commit of the build under test.
    pub commit: String,
    /// The tester's role.
    pub tester: String,
    /// The local date the run started (`YYYY-MM-DD`).
    pub date: String,
}

/// The version of one app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    /// Unix seconds.
    pub ts: u64,
    /// The matrix.
    pub os: Os,
    /// `W1` and so on.
    pub app: String,
    /// The version as the person read it.
    pub version: String,
}

/// The result of one cell (an app and a snippet type).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRecord {
    /// Unix seconds.
    pub ts: u64,
    /// The matrix.
    pub os: Os,
    /// `W1` and so on.
    pub app: String,
    /// `T1` to `T5`.
    pub typ: String,
    /// How it was run.
    pub mode: Mode,
    /// The verdict.
    pub pass: bool,
    /// Repetitions done.
    pub reps: u32,
    /// Repetitions that met every criterion.
    pub reps_passed: u32,
    /// Secure rows only: nothing expanded, nothing was deleted or inserted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nothing_expanded: Option<bool>,
    /// T1 only, when it could be measured: the 95th percentile, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_p95_ms: Option<u64>,
    /// T1 only: what one undo did (`whole`, `partial`, `abbreviation`, `none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<String>,
    /// The tester's note or the harness's reason for a failure. Never field text.
    #[serde(default)]
    pub note: String,
    /// The field's text after a failure, only when the tester agreed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
}

/// The exception record of a failing cell (matrix section 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionRecord {
    /// Unix seconds.
    pub ts: u64,
    /// The matrix.
    pub os: Os,
    /// `E-<os>-<n>`.
    pub id: String,
    /// `W9/T2`.
    pub cell: String,
    /// What was seen.
    pub symptom: String,
    /// Why.
    pub cause: String,
    /// How it is resolved.
    pub resolution: Resolution,
    /// The issue link, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// The date found.
    pub found: String,
    /// The date the cell was re-tested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retested: Option<String>,
}

/// One line of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The start of a run.
    Run(RunInfo),
    /// An app's version.
    App(AppInfo),
    /// A cell.
    Cell(CellRecord),
    /// An exception.
    Exception(ExceptionRecord),
}

/// Unix seconds now.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The key of a cell: `W9/T2`.
pub fn cell_key(app: &str, typ: &str) -> String {
    format!("{app}/{typ}")
}

/// Appends one record as one line.
pub fn append(path: &Path, record: &Record) -> Result<(), String> {
    let mut line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Everything the file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResultSet {
    /// The records, oldest first.
    pub records: Vec<Record>,
}

impl ResultSet {
    /// Reads `path`; a missing file is an empty set. A last line cut short by a crash is
    /// ignored; any other bad line is an error naming it.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        Self::parse(&text)
    }

    /// Parses the text of a results file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut records = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<Record>(line) {
                Ok(r) => records.push(r),
                Err(_) if i + 1 == lines.len() && !text.ends_with('\n') => {}
                Err(e) => return Err(format!("results line {}: {e}", i + 1)),
            }
        }
        Ok(Self { records })
    }

    /// The latest record of a cell.
    pub fn cell(&self, os: Os, app: &str, typ: &str) -> Option<&CellRecord> {
        self.records.iter().rev().find_map(|r| match r {
            Record::Cell(c) if c.os == os && c.app == app && c.typ == typ => Some(c),
            _ => None,
        })
    }

    /// True when the cell has a record.
    pub fn done(&self, os: Os, app: &str, typ: &str) -> bool {
        self.cell(os, app, typ).is_some()
    }

    /// The latest version given for an app.
    pub fn app_version(&self, os: Os, app: &str) -> Option<&str> {
        self.records.iter().rev().find_map(|r| match r {
            Record::App(a) if a.os == os && a.app == app => Some(a.version.as_str()),
            _ => None,
        })
    }

    /// The latest run record of an OS.
    pub fn last_run(&self, os: Os) -> Option<&RunInfo> {
        self.records.iter().rev().find_map(|r| match r {
            Record::Run(run) if run.os == os => Some(run),
            _ => None,
        })
    }

    /// The exceptions of an OS, oldest first.
    pub fn exceptions(&self, os: Os) -> Vec<&ExceptionRecord> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Exception(e) if e.os == os => Some(e),
                _ => None,
            })
            .collect()
    }

    /// The next exception id for an OS: `E-win-1`, `E-mac-1`, ...
    pub fn next_exception_id(&self, os: Os) -> String {
        let tag = match os {
            Os::Windows => "win",
            Os::Macos => "mac",
        };
        format!("E-{tag}-{}", self.exceptions(os).len() + 1)
    }

    /// The operating systems the file has records for.
    pub fn oses(&self) -> Vec<Os> {
        let mut out = Vec::new();
        for r in &self.records {
            let os = match r {
                Record::Run(x) => x.os,
                Record::App(x) => x.os,
                Record::Cell(x) => x.os,
                Record::Exception(x) => x.os,
            };
            if !out.contains(&os) {
                out.push(os);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(app: &str, typ: &str, pass: bool, ts: u64) -> CellRecord {
        CellRecord {
            ts,
            os: Os::Windows,
            app: app.to_owned(),
            typ: typ.to_owned(),
            mode: Mode::Auto,
            pass,
            reps: 5,
            reps_passed: if pass { 5 } else { 3 },
            nothing_expanded: None,
            latency_p95_ms: None,
            undo: None,
            note: String::new(),
            observed: None,
        }
    }

    #[test]
    fn append_then_load_round_trips_and_resumes() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("r.jsonl");
        assert!(
            !ResultSet::load(&path)
                .unwrap_or_default()
                .done(Os::Windows, "W1", "T1")
        );
        append(&path, &Record::Cell(cell("W1", "T1", true, 1))).unwrap_or_else(|e| panic!("{e}"));
        append(&path, &Record::Cell(cell("W1", "T2", false, 2))).unwrap_or_else(|e| panic!("{e}"));
        let set = ResultSet::load(&path).unwrap_or_default();
        assert!(set.done(Os::Windows, "W1", "T1"));
        assert!(set.done(Os::Windows, "W1", "T2"));
        assert!(!set.done(Os::Windows, "W1", "T3"));
        assert!(!set.done(Os::Macos, "W1", "T1"));
        // A re-run appends; the latest record wins and the file only grows.
        append(&path, &Record::Cell(cell("W1", "T2", true, 3))).unwrap_or_else(|e| panic!("{e}"));
        let set = ResultSet::load(&path).unwrap_or_default();
        assert_eq!(set.records.len(), 3);
        assert_eq!(
            set.cell(Os::Windows, "W1", "T2").map(|c| c.pass),
            Some(true)
        );
    }

    #[test]
    fn a_line_cut_short_by_a_crash_is_ignored_but_a_bad_middle_line_is_not() {
        let good =
            serde_json::to_string(&Record::Cell(cell("W1", "T1", true, 1))).unwrap_or_default();
        let cut = format!("{good}\n{{\"kind\":\"cell\",\"os\":\"win");
        assert_eq!(ResultSet::parse(&cut).map(|s| s.records.len()), Ok(1));
        let bad = format!("{{nope}}\n{good}\n");
        assert!(ResultSet::parse(&bad).is_err());
    }

    #[test]
    fn exception_ids_count_per_os() {
        let mut set = ResultSet::default();
        assert_eq!(set.next_exception_id(Os::Windows), "E-win-1");
        set.records.push(Record::Exception(ExceptionRecord {
            ts: 1,
            os: Os::Windows,
            id: "E-win-1".to_owned(),
            cell: "W9/T2".to_owned(),
            symptom: "s".to_owned(),
            cause: "c".to_owned(),
            resolution: Resolution::Issue,
            issue: None,
            found: "2026-01-01".to_owned(),
            retested: None,
        }));
        assert_eq!(set.next_exception_id(Os::Windows), "E-win-2");
        assert_eq!(set.next_exception_id(Os::Macos), "E-mac-1");
    }

    #[test]
    fn a_cell_record_never_serialises_absent_text() {
        let line =
            serde_json::to_string(&Record::Cell(cell("W1", "T1", true, 1))).unwrap_or_default();
        assert!(!line.contains("observed"));
        assert!(line.contains("\"kind\":\"cell\""));
    }
}
