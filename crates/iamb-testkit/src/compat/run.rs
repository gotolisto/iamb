//! `iamb-compat run`: the walk through the matrix.
//!
//! For every app and snippet type the harness says what to open and where to put the caret.
//! Then, per cell:
//!
//! - **automatic** (where the app allows it and the system has a typist and a clipboard): after
//!   the person confirms and switches to the app, the harness types the abbreviation, waits,
//!   checks that the clipboard still holds the sentinel, types the caret probe `^`, selects all,
//!   copies, reads the clipboard and compares it with the expected text (the probe sits at the
//!   expected caret position), clears the field, and does it again for every repetition;
//! - **manual**: the person types the abbreviation as many times as asked and answers pass,
//!   fail or skip, with a note. The clipboard sentinel is still checked by the harness.
//!
//! Secure rows are always manual. Rich surfaces (T5) are always judged by the person: the
//! structure (bold, link, image) cannot be read back as text.
//!
//! The field's text is never written to the results file, unless a cell failed and the person
//! said yes to including it.

use std::collections::VecDeque;
use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::time::Duration;

use iamb_platform::{ClipboardAccess, ClipboardContent};

use super::apps::{App, Os};
use super::results::{
    AppInfo, CellRecord, Mode, Record, ResultSet, RunInfo, append, cell_key, now_secs,
};
use super::types::{CARET_PROBE, Expectation, SnippetType, next_day};

/// Where the harness asks and says things.
pub trait Ui {
    /// Shows text.
    fn say(&mut self, text: &str);
    /// Shows `prompt` and returns the answer line; `None` when the input has ended.
    fn ask(&mut self, prompt: &str) -> Option<String>;
}

/// Standard input and output.
#[derive(Debug, Default)]
pub struct StdUi;

impl Ui for StdUi {
    fn say(&mut self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
    }

    fn ask(&mut self, prompt: &str) -> Option<String> {
        {
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "{prompt} ");
            let _ = out.flush();
        }
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    }
}

/// A scripted person, for tests: fixed answers, everything said is kept.
#[derive(Debug, Default)]
pub struct ScriptedUi {
    /// The answers still to give.
    pub answers: VecDeque<String>,
    /// Everything shown, prompts included.
    pub shown: Vec<String>,
}

impl ScriptedUi {
    /// A person who gives `answers` in order and then closes the input.
    pub fn new(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|s| (*s).to_owned()).collect(),
            shown: Vec::new(),
        }
    }
}

impl Ui for ScriptedUi {
    fn say(&mut self, text: &str) {
        self.shown.push(text.to_owned());
    }

    fn ask(&mut self, prompt: &str) -> Option<String> {
        self.shown.push(format!("? {prompt}"));
        self.answers.pop_front()
    }
}

/// Which mode to use for the cells that allow both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChoice {
    /// Ask once per app.
    Ask,
    /// Automatic where possible.
    Auto,
    /// Always manual.
    Manual,
}

/// What `run` was asked to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// The matrix.
    pub os: Os,
    /// Only this app.
    pub app: Option<String>,
    /// The results file.
    pub results: PathBuf,
    /// The mode.
    pub mode: ModeChoice,
    /// Run cells again that already have a record.
    pub again: bool,
    /// Only these types (`T1`...); empty means all.
    pub types: Vec<String>,
    /// Repetitions per cell; `None` is 5.
    pub reps: Option<u32>,
    /// Seconds to switch to the app before typing starts.
    pub delay_secs: u64,
    /// Milliseconds to wait after typing before reading back.
    pub settle_ms: u64,
}

/// Repetitions per cell when nothing else is asked (matrix section 3).
pub const DEFAULT_REPS: u32 = 5;

/// The tools of the run.
pub struct Harness<'a> {
    /// Questions and answers.
    pub ui: &'a mut dyn Ui,
    /// Types and copies; `None` where the system has no typist.
    pub typist: Option<&'a mut dyn super::typist::Typist>,
    /// The clipboard; `None` where the system has none.
    pub clipboard: Option<&'a dyn ClipboardAccess>,
    /// Waits.
    pub pause: &'a dyn Fn(Duration),
    /// The UTC date, `YYYY-MM-DD`, offered as the default for today's date.
    pub utc_date: String,
}

/// What a run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    /// Cells recorded now.
    pub recorded: usize,
    /// Cells left alone because they already had a record.
    pub already_done: usize,
    /// The person quit (or the input ended).
    pub quit: bool,
}

