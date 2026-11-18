//! Tests of the compatibility harness (task 2.7) that need no real desktop: the fixture library
//! through the engine's dry run, the app list against the matrix document, a whole run against
//! a simulated machine, resume, the report, and the command line.

#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use iamb_cli::expand::{DryEnv, expand, personal_profile, snapshot_from_store};
use iamb_core::datetime::LocalDateTime;
use iamb_core::template::FormValues;
use iamb_platform::mock::MockPlatform;
use iamb_platform::{ClipboardAccess, ClipboardContent};
use iamb_store::Store;
use iamb_testkit::compat::apps::{Os, all_apps, apps_for};
use iamb_testkit::compat::report::{compute, render_markdown};
use iamb_testkit::compat::results::{Mode, Record, ResultSet};
use iamb_testkit::compat::run::{Harness, ModeChoice, Options, ScriptedUi, Summary, Ui, run};
use iamb_testkit::compat::setup::import_fixtures;
use iamb_testkit::compat::types::{
    CARET_PROBE, FIXED_DATE, FIXTURE_JSON, SnippetType, render_expected_txt, types,
};
use iamb_testkit::compat::typist::Typist;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

// ---- the fixture library --------------------------------------------------------------------

#[test]
fn the_library_imports_cleanly_and_twice_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let first = import_fixtures(dir.path()).unwrap();
    assert_eq!((first.imported, first.skipped), (5, 0));
    let second = import_fixtures(dir.path()).unwrap();
    assert_eq!((second.imported, second.skipped), (0, 5));
}

#[test]
fn each_snippet_expands_to_its_expected_text_with_the_fixed_date() {
    let dir = tempfile::tempdir().unwrap();
    import_fixtures(dir.path()).unwrap();
    // Wait for the lock as the service and the command-line tool do: another test here spawns
    // a process, and a child inherits the lock's descriptor between fork and exec.
    let store = Store::open_with_timeout(dir.path(), std::time::Duration::from_secs(3)).unwrap();
    let profile = personal_profile(&store).unwrap().unwrap();
    let snapshot = snapshot_from_store(&store, profile).unwrap();
    let env = DryEnv {
        now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).unwrap(),
        clipboard: None,
    };
    let kinds = types().unwrap();
    assert_eq!(kinds.len(), 5);
    for t in &kinds {
        let outcome = expand(&snapshot, &t.abbreviation, &env, &FormValues::new());
        let want = t.expectation(FIXED_DATE, false);
        assert_eq!(outcome.expansions, 1, "{} expands once", t.id);
        assert!(
            outcome.diagnostics.is_empty(),
            "{}: {:?}",
            t.id,
            outcome.diagnostics
        );
        assert_eq!(outcome.text, want.text, "{} text", t.id);
        assert_eq!(outcome.caret, want.caret, "{} caret", t.id);
    }
}

#[test]
fn the_fixture_has_exactly_the_types_of_the_matrix() {
    let doc = fs::read_to_string(repo_root().join("docs/compatibility-matrix.md")).unwrap();
    let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
    let snippets = json["snippets"].as_array().unwrap();
    let kinds = types().unwrap();
    assert_eq!(snippets.len(), kinds.len());
    for t in &kinds {
        let s = snippets
            .iter()
            .find(|s| s["abbreviation"] == t.abbreviation.as_str())
            .unwrap_or_else(|| panic!("{} is not in compat.json", t.abbreviation));
        assert_eq!(s["trigger"], "immediate");
        assert_eq!(s["case_mode"], "exact");
        assert_eq!(s["kind"], if t.rich { "rich" } else { "plain" });
        assert!(
            doc.contains(&format!("`{}`", t.abbreviation)),
            "the matrix document does not name {}",
            t.abbreviation
        );
    }
    // T1: 27 characters; T2: exactly 500 characters on 8 lines.
    let by = |n: usize| {
        snippets
            .iter()
            .find(|s| s["abbreviation"] == format!(";cpt{n}"))
            .unwrap()
    };
    assert_eq!(by(1)["body"].as_str().unwrap().chars().count(), 27);
    let t2 = by(2)["body"].as_str().unwrap();
    assert_eq!((t2.chars().count(), t2.lines().count()), (500, 8));
}

#[test]
fn the_expected_text_file_is_generated_from_the_data() {
    let file = fs::read_to_string(fixtures().join("compat-expected.txt")).unwrap();
    let made = render_expected_txt(FIXED_DATE).unwrap();
    assert_eq!(
        file.trim_end(),
        made.trim_end(),
        "regenerate with: iamb-compat expected > crates/iamb-testkit/fixtures/compat-expected.txt"
    );
}

