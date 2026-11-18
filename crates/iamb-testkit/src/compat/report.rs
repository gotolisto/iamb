//! The results table and the pass rate, as `docs/compatibility-matrix.md` sections 4 to 6 say.
//!
//! - 20 apps times 5 types is 100 cells per OS; the pass rate is passing cells over all cells.
//! - A secure row (password field) passes when nothing expands: the record must say
//!   `nothing_expanded: true`.
//! - A cell with no record is "not run" and does not pass.
//! - The KPI is met when the pass rate is at least 95% and every failing cell has an exception
//!   record. A run with cells not run is reported as not met: the KPI is about the whole matrix.
//! - A cell fixed by an override passes only through a re-run, which is a later record of the
//!   same cell; the latest record counts.

use std::fmt::Write as _;

use super::apps::{App, Os};
use super::results::{CellRecord, ExceptionRecord, ResultSet, RunInfo, cell_key};
use super::types::SnippetType;

/// The pass rate the KPI asks for, in percent.
pub const KPI_PERCENT: usize = 95;

/// The state of one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Passed.
    Pass,
    /// Failed.
    Fail,
    /// No record.
    NotRun,
}

impl Status {
    const fn letter(self) -> &'static str {
        match self {
            Self::Pass => "P",
            Self::Fail => "F",
            Self::NotRun => "-",
        }
    }
}

/// Whether a cell record counts as a pass for `app`.
pub fn passes(app: &App, record: &CellRecord) -> bool {
    if app.secure {
        record.pass && record.nothing_expanded == Some(true)
    } else {
        record.pass
    }
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The app.
    pub app: App,
    /// Its version, if given.
    pub version: Option<String>,
    /// One status per type, in order.
    pub cells: Vec<Status>,
    /// T1's latency, if measured.
    pub latency_p95_ms: Option<u64>,
    /// T1's undo result.
    pub undo: Option<String>,
    /// Exception ids of this app's cells.
    pub exceptions: Vec<String>,
}

/// The computed result for one OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The matrix.
    pub os: Os,
    /// The run's header, if the file has one.
    pub run: Option<RunInfo>,
    /// The type ids, in column order.
    pub type_ids: Vec<String>,
    /// One row per app.
    pub rows: Vec<Row>,
    /// Cells that passed.
    pub passed: usize,
    /// All cells (apps times types).
    pub total: usize,
    /// Cells that failed, as `W9/T2`.
    pub failures: Vec<String>,
    /// Failed cells with no exception record.
    pub unresolved: Vec<String>,
    /// Cells with no record.
    pub not_run: Vec<String>,
    /// The exception records.
    pub exceptions: Vec<ExceptionRecord>,
}

impl Report {
    /// The pass rate in whole percent, rounded down.
    pub fn percent(&self) -> usize {
        (self.passed * 100).checked_div(self.total).unwrap_or(0)
    }

    /// True when the pass rate reaches the KPI.
    pub fn rate_met(&self) -> bool {
        self.total > 0 && self.passed * 100 >= KPI_PERCENT * self.total
    }

    /// True when the KPI is met: the rate, every failure resolved, nothing left unrun.
    pub fn kpi_met(&self) -> bool {
        self.rate_met() && self.unresolved.is_empty() && self.not_run.is_empty()
    }
}

/// Computes the report of `os` from `set`.
pub fn compute(os: Os, apps: &[App], types: &[SnippetType], set: &ResultSet) -> Report {
    let exceptions: Vec<ExceptionRecord> = set.exceptions(os).into_iter().cloned().collect();
    let mut rows = Vec::new();
    let (mut passed, mut failures, mut not_run) = (0, Vec::new(), Vec::new());
    for app in apps.iter().filter(|a| a.os == os) {
        let mut cells = Vec::new();
        for t in types {
            let key = cell_key(&app.id, &t.id);
            let status = match set.cell(os, &app.id, &t.id) {
                None => {
                    not_run.push(key);
                    Status::NotRun
                }
                Some(rec) if passes(app, rec) => {
                    passed += 1;
                    Status::Pass
                }
                Some(_) => {
                    failures.push(key);
                    Status::Fail
                }
            };
            cells.push(status);
        }
        let first = types.first().map(|t| t.id.as_str()).unwrap_or_default();
        let t1 = set.cell(os, &app.id, first);
        let prefix = format!("{}/", app.id);
        let mut ids: Vec<String> = Vec::new();
        for e in &exceptions {
            if e.cell.starts_with(&prefix) && !ids.contains(&e.id) {
                ids.push(e.id.clone());
            }
        }
        rows.push(Row {
            app: app.clone(),
            version: set.app_version(os, &app.id).map(str::to_owned),
            cells,
            latency_p95_ms: t1.and_then(|c| c.latency_p95_ms),
            undo: t1.and_then(|c| c.undo.clone()),
            exceptions: ids,
        });
    }
    let unresolved = failures
        .iter()
        .filter(|key| !exceptions.iter().any(|e| &e.cell == *key))
        .cloned()
        .collect();
    Report {
        os,
        run: set.last_run(os).cloned(),
        type_ids: types.iter().map(|t| t.id.clone()).collect(),
        total: rows.len() * types.len(),
        rows,
        passed,
        failures,
        unresolved,
        not_run,
        exceptions,
    }
}