enum Flow {
    Next,
    SkipApp,
    Quit,
}

/// Normalises a read-back: line ends become `\n` and one trailing line end is dropped when the
/// expected text has none.
pub fn normalise_readback(got: &str, expected_ends_with_newline: bool) -> String {
    let mut s = got.replace("\r\n", "\n").replace('\r', "\n");
    if !expected_ends_with_newline && s.ends_with('\n') {
        s.pop();
    }
    s
}

/// A description of how `got` differs from `expected` that holds no field text.
pub fn describe_mismatch(expected: &str, got: &str, abbreviation: &str) -> String {
    let (e, g): (Vec<char>, Vec<char>) = (expected.chars().collect(), got.chars().collect());
    let first = e
        .iter()
        .zip(&g)
        .position(|(a, b)| a != b)
        .unwrap_or(e.len().min(g.len()));
    let kind = match g.len().cmp(&e.len()) {
        std::cmp::Ordering::Greater => "extra characters",
        std::cmp::Ordering::Less => "missing characters",
        std::cmp::Ordering::Equal => "different characters",
    };
    let left = if got.contains(abbreviation) {
        ", the abbreviation is still there"
    } else {
        ""
    };
    format!(
        "{kind}: read back {} characters, expected {}, first difference at character {}{left}",
        g.len(),
        e.len(),
        first + 1
    )
}

impl Harness<'_> {
    fn pause_ms(&self, ms: u64) {
        (self.pause)(Duration::from_millis(ms));
    }

    fn choice(&mut self, prompt: &str, allowed: &str, default: char) -> Option<char> {
        loop {
            let answer = self.ui.ask(prompt)?;
            let c = answer
                .chars()
                .next()
                .map_or(default, |c| c.to_ascii_lowercase());
            if allowed.contains(c) {
                return Some(c);
            }
            self.ui.say(&format!("Please answer one of: {allowed}"));
        }
    }

    fn text(&mut self, prompt: &str) -> Option<String> {
        self.ui.ask(prompt)
    }

    fn clipboard_sentinel(&self, n: u32) -> Option<String> {
        let clipboard = self.clipboard?;
        let sentinel = format!("clip-sentinel-{n}");
        clipboard
            .write(&ClipboardContent::text(sentinel.clone()), false)
            .ok()?;
        Some(sentinel)
    }

    fn clipboard_text(&self) -> Option<String> {
        let clipboard = self.clipboard?;
        clipboard
            .read_text()
            .ok()
            .map(|t| t.map(|s| s.expose().clone()).unwrap_or_default())
    }
}

struct Cell<'a> {
    app: &'a App,
    typ: &'a SnippetType,
    reps: u32,
    dates: Vec<String>,
}

impl Cell<'_> {
    fn expectations(&self) -> Vec<Expectation> {
        self.dates
            .iter()
            .map(|d| self.typ.expectation(d, self.app.single_line))
            .collect()
    }
}

/// The result of judging a cell.
struct Verdict {
    mode: Mode,
    pass: bool,
    reps_passed: u32,
    nothing_expanded: Option<bool>,
    note: String,
    observed: Option<String>,
}

/// Runs the walk. Records go to `opts.results` as they are made.
pub fn run(
    h: &mut Harness<'_>,
    opts: &Options,
    apps: &[App],
    types: &[SnippetType],
) -> Result<Summary, String> {
    let mut set = ResultSet::load(&opts.results)?;
    let mut summary = Summary::default();
    let types: Vec<&SnippetType> = types
        .iter()
        .filter(|t| opts.types.is_empty() || opts.types.contains(&t.id))
        .collect();
    let apps: Vec<&App> = apps
        .iter()
        .filter(|a| {
            a.os == opts.os
                && opts
                    .app
                    .as_ref()
                    .is_none_or(|id| id.eq_ignore_ascii_case(&a.id))
        })
        .collect();
    if apps.is_empty() {
        return Err(format!(
            "no app {} in the {} matrix",
            opts.app.as_deref().unwrap_or("?"),
            opts.os.name()
        ));
    }
    let Some(run_info) = start_of_run(h, opts, &mut set)? else {
        summary.quit = true;
        return Ok(summary);
    };
    let mut dates = vec![run_info.date.clone()];
    dates.extend(next_day(&run_info.date));
    for app in apps {
        let pending: Vec<&&SnippetType> = types
            .iter()
            .filter(|t| opts.again || !set.done(opts.os, &app.id, &t.id))
            .collect();
        if pending.is_empty() {
            summary.already_done += types.len();
            h.ui.say(&format!(
                "{} {}: all cells already recorded (use --again to repeat).",
                app.id, app.name
            ));
            continue;
        }
        summary.already_done += types.len() - pending.len();
        match run_app(h, opts, app, &pending, &dates, &mut set, &mut summary)? {
            Flow::Quit => {
                summary.quit = true;
                break;
            }
            Flow::Next | Flow::SkipApp => {}
        }
    }
    Ok(summary)
}