// ---- the app list against the matrix document -------------------------------------------------

struct DocApp {
    id: String,
    name: String,
    surface: String,
    secure: bool,
}

fn plain(s: &str) -> String {
    s.replace(['`', '*'], "")
}

fn doc_apps(doc: &str) -> Vec<DocApp> {
    let start = doc.find("## 1. Apps").unwrap();
    let end = doc.find("## 2. Snippet types").unwrap();
    doc[start..end]
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line
                .trim()
                .trim_matches('|')
                .split(" | ")
                .map(str::trim)
                .collect();
            let id = cells.first()?;
            let ok = id.len() > 1
                && (id.starts_with('W') || id.starts_with('M'))
                && id[1..].chars().all(|c| c.is_ascii_digit());
            if !ok || cells.len() < 5 {
                return None;
            }
            Some(DocApp {
                id: (*id).to_owned(),
                name: plain(cells[1]),
                surface: plain(cells[3]),
                secure: cells[4].contains("nothing expands"),
            })
        })
        .collect()
}

#[test]
fn the_app_file_and_the_matrix_document_agree() {
    let doc = fs::read_to_string(repo_root().join("docs/compatibility-matrix.md")).unwrap();
    let from_doc = doc_apps(&doc);
    let from_file = all_apps().unwrap();
    assert_eq!(from_doc.len(), 40, "20 rows per OS in the document");
    assert_eq!(from_file.len(), 40);
    for os in [Os::Windows, Os::Macos] {
        assert_eq!(apps_for(os).unwrap().len(), 20);
    }
    for (d, f) in from_doc.iter().zip(&from_file) {
        assert_eq!(d.id, f.id);
        assert_eq!(d.name, f.name, "{} name", d.id);
        assert_eq!(d.surface, f.surface, "{} surface", d.id);
        assert_eq!(d.secure, f.secure, "{} secure flag", d.id);
    }
    // The rich surfaces named under the snippet-type table.
    let line = doc
        .lines()
        .find(|l| l.starts_with("Rich surfaces for T5:"))
        .unwrap();
    let rich_doc: Vec<&str> = line["Rich surfaces for T5:".len()..]
        .split(['.', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() < 5)
        .collect();
    let rich_file: Vec<&str> = from_file
        .iter()
        .filter(|a| a.rich)
        .map(|a| a.id.as_str())
        .collect();
    let mut a = rich_doc.clone();
    let mut b = rich_file.clone();
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b, "rich rows");
    // Every row has what a person needs to find the surface.
    for app in &from_file {
        assert!(!app.surface.is_empty() && !app.name.is_empty());
        assert!(
            !app.secure || !app.auto,
            "{}: no automatic mode on a password field",
            app.id
        );
    }
}

// ---- a run against a simulated machine --------------------------------------------------------

/// A field that behaves like an app with Iamb running: typing the abbreviation of a snippet
/// replaces it with the expansion and puts the caret where the template says.
struct FakeField {
    machine: MockPlatform,
    kinds: Vec<SnippetType>,
    date: String,
    field: Vec<char>,
    caret: usize,
    /// Expansions lose their last character (a deliberately broken app).
    drop_last: bool,
    typed: Vec<String>,
}

impl FakeField {
    fn new(machine: &MockPlatform, drop_last: bool) -> Self {
        Self {
            machine: machine.clone(),
            kinds: types().unwrap(),
            date: FIXED_DATE.to_owned(),
            field: Vec::new(),
            caret: 0,
            drop_last,
            typed: Vec::new(),
        }
    }
}

impl Typist for FakeField {
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.typed.push(text.to_owned());
        for c in text.chars() {
            self.field.insert(self.caret, c);
            self.caret += 1;
            let before: String = self.field[..self.caret].iter().collect();
            for t in &self.kinds {
                if before.ends_with(&t.abbreviation) {
                    let start = self.caret - t.abbreviation.chars().count();
                    let mut exp = t.expectation(&self.date, false);
                    if self.drop_last {
                        exp.text.pop();
                    }
                    self.field.splice(start..self.caret, exp.text.chars());
                    self.caret = start + exp.caret.min(exp.text.chars().count());
                }
            }
        }
        Ok(())
    }

    fn select_all_copy(&mut self) -> Result<(), String> {
        let text: String = self.field.iter().collect();
        self.machine
            .write(&ClipboardContent::text(text), false)
            .map_err(|e| e.to_string())
    }

    fn clear_field(&mut self) -> Result<(), String> {
        self.field.clear();
        self.caret = 0;
        Ok(())
    }
}