fn cell_text(s: &str) -> String {
    s.replace('|', "/").replace('\n', " ")
}

/// The Markdown of the report: the header line, the table and the exceptions, in the template
/// of the matrix section 6, then what is still open.
#[expect(clippy::too_many_lines, reason = "one linear template")]
pub fn render_markdown(r: &Report) -> String {
    let mut out = String::new();
    let field = |v: Option<&str>| match v {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => "?".to_owned(),
    };
    let run = r.run.as_ref();
    let _ = writeln!(
        out,
        "Run: {} · OS: {} · Machine: {} · Commit: {} · Tester: {}\n",
        field(run.map(|x| x.date.as_str())),
        field(run.map(|x| x.os_build.as_str())),
        field(run.map(|x| x.machine.as_str())),
        field(run.map(|x| x.commit.as_str())),
        field(run.map(|x| x.tester.as_str())),
    );
    let _ = write!(out, "| #  | App (version) |");
    for id in &r.type_ids {
        let _ = write!(out, " {id} |");
    }
    let _ = writeln!(out, " T1 p95 ms | Undo (T1) | Exception |");
    let _ = write!(out, "| -- | ------------- |");
    for _ in &r.type_ids {
        let _ = write!(out, " -- |");
    }
    let _ = writeln!(out, " --------- | --------- | --------- |");
    for row in &r.rows {
        let name = match &row.version {
            Some(v) => format!("{} ({v})", row.app.name),
            None => row.app.name.clone(),
        };
        let _ = write!(out, "| {} | {} |", row.app.id, cell_text(&name));
        for c in &row.cells {
            let _ = write!(out, " {} |", c.letter());
        }
        let _ = writeln!(
            out,
            " {} | {} | {} |",
            row.latency_p95_ms
                .map_or_else(|| "n/a".to_owned(), |v| v.to_string()),
            row.undo.as_deref().map_or("n/a".to_owned(), cell_text),
            row.exceptions.join(", "),
        );
    }
    let _ = writeln!(
        out,
        "\nCells passed: {}/{} ({}%) · KPI met: {}",
        r.passed,
        r.total,
        r.percent(),
        if r.kpi_met() { "yes" } else { "no" }
    );
    if !r.kpi_met() {
        let mut why = Vec::new();
        if !r.rate_met() {
            why.push(format!("pass rate below {KPI_PERCENT}%"));
        }
        if !r.unresolved.is_empty() {
            why.push(format!(
                "{} failing cell(s) without an exception record",
                r.unresolved.len()
            ));
        }
        if !r.not_run.is_empty() {
            why.push(format!("{} cell(s) not run", r.not_run.len()));
        }
        let _ = writeln!(out, "\nWhy not: {}.", why.join("; "));
    }
    let _ = writeln!(
        out,
        "\nExceptions:\n| ID | Cell | Symptom | Cause | Resolution | Issue | Found | Re-tested |\n| -- | ---- | ------- | ----- | ---------- | ----- | ----- | --------- |"
    );
    for e in &r.exceptions {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            e.id,
            e.cell,
            cell_text(&e.symptom),
            cell_text(&e.cause),
            e.resolution.name(),
            e.issue.as_deref().unwrap_or(""),
            e.found,
            e.retested.as_deref().unwrap_or(""),
        );
    }
    let _ = writeln!(out, "\nFailures that still need an override or an issue:");
    if r.unresolved.is_empty() {
        let _ = writeln!(out, "- none");
    }
    for key in &r.unresolved {
        let _ = writeln!(
            out,
            "- {key}: add a record with `iamb-compat exception --os {} --cell {key} ...`",
            r.os.name()
        );
    }
    if !r.not_run.is_empty() {
        let _ = writeln!(
            out,
            "\nNot run ({}): {}",
            r.not_run.len(),
            r.not_run.join(", ")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::apps::apps_for;
    use crate::compat::results::{Mode, Record, Resolution};
    use crate::compat::types::types;

    fn rec(app: &str, typ: &str, pass: bool) -> Record {
        Record::Cell(CellRecord {
            ts: 1,
            os: Os::Windows,
            app: app.to_owned(),
            typ: typ.to_owned(),
            mode: Mode::Manual,
            pass,
            reps: 5,
            reps_passed: 5,
            nothing_expanded: None,
            latency_p95_ms: None,
            undo: None,
            note: String::new(),
            observed: None,
        })
    }

    /// Every cell of the Windows matrix as a pass; secure rows say nothing expanded.
    fn everything_passes() -> ResultSet {
        let apps = apps_for(Os::Windows).unwrap_or_default();
        let mut set = ResultSet::default();
        for app in &apps {
            for t in types().unwrap_or_default() {
                let mut r = rec(&app.id, &t.id, true);
                if let Record::Cell(c) = &mut r
                    && app.secure
                {
                    c.nothing_expanded = Some(true);
                }
                set.records.push(r);
            }
        }
        set
    }

    fn report(set: &ResultSet) -> Report {
        compute(
            Os::Windows,
            &apps_for(Os::Windows).unwrap_or_default(),
            &types().unwrap_or_default(),
            set,
        )
    }

    fn exception(cell: &str) -> Record {
        Record::Exception(ExceptionRecord {
            ts: 1,
            os: Os::Windows,
            id: "E-win-1".to_owned(),
            cell: cell.to_owned(),
            symptom: "s".to_owned(),
            cause: "c".to_owned(),
            resolution: Resolution::Override,
            issue: None,
            found: "2026-03-18".to_owned(),
            retested: None,
        })
    }

    #[test]
    fn a_full_pass_is_100_of_100() {
        let r = report(&everything_passes());
        assert_eq!((r.passed, r.total, r.percent()), (100, 100, 100));
        assert!(r.kpi_met());
        assert!(render_markdown(&r).contains("Cells passed: 100/100 (100%) · KPI met: yes"));
    }

    #[test]
    fn secure_rows_pass_only_when_nothing_expanded() {
        let mut set = everything_passes();
        // W19/T1: the tester said pass but the record says something expanded.
        for r in &mut set.records {
            if let Record::Cell(c) = r {
                if c.app == "W19" && c.typ == "T1" {
                    c.nothing_expanded = Some(false);
                }
                if c.app == "W20" && c.typ == "T3" {
                    c.nothing_expanded = None;
                }
            }
        }
        let r = report(&set);
        assert_eq!(r.passed, 98);
        assert_eq!(r.failures, ["W19/T1", "W20/T3"]);
    }

    #[test]
    fn the_threshold_is_95_cells_of_100() {
        let mut set = everything_passes();
        for r in &mut set.records {
            if let Record::Cell(c) = r {
                // Five failures: W1/T1..T5.
                if c.app == "W1" {
                    c.pass = false;
                }
            }
        }
        set.records.push(exception("W1/T1"));
        let r = report(&set);
        assert_eq!(r.passed, 95);
        assert!(r.rate_met());
        // Four of the five failures have no exception record.
        assert!(!r.kpi_met());
        assert_eq!(r.unresolved, ["W1/T2", "W1/T3", "W1/T4", "W1/T5"]);
        // One more failure: 94 of 100.
        for r in &mut set.records {
            if let Record::Cell(c) = r
                && c.app == "W2"
                && c.typ == "T1"
            {
                c.pass = false;
            }
        }
        let r = report(&set);
        assert_eq!(r.passed, 94);
        assert!(!r.rate_met());
    }

    #[test]
    fn exceptions_make_the_kpi_when_the_rate_holds() {
        let mut set = everything_passes();
        for r in &mut set.records {
            if let Record::Cell(c) = r
                && c.app == "W9"
                && c.typ == "T2"
            {
                c.pass = false;
            }
        }
        assert!(!report(&set).kpi_met());
        set.records.push(exception("W9/T2"));
        let r = report(&set);
        assert!(r.kpi_met());
        assert_eq!(r.rows[8].exceptions, ["E-win-1"]);
        // The re-run after the override passes and counts.
        set.records.push(rec("W9", "T2", true));
        assert_eq!(report(&set).passed, 100);
    }

    #[test]
    fn cells_not_run_do_not_pass_and_block_the_kpi() {
        let mut set = everything_passes();
        set.records
            .retain(|r| !matches!(r, Record::Cell(c) if c.app == "W3"));
        let r = report(&set);
        assert_eq!(r.passed, 95);
        assert!(r.rate_met());
        assert!(!r.kpi_met());
        assert_eq!(r.not_run.len(), 5);
        let md = render_markdown(&r);
        assert!(md.contains("5 cell(s) not run"));
        assert!(md.contains("Not run (5): W3/T1"));
    }

    #[test]
    fn the_table_follows_the_template() {
        let md = render_markdown(&report(&everything_passes()));
        assert!(md.contains(
            "| #  | App (version) | T1 | T2 | T3 | T4 | T5 | T1 p95 ms | Undo (T1) | Exception |"
        ));
        assert!(md.contains("| W1 | Notepad | P | P | P | P | P | n/a | n/a |  |"));
        assert!(md.contains("Failures that still need an override or an issue:\n- none"));
    }
}