fn start_of_run(
    h: &mut Harness<'_>,
    opts: &Options,
    set: &mut ResultSet,
) -> Result<Option<RunInfo>, String> {
    if let Some(run) = set.last_run(opts.os) {
        h.ui.say(&format!(
            "Resuming the {} run of {} on {}.",
            opts.os.name(),
            run.date,
            run.machine
        ));
        return Ok(Some(run.clone()));
    }
    h.ui.say("New run. A few facts for the results header (Enter to leave one blank).");
    let Some(os_build) = h.text("OS name and build (for example Windows 11 23H2 build 22631):")
    else {
        return Ok(None);
    };
    let Some(machine) = h.text("Machine (model, CPU, RAM):") else {
        return Ok(None);
    };
    let Some(commit) = h.text("Commit of the Iamb build under test:") else {
        return Ok(None);
    };
    let Some(tester) = h.text("Tester (role):") else {
        return Ok(None);
    };
    let default_date = h.utc_date.clone();
    let Some(date) = h.text(&format!(
        "Today's local date as YYYY-MM-DD [{default_date}]:"
    )) else {
        return Ok(None);
    };
    let date = if date.is_empty() { default_date } else { date };
    if next_day(&date).is_none() {
        return Err(format!("{date:?} is not a date like 2026-03-18"));
    }
    let info = RunInfo {
        ts: now_secs(),
        os: opts.os,
        os_build,
        machine,
        commit,
        tester,
        date,
    };
    append(&opts.results, &Record::Run(info.clone()))?;
    set.records.push(Record::Run(info.clone()));
    Ok(Some(info))
}

#[expect(
    clippy::too_many_lines,
    reason = "one linear walk over the cells of an app"
)]
fn run_app(
    h: &mut Harness<'_>,
    opts: &Options,
    app: &App,
    pending: &[&&SnippetType],
    dates: &[String],
    set: &mut ResultSet,
    summary: &mut Summary,
) -> Result<Flow, String> {
    h.ui.say(&format!("\n=== {} {} ===", app.id, app.name));
    h.ui.say(&format!("Open: {} - {}", app.name, app.surface));
    if !app.notes.is_empty() {
        h.ui.say(&format!("Known quirks: {}", app.notes));
    }
    if app.secure {
        h.ui.say(
            "SECURE ROW: Iamb must NOT expand anything here, and must not remember what you type.",
        );
    }
    let Some(go) = h.choice(
        "Ready? [Enter] start, [s] skip this app, [q] quit:",
        "sq\n",
        '\n',
    ) else {
        return Ok(Flow::Quit);
    };
    match go {
        's' => return Ok(Flow::SkipApp),
        'q' => return Ok(Flow::Quit),
        _ => {}
    }
    if set.app_version(opts.os, &app.id).is_none() {
        let Some(version) = h.text("Version of the app (Help > About):") else {
            return Ok(Flow::Quit);
        };
        let info = AppInfo {
            ts: now_secs(),
            os: opts.os,
            app: app.id.clone(),
            version,
        };
        append(&opts.results, &Record::App(info.clone()))?;
        set.records.push(Record::App(info));
    }
    if let Some(extra) = &app.extra {
        h.ui.say(&format!("Extra check (not a cell): {extra}"));
    }
    let auto_possible = !app.secure && app.auto && h.typist.is_some() && h.clipboard.is_some();
    let mut auto = auto_possible;
    if auto_possible {
        auto = match opts.mode {
            ModeChoice::Auto => true,
            ModeChoice::Manual => false,
            ModeChoice::Ask => {
                let Some(c) = h.choice(
                    "Automatic (the harness types and reads back) or manual? [A/m]:",
                    "am\n",
                    'a',
                ) else {
                    return Ok(Flow::Quit);
                };
                c != 'm'
            }
        };
    }
    for typ in pending {
        let cell = Cell {
            app,
            typ,
            reps: opts.reps.unwrap_or(DEFAULT_REPS).max(1),
            dates: dates.to_vec(),
        };
        let use_auto = auto && !(app.rich && typ.rich);
        let verdict = if app.secure {
            secure_cell(h, &cell)
        } else if use_auto {
            auto_cell(h, opts, &cell)?
        } else {
            manual_cell(h, &cell)
        };
        let Some(verdict) = verdict else {
            return Ok(Flow::Quit);
        };
        let Some(verdict) = verdict else {
            continue;
        };
        let undo = if typ.number() == 1 && !app.secure {
            ask_undo(h)
        } else {
            None
        };
        let record = CellRecord {
            ts: now_secs(),
            os: opts.os,
            app: app.id.clone(),
            typ: typ.id.clone(),
            mode: verdict.mode,
            pass: verdict.pass,
            reps: cell.reps,
            reps_passed: verdict.reps_passed,
            nothing_expanded: verdict.nothing_expanded,
            latency_p95_ms: None,
            undo,
            note: verdict.note,
            observed: verdict.observed,
        };
        append(&opts.results, &Record::Cell(record.clone()))?;
        set.records.push(Record::Cell(record));
        summary.recorded += 1;
        h.ui.say(&format!(
            "{} recorded: {}",
            cell_key(&app.id, &typ.id),
            if verdict.pass { "PASS" } else { "FAIL" }
        ));
    }
    Ok(Flow::Next)
}