fn options(dir: &Path, app: &str) -> Options {
    Options {
        os: Os::Windows,
        app: Some(app.to_owned()),
        results: dir.join("r.jsonl"),
        mode: ModeChoice::Auto,
        again: false,
        types: Vec::new(),
        reps: Some(2),
        delay_secs: 0,
        settle_ms: 0,
    }
}

fn run_with(
    answers: &[&str],
    machine: &MockPlatform,
    drop_last: bool,
    opts: &Options,
) -> (Summary, ScriptedUi, FakeField) {
    let apps = all_apps().unwrap();
    let kinds = types().unwrap();
    let mut ui = ScriptedUi::new(answers);
    let mut field = FakeField::new(machine, drop_last);
    let pause = |_: std::time::Duration| {};
    let summary = {
        let mut h = Harness {
            ui: &mut ui,
            typist: Some(&mut field),
            clipboard: Some(machine),
            pause: &pause,
            utc_date: FIXED_DATE.to_owned(),
        };
        run(&mut h, opts, &apps, &kinds).unwrap()
    };
    (summary, ui, field)
}

/// Answers for a first run of one plain app in automatic mode: the header facts, the date,
/// "ready", the version, then per cell "start" (and the undo answer after T1).
const HEADER: [&str; 5] = [
    "Windows 11 test build",
    "test machine",
    "abc1234",
    "tester",
    "",
];

fn auto_answers() -> Vec<&'static str> {
    let mut a = HEADER.to_vec();
    a.extend(["", "1.2.3"]); // ready, version
    a.extend(["", "w"]); // T1 start, undo
    a.extend(["", "", "", ""]); // T2..T5 start
    a
}

#[test]
fn an_automatic_run_records_passes_and_never_the_field_text() {
    let dir = tempfile::tempdir().unwrap();
    let machine = MockPlatform::new();
    machine.copy(ClipboardContent::text("the owner's own clipboard"));
    let opts = options(dir.path(), "W1");
    let (summary, _ui, field) = run_with(&auto_answers(), &machine, false, &opts);
    assert_eq!((summary.recorded, summary.quit), (5, false));
    // The harness typed each abbreviation, then the caret probe, per repetition.
    assert_eq!(
        field.typed.iter().filter(|t| t.as_str() == ";cpt4").count(),
        2
    );
    assert_eq!(field.typed.iter().filter(|t| t.as_str() == "^").count(), 10);
    // The owner's clipboard came back.
    assert_eq!(
        machine
            .read_text()
            .unwrap()
            .map(|t| t.expose().clone())
            .as_deref(),
        Some("the owner's own clipboard")
    );
    let raw = fs::read_to_string(&opts.results).unwrap();
    assert!(
        !raw.contains("Kind regards"),
        "no field text in the results"
    );
    assert!(!raw.contains("Report for"));
    let set = ResultSet::load(&opts.results).unwrap();
    for t in 1..=5 {
        let c = set.cell(Os::Windows, "W1", &format!("T{t}")).unwrap();
        assert!(c.pass, "T{t}");
        assert_eq!((c.mode, c.reps, c.reps_passed), (Mode::Auto, 2, 2));
    }
    assert_eq!(
        set.cell(Os::Windows, "W1", "T1").unwrap().undo.as_deref(),
        Some("whole")
    );
    assert_eq!(set.app_version(Os::Windows, "W1"), Some("1.2.3"));
    assert_eq!(set.last_run(Os::Windows).unwrap().tester, "tester");
}

#[test]
fn a_run_resumes_and_again_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let machine = MockPlatform::new();
    let mut opts = options(dir.path(), "W1");
    opts.types = vec!["T1".to_owned(), "T2".to_owned()];
    let mut answers = HEADER.to_vec();
    answers.extend(["", "1.0", "", "n", ""]);
    let (first, ..) = run_with(&answers, &machine, false, &opts);
    assert_eq!(first.recorded, 2);

    // Second start: the header, the version and the done cells are not asked again.
    let (second, ui, _) = run_with(&[], &machine, false, &opts);
    assert_eq!((second.recorded, second.already_done), (0, 2));
    assert!(ui.shown.iter().any(|l| l.contains("already recorded")));
    let lines = fs::read_to_string(&opts.results).unwrap().lines().count();
    assert_eq!(lines, 4, "run, app, two cells");

    // Resume with one more type: only T3 is asked.
    opts.types = vec!["T1".to_owned(), "T2".to_owned(), "T3".to_owned()];
    let (third, ..) = run_with(&["", ""], &machine, false, &opts);
    assert_eq!((third.recorded, third.already_done), (1, 2));

    // --again runs a cell once more and appends; the latest record counts.
    opts.types = vec!["T3".to_owned()];
    opts.again = true;
    let (fourth, ..) = run_with(&["", "", ""], &machine, false, &opts);
    assert_eq!(fourth.recorded, 1);
    let set = ResultSet::load(&opts.results).unwrap();
    let t3: Vec<_> = set
        .records
        .iter()
        .filter(|r| matches!(r, Record::Cell(c) if c.typ == "T3"))
        .collect();
    assert_eq!(t3.len(), 2);
}

#[test]
fn a_failing_app_is_recorded_without_its_text_unless_the_person_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let machine = MockPlatform::new();
    let mut opts = options(dir.path(), "W1");
    opts.types = vec!["T1".to_owned()];
    // header, ready, version, T1 start, [Enter] record the failure (retry offered first),
    // include the text? no, note, undo skipped.
    let mut answers = HEADER.to_vec();
    answers.extend(["", "1.0", "", "", "n", "dropped the end", ""]);
    let (summary, ui, _) = run_with(&answers, &machine, true, &opts);
    assert_eq!(summary.recorded, 1);
    let set = ResultSet::load(&opts.results).unwrap();
    let c = set.cell(Os::Windows, "W1", "T1").unwrap();
    assert!(!c.pass);
    assert_eq!(c.reps_passed, 0);
    assert!(c.note.contains("missing characters"), "{}", c.note);
    assert!(c.note.contains("dropped the end"));
    assert!(c.observed.is_none());
    let shown = ui.shown.join("\n");
    assert!(
        !shown.contains("Kind regards, the Iamb tea\n"),
        "the field's text is not echoed"
    );

    // With the person's agreement the text is kept, for failed cells only.
    let dir2 = tempfile::tempdir().unwrap();
    let mut opts2 = options(dir2.path(), "W1");
    opts2.types = vec!["T1".to_owned()];
    let mut answers = HEADER.to_vec();
    answers.extend(["", "1.0", "", "", "y", "", ""]);
    run_with(&answers, &machine, true, &opts2);
    let set = ResultSet::load(&opts2.results).unwrap();
    let observed = set
        .cell(Os::Windows, "W1", "T1")
        .unwrap()
        .observed
        .clone()
        .unwrap();
    assert!(observed.starts_with("Kind regards, the Iamb tea"));
    assert!(observed.ends_with(CARET_PROBE));
}

#[test]
fn secure_rows_are_manual_and_pass_when_nothing_expands() {
    let dir = tempfile::tempdir().unwrap();
    let machine = MockPlatform::new();
    let opts = options(dir.path(), "W19");
    let mut answers = HEADER.to_vec();
    answers.extend(["", "7.0"]); // ready, version
    // T1: nothing expanded, diagnostics yes, buffer check no, note. T2..T5: n, y, note.
    answers.extend(["n", "y", "n", ""]);
    for _ in 2..=5 {
        answers.extend(["n", "y", ""]);
    }
    let (summary, ..) = run_with(&answers, &machine, false, &opts);
    assert_eq!(summary.recorded, 5);
    let set = ResultSet::load(&opts.results).unwrap();
    for t in 1..=5 {
        let c = set.cell(Os::Windows, "W19", &format!("T{t}")).unwrap();
        assert!(c.pass && c.mode == Mode::Manual);
        assert_eq!(c.nothing_expanded, Some(true));
    }
    let report = compute(
        Os::Windows,
        &apps_for(Os::Windows).unwrap(),
        &types().unwrap(),
        &set,
    );
    assert_eq!(report.passed, 5);
}

/// A person whose expansion leaves its own text on the clipboard while they judge the cell.
struct DirtyingUi {
    inner: ScriptedUi,
    machine: MockPlatform,
}

impl Ui for DirtyingUi {
    fn say(&mut self, text: &str) {
        self.inner.say(text);
    }

    fn ask(&mut self, prompt: &str) -> Option<String> {
        if prompt.starts_with("Result:") {
            self.machine
                .copy(ClipboardContent::text("left behind by the expansion"));
        }
        self.inner.ask(prompt)
    }
}