fn ask_undo(h: &mut Harness<'_>) -> Option<String> {
    let c = h.choice(
        "One undo (Ctrl+Z / Cmd+Z) after an expansion: [w]hole expansion removed, [p]art of it, [a]bbreviation restored, [n]othing, Enter to skip:",
        "wpan\n",
        '\n',
    )?;
    match c {
        'w' => Some("whole".to_owned()),
        'p' => Some("partial".to_owned()),
        'a' => Some("abbreviation".to_owned()),
        'n' => Some("none".to_owned()),
        _ => None,
    }
}

fn describe_expected(h: &mut Harness<'_>, cell: &Cell<'_>) {
    let exps = cell.expectations();
    let exp = &exps[0];
    if cell.typ.has_date() {
        h.ui.say(&format!(
            "Expected text (with today's date), caret shown as |:\n{}",
            exp.with_marker('|')
        ));
    } else {
        h.ui.say(&format!(
            "Expected text, caret shown as |:\n{}",
            exp.with_marker('|')
        ));
    }
    if cell.app.single_line && cell.typ.single_line.is_some() {
        h.ui.say("This is a one-line field: line breaks are dropped.");
    }
    if cell.app.rich && cell.typ.rich {
        for check in &cell.typ.rich_checks {
            h.ui.say(&format!("Rich surface, check: {check}"));
        }
    }
}

/// `None` when the person quit; `Some(None)` when the cell was skipped.
type Judged = Option<Option<Verdict>>;

fn manual_cell(h: &mut Harness<'_>, cell: &Cell<'_>) -> Judged {
    h.ui.say(&format!(
        "\n--- {}/{} {}: type {} ({} times, clearing the field between) ---",
        cell.app.id, cell.typ.id, cell.typ.name, cell.typ.abbreviation, cell.reps
    ));
    describe_expected(h, cell);
    let snapshot = h.clipboard.and_then(|c| c.snapshot().ok());
    let sentinel = h.clipboard_sentinel(1);
    if sentinel.is_some() {
        h.ui.say("The clipboard now holds a test value; the harness checks it afterwards.");
    }
    let verdict = h.choice(
        "Result: [p]ass, [f]ail, [s]kip this cell, [q]uit:",
        "pfsq",
        's',
    );
    let clipboard_ok = sentinel
        .as_ref()
        .map(|s| h.clipboard_text().as_deref() == Some(s.as_str()));
    if let (Some(c), Some(snap)) = (h.clipboard, snapshot) {
        let _ = c.restore(snap);
    }
    let verdict = verdict?;
    if verdict == 'q' {
        return None;
    }
    if verdict == 's' {
        return Some(None);
    }
    let mut pass = verdict == 'p';
    let mut note = h.text("Note (Enter for none):")?;
    if pass && clipboard_ok == Some(false) {
        pass = false;
        let msg = "clipboard not restored after the expansion";
        h.ui.say(&format!(
            "The harness found the clipboard changed: recording a failure ({msg})."
        ));
        note = if note.is_empty() {
            msg.to_owned()
        } else {
            format!("{msg}; {note}")
        };
    }
    Some(Some(Verdict {
        mode: Mode::Manual,
        pass,
        reps_passed: if pass { cell.reps } else { 0 },
        nothing_expanded: None,
        note,
        observed: None,
    }))
}