#[test]
fn a_manual_pass_is_failed_when_the_clipboard_was_not_restored() {
    let dir = tempfile::tempdir().unwrap();
    let machine = MockPlatform::new();
    machine.copy(ClipboardContent::text("mine"));
    let mut opts = options(dir.path(), "W1");
    opts.mode = ModeChoice::Manual;
    opts.types = vec!["T1".to_owned()];
    let mut answers = HEADER.to_vec();
    answers.extend(["", "1.0", "p", "", ""]);
    let mut ui = DirtyingUi {
        inner: ScriptedUi::new(&answers),
        machine: machine.clone(),
    };
    let pause = |_: std::time::Duration| {};
    {
        let mut h = Harness {
            ui: &mut ui,
            typist: None,
            clipboard: Some(&machine),
            pause: &pause,
            utc_date: FIXED_DATE.to_owned(),
        };
        run(&mut h, &opts, &all_apps().unwrap(), &types().unwrap()).unwrap();
    }
    let set = ResultSet::load(&opts.results).unwrap();
    let c = set.cell(Os::Windows, "W1", "T1").unwrap();
    assert!(
        !c.pass,
        "the person said pass but the clipboard was not restored"
    );
    assert_eq!(c.mode, Mode::Manual);
    assert!(c.note.contains("clipboard not restored"));
    // The owner's own clipboard is back.
    assert_eq!(
        machine
            .read_text()
            .unwrap()
            .map(|t| t.expose().clone())
            .as_deref(),
        Some("mine")
    );
}

#[test]
fn the_report_of_a_run_is_a_markdown_table() {
    let dir = tempfile::tempdir().unwrap();
    let machine = MockPlatform::new();
    let opts = options(dir.path(), "W1");
    run_with(&auto_answers(), &machine, false, &opts);
    let set = ResultSet::load(&opts.results).unwrap();
    let r = compute(
        Os::Windows,
        &apps_for(Os::Windows).unwrap(),
        &types().unwrap(),
        &set,
    );
    let md = render_markdown(&r);
    assert!(md.contains("Run: 2026-03-18 · OS: Windows 11 test build · Machine: test machine · Commit: abc1234 · Tester: tester"));
    assert!(md.contains("| W1 | Notepad (1.2.3) | P | P | P | P | P | n/a | whole |  |"));
    assert!(md.contains("| W2 | Google Chrome | - | - | - | - | - |"));
    assert!(md.contains("Cells passed: 5/100 (5%) · KPI met: no"));
}

// ---- the command line ---------------------------------------------------------------------------

fn compat(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_iamb-compat"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_works_for_every_subcommand() {
    let top = compat(&["--help"]);
    assert!(top.status.success());
    let text = String::from_utf8(top.stdout).unwrap();
    for sub in ["setup", "run", "report", "exception", "expected"] {
        assert!(text.contains(sub), "top-level help lists {sub}");
        let out = compat(&[sub, "--help"]);
        assert!(out.status.success(), "{sub} --help");
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(help.contains("Usage: iamb-compat"), "{sub}: {help}");
    }
    let run_help = String::from_utf8(compat(&["run", "--help"]).stdout).unwrap();
    for flag in ["--os", "--app", "--results", "--again", "--mode"] {
        assert!(run_help.contains(flag), "run --help names {flag}");
    }
}

#[test]
fn setup_report_and_exception_work_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let out = compat(&["setup", "--data-dir", data.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("5 snippet(s) imported"));
    assert!(text.contains("--data-dir"));

    // A results file with one failing cell.
    let results = dir.path().join("r.jsonl");
    let machine = MockPlatform::new();
    let mut answers = HEADER.to_vec();
    answers.extend(["", "1.0", "", "", "n", "broken", ""]);
    let mut o = options(dir.path(), "W1");
    o.types = vec!["T1".to_owned()];
    run_with(&answers, &machine, true, &o);
    assert_eq!(o.results, results);

    let report = compat(&["report", "--results", results.to_str().unwrap()]);
    let md = String::from_utf8(report.stdout).unwrap();
    assert!(md.contains("W1/T1: add a record"), "{md}");
    let none = compat(&[
        "exception",
        "--os",
        "windows",
        "--cell",
        "W1/T1",
        "--resolution",
        "issue",
        "--symptom",
        "x",
        "--results",
        results.to_str().unwrap(),
    ]);
    assert!(!none.status.success(), "an issue needs a link");
    let ok = compat(&[
        "exception",
        "--os",
        "windows",
        "--cell",
        "W1/T1",
        "--resolution",
        "issue",
        "--issue",
        "https://example.org/1",
        "--symptom",
        "drops a character",
        "--results",
        results.to_str().unwrap(),
    ]);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let md = String::from_utf8(compat(&["report", "--results", results.to_str().unwrap()]).stdout)
        .unwrap();
    assert!(
        md.contains("| E-win-1 | W1/T1 | drops a character |"),
        "{md}"
    );
    assert!(md.contains("Failures that still need an override or an issue:\n- none"));
}