fn secure_cell(h: &mut Harness<'_>, cell: &Cell<'_>) -> Judged {
    h.ui.say(&format!(
        "\n--- {}/{} {}: type {} into the password field ---",
        cell.app.id, cell.typ.id, cell.typ.name, cell.typ.abbreviation
    ));
    h.ui.say(&format!(
        "Pass when the field holds exactly the {} typed characters (nothing deleted, nothing inserted) and the clipboard is untouched.",
        cell.typ.abbreviation.chars().count()
    ));
    let snapshot = h.clipboard.and_then(|c| c.snapshot().ok());
    let sentinel = h.clipboard_sentinel(1);
    let nothing = h.choice(
        "Did anything expand, get deleted or get inserted? [n]o (pass), [y]es (fail), [s]kip, [q]uit:",
        "nysq",
        's',
    );
    let clipboard_ok = sentinel
        .as_ref()
        .map(|s| h.clipboard_text().as_deref() == Some(s.as_str()));
    if let (Some(c), Some(snap)) = (h.clipboard, snapshot) {
        let _ = c.restore(snap);
    }
    let nothing = nothing?;
    match nothing {
        'q' => return None,
        's' => return Some(None),
        _ => {}
    }
    let mut pass = nothing == 'n';
    let mut notes = Vec::new();
    if !pass {
        notes.push("something expanded or changed in the secure field".to_owned());
    }
    if clipboard_ok == Some(false) {
        pass = false;
        notes.push("clipboard changed".to_owned());
    }
    let diag = h.choice(
        "Does the diagnostics ring / worker.log show a \"secure field\" entry? [y]es, [n]o, [u]nknown:",
        "ynu",
        'u',
    )?;
    if diag == 'n' {
        pass = false;
        notes.push("no secure-field diagnostic".to_owned());
    }
    if cell.typ.number() == 1 {
        let half = cell.typ.abbreviation.chars().count().div_ceil(2);
        let (first, second): (String, String) = (
            cell.typ.abbreviation.chars().take(half).collect(),
            cell.typ.abbreviation.chars().skip(half).collect(),
        );
        h.ui.say(&format!(
            "Buffer check: type {first} in the password field, click into a normal field, type {second}."
        ));
        let buf = h.choice("Did anything expand? [n]o (pass), [y]es (fail):", "ny", 'y')?;
        if buf == 'y' {
            pass = false;
            notes.push("buffer check: text typed in the secure field was remembered".to_owned());
        }
    }
    let user_note = h.text("Note (Enter for none):")?;
    if !user_note.is_empty() {
        notes.push(user_note);
    }
    Some(Some(Verdict {
        mode: Mode::Manual,
        pass,
        reps_passed: u32::from(pass),
        nothing_expanded: Some(nothing == 'n'),
        note: notes.join("; "),
        observed: None,
    }))
}

struct AutoRun {
    passed: u32,
    problems: Vec<String>,
    last_readback: Option<String>,
}

fn auto_cell(h: &mut Harness<'_>, opts: &Options, cell: &Cell<'_>) -> Result<Judged, String> {
    h.ui.say(&format!(
        "\n--- {}/{} {}: automatic, {} repetitions of {} ---",
        cell.app.id, cell.typ.id, cell.typ.name, cell.reps, cell.typ.abbreviation
    ));
    h.ui.say(&format!(
        "Click into the empty field of {} now. Typing starts {} s after you press Enter. Do not touch the keyboard or mouse until it says done.",
        cell.app.name, opts.delay_secs
    ));
    let go = h.choice(
        "[Enter] start, [m]anual for this cell, [s]kip, [q]uit:",
        "msq\n",
        '\n',
    );
    match go {
        None | Some('q') => return Ok(None),
        Some('s') => return Ok(Some(None)),
        Some('m') => return Ok(Some(manual_cell_verdict(h, cell))),
        _ => {}
    }
    let mut attempt = 0;
    loop {
        attempt += 1;
        h.pause_ms(opts.delay_secs * 1000);
        let snapshot = h.clipboard.and_then(|c| c.snapshot().ok());
        let result = auto_reps(h, opts, cell);
        if let (Some(c), Some(snap)) = (h.clipboard, snapshot) {
            let _ = c.restore(snap);
        }
        let run = match result {
            Ok(run) => run,
            Err(e) => {
                h.ui.say(&format!("The harness could not type or copy: {e}"));
                let c = h.choice("[m]anual for this cell, [s]kip, [q]uit:", "msq", 's');
                return Ok(match c {
                    Some('m') => Some(manual_cell_verdict(h, cell)),
                    Some('s') => Some(None),
                    _ => None,
                });
            }
        };
        h.ui.say("Done.");
        if run.passed == cell.reps {
            return Ok(Some(Some(Verdict {
                mode: Mode::Auto,
                pass: true,
                reps_passed: run.passed,
                nothing_expanded: None,
                note: String::new(),
                observed: None,
            })));
        }
        h.ui.say(&format!(
            "{} of {} repetitions passed. First problem: {}",
            run.passed,
            cell.reps,
            run.problems.first().map_or("unknown", String::as_str)
        ));
        let again = if attempt < 2 { "[r]etry once, " } else { "" };
        let c = h.choice(
            &format!("{again}[m]anual judgement, [Enter] record the failure, [s]kip, [q]uit:"),
            "rmsq\n",
            '\n',
        );
        match c {
            Some('r') if attempt < 2 => {
                h.ui.say("Clear the field and click into it again.");
                continue;
            }
            Some('m') => return Ok(Some(manual_cell_verdict(h, cell))),
            Some('s') => return Ok(Some(None)),
            None | Some('q') => return Ok(None),
            _ => {}
        }
        let mut observed = None;
        if let Some(text) = run.last_readback {
            let c = h.choice(
                "Include the field's text in the results file to help with the exception record? It may hold other text you see there. [y/N]:",
                "yn\n",
                'n',
            );
            if c == Some('y') {
                observed = Some(text);
            }
        }
        let note = h.text("Note (Enter for none):").unwrap_or_default();
        let mut parts = run.problems;
        parts.dedup();
        if !note.is_empty() {
            parts.push(note);
        }
        return Ok(Some(Some(Verdict {
            mode: Mode::Auto,
            pass: false,
            reps_passed: run.passed,
            nothing_expanded: None,
            note: parts.join("; "),
            observed,
        })));
    }
}

fn manual_cell_verdict(h: &mut Harness<'_>, cell: &Cell<'_>) -> Option<Verdict> {
    // Used after an automatic cell is handed to the person: a quit or skip ends the cell.
    manual_cell(h, cell).flatten()
}

fn auto_reps(h: &mut Harness<'_>, opts: &Options, cell: &Cell<'_>) -> Result<AutoRun, String> {
    let expectations: Vec<String> = cell
        .expectations()
        .iter()
        .map(|e| e.with_marker(CARET_PROBE))
        .collect();
    let ends_nl = expectations.iter().any(|e| e.ends_with('\n'));
    let mut run = AutoRun {
        passed: 0,
        problems: Vec::new(),
        last_readback: None,
    };
    for rep in 1..=cell.reps {
        let sentinel = h.clipboard_sentinel(rep);
        let abbreviation = cell.typ.abbreviation.clone();
        {
            let typist = h.typist.as_deref_mut().ok_or("no typist on this system")?;
            typist.type_text(&abbreviation)?;
        }
        h.pause_ms(opts.settle_ms);
        let mut problem = None;
        if let Some(s) = &sentinel
            && h.clipboard_text().as_deref() != Some(s.as_str())
        {
            problem = Some("clipboard not restored after the expansion".to_owned());
        }
        {
            let typist = h.typist.as_deref_mut().ok_or("no typist on this system")?;
            typist.type_text(&CARET_PROBE.to_string())?;
        }
        h.pause_ms(150);
        {
            let typist = h.typist.as_deref_mut().ok_or("no typist on this system")?;
            typist.select_all_copy()?;
        }
        h.pause_ms(250);
        let got = h.clipboard_text().unwrap_or_default();
        let got = normalise_readback(&got, ends_nl);
        if !expectations.contains(&got) {
            let closest = &expectations[0];
            let why = describe_mismatch(closest, &got, &abbreviation);
            problem = Some(match problem {
                Some(p) => format!("{p}; {why}"),
                None => why,
            });
            run.last_readback = Some(got);
        }
        {
            let typist = h.typist.as_deref_mut().ok_or("no typist on this system")?;
            typist.clear_field()?;
        }
        h.pause_ms(250);
        match problem {
            None => run.passed += 1,
            Some(p) => run.problems.push(format!("repetition {rep}: {p}")),
        }
    }
    Ok(run)
}
