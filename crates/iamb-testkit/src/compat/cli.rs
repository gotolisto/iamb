//! The `iamb-compat` command line.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use iamb_core::datetime::civil_from_days;
use iamb_platform::ClipboardAccess;

use super::apps::{Os, apps_for};
use super::latency::{self, ProbeLink as _, ProbeProcess};
use super::probe::protocol::{Command as ProbeCommand, Reply};
use super::report::{compute, render_markdown};
use super::results::{ExceptionRecord, Record, Resolution, ResultSet, append, cell_key, now_secs};
use super::run::{self, Harness, ModeChoice, Options, StdUi};
use super::setup;
use super::types::{FIXED_DATE, render_expected_txt, types};
use super::typist::{SystemTypist, Typist};

/// Walks the owner of a real machine through the compatibility matrix (task 2.7).
#[derive(Debug, Parser)]
#[command(
    name = "iamb-compat",
    version,
    about = "Walks you through the Iamb compatibility matrix (20 apps x 5 snippet types per OS) and reports the result.",
    after_help = "Typical use:\n  iamb-compat setup --data-dir C:\\iamb-compat\n  iamb-compat run --os windows\n  iamb-compat report --results compat-windows.jsonl\n  iamb-compat latency"
)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Import the fixture snippets into a data directory and say how to start the service on it.
    #[command(after_help = "Example:\n  iamb-compat setup --data-dir C:\\iamb-compat")]
    Setup(SetupArgs),
    /// Walk through the matrix of one OS, app by app, and record each cell.
    #[command(
        after_help = "Results go to an append-only JSON-lines file; run again to resume.\n\nExample:\n  iamb-compat run --os windows\n  iamb-compat run --os windows --app W9 --again"
    )]
    Run(RunArgs),
    /// Print the results table (Markdown), the pass rate and what still needs an exception.
    #[command(
        after_help = "Example:\n  iamb-compat report --results compat-windows.jsonl > report.md"
    )]
    Report(ReportArgs),
    /// Record an exception for a failing cell (an override, a documented limitation or an issue).
    #[command(
        after_help = "Example:\n  iamb-compat exception --os windows --cell W9/T2 --resolution issue --issue https://example.org/1 --symptom \"newline commits the cell\" --cause \"Enter commits in edit mode\""
    )]
    Exception(ExceptionArgs),
    /// Print the expected text of every snippet type (the content of compat-expected.txt).
    #[command(after_help = "Example:\n  iamb-compat expected --date 2026-03-18")]
    Expected(ExpectedArgs),
    /// Measure the end-to-end latency (last key to expanded text) with the probe window, and
    /// write the `e2e-plain-p95` result.
    #[command(
        after_help = "Needs the latency snippets (`iamb-compat setup --latency`), the service running on them and the probe (`cargo build --release -p iamb-testkit --features probe --bin iamb-probe`).\n\nExample:\n  iamb-compat latency\n  iamb-compat latency --trials 50\n  iamb-compat latency --target browser --open"
    )]
    Latency(LatencyArgs),
}

/// Arguments of `setup`.
#[derive(Debug, Args)]
pub struct SetupArgs {
    /// The Iamb data directory to fill (created if missing). Use a directory of its own.
    #[arg(long, value_name = "DIR")]
    pub data_dir: PathBuf,
    /// Also import the 1,000 snippets of the latency run (`;laaa` to `;lbml`).
    #[arg(long)]
    pub latency: bool,
}

/// Arguments of `latency`.
#[derive(Debug, Args)]
pub struct LatencyArgs {
    /// What the abbreviations are typed into.
    #[arg(long, value_enum, default_value = "native")]
    pub target: TargetArg,
    /// The probe program (default: iamb-probe next to this program).
    #[arg(long, value_name = "FILE")]
    pub probe: Option<PathBuf>,
    /// Trials: 40% short, 40% medium, 20% long bodies (1000 is the KPI sample).
    #[arg(long, default_value_t = latency::LIBRARY_SIZE, value_name = "N")]
    pub trials: usize,
    /// The seed of the trial order (the snippets themselves are fixed).
    #[arg(long, default_value_t = latency::DEFAULT_ORDER_SEED)]
    pub seed: u64,
    /// Where the result goes (default: `IAMB_BENCH_RESULTS_DIR`, else `target/bench-results`).
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Seconds before the first trial.
    #[arg(long, default_value_t = 3, value_name = "SECONDS")]
    pub delay: u64,
    /// Milliseconds between trials.
    #[arg(long, default_value_t = 300, value_name = "MS")]
    pub gap: u64,
    /// Milliseconds the probe keeps checking the text after a match.
    #[arg(long, default_value_t = 300, value_name = "MS")]
    pub settle: u64,
    /// Milliseconds the probe waits for the last key of a trial.
    #[arg(long, default_value_t = 10_000, value_name = "MS")]
    pub key_wait: u64,
    /// Browser: open the page in the default browser.
    #[arg(long)]
    pub open: bool,
    /// Browser: the page's port on 127.0.0.1 (0: any free port).
    #[arg(long, default_value_t = 0)]
    pub port: u16,
}

/// The latency targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TargetArg {
    /// The native probe window (the KPI).
    Native,
    /// The probe's page in a browser.
    Browser,
}

/// Arguments of `run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Which matrix to walk.
    #[arg(long, value_enum)]
    pub os: Os,
    /// Only this app (`W9`, `M3`, ...).
    #[arg(long, value_name = "ID")]
    pub app: Option<String>,
    /// The results file (default: compat-<os>.jsonl in this directory).
    #[arg(long, value_name = "FILE")]
    pub results: Option<PathBuf>,
    /// Run cells again that already have a result (the new result replaces the old one).
    #[arg(long)]
    pub again: bool,
    /// Automatic, manual, or ask per app.
    #[arg(long, value_enum, default_value = "ask")]
    pub mode: ModeArg,
    /// Only these snippet types, comma separated (for example `1,2,3,4`).
    #[arg(long, value_delimiter = ',', value_name = "N")]
    pub types: Vec<u8>,
    /// Repetitions per cell.
    #[arg(long, default_value_t = run::DEFAULT_REPS, value_name = "N")]
    pub reps: u32,
    /// Seconds to switch to the app before typing starts.
    #[arg(long, default_value_t = 5, value_name = "SECONDS")]
    pub delay: u64,
    /// Milliseconds to wait after typing before reading the field back.
    #[arg(long, default_value_t = 1200, value_name = "MS")]
    pub settle: u64,
}

/// The mode choices.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ModeArg {
    /// Ask once per app.
    Ask,
    /// Automatic where possible.
    Auto,
    /// Always manual.
    Manual,
}

/// Arguments of `report`.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// The results file.
    #[arg(long, value_name = "FILE")]
    pub results: PathBuf,
    /// Which matrix (default: the one the file holds).
    #[arg(long, value_enum)]
    pub os: Option<Os>,
}

/// Arguments of `exception`.
#[derive(Debug, Args)]
pub struct ExceptionArgs {
    /// The results file (default: compat-<os>.jsonl).
    #[arg(long, value_name = "FILE")]
    pub results: Option<PathBuf>,
    /// The matrix.
    #[arg(long, value_enum)]
    pub os: Os,
    /// The failing cell, like `W9/T2`.
    #[arg(long, value_name = "APP/TYPE")]
    pub cell: String,
    /// How it is resolved.
    #[arg(long, value_enum)]
    pub resolution: ResolutionArg,
    /// What was seen.
    #[arg(long)]
    pub symptom: String,
    /// Why it happens.
    #[arg(long, default_value = "")]
    pub cause: String,
    /// The issue link, for an issue.
    #[arg(long)]
    pub issue: Option<String>,
    /// The date found, `YYYY-MM-DD` (default: today, UTC).
    #[arg(long)]
    pub found: Option<String>,
}

/// The resolution choices.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ResolutionArg {
    /// A per-app override; the cell must be re-run and pass.
    Override,
    /// A documented limitation with a workaround.
    Limitation,
    /// An upstream or Iamb issue.
    Issue,
}

/// Arguments of `expected`.
#[derive(Debug, Args)]
pub struct ExpectedArgs {
    /// The date shown in the template result.
    #[arg(long, default_value = FIXED_DATE)]
    pub date: String,
}

fn default_results(os: Os) -> PathBuf {
    PathBuf::from(format!("compat-{}.jsonl", os.name()))
}

fn utc_date() -> String {
    let days = i64::try_from(now_secs() / 86_400).unwrap_or(0);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn out(text: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{text}");
}

#[cfg(windows)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the same signature on every system"
)]
fn native_clipboard() -> Option<Box<dyn ClipboardAccess>> {
    Some(Box::new(iamb_platform_windows::Platform::new()))
}

#[cfg(target_os = "macos")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the same signature on every system"
)]
fn native_clipboard() -> Option<Box<dyn ClipboardAccess>> {
    Some(Box::new(iamb_platform_macos::Platform::new()))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn native_clipboard() -> Option<Box<dyn ClipboardAccess>> {
    None
}

/// Runs the tool with `args` (the first is the program name) and returns the exit status.
pub fn main_with<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return ExitCode::from(u8::try_from(code).unwrap_or(2));
        }
    };
    match execute(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "iamb-compat: {message}");
            ExitCode::FAILURE
        }
    }
}

fn execute(command: Command) -> Result<(), String> {
    match command {
        Command::Setup(a) => cmd_setup(&a),
        Command::Run(a) => cmd_run(&a),
        Command::Report(a) => cmd_report(&a),
        Command::Exception(a) => cmd_exception(&a),
        Command::Expected(a) => {
            out(render_expected_txt(&a.date)?.trim_end());
            Ok(())
        }
        Command::Latency(a) => cmd_latency(&a),
    }
}

#[cfg(not(feature = "store"))]
fn import_and_say(_a: &SetupArgs) -> Result<(), String> {
    Err("this build has no store support (feature `store`)".to_owned())
}

#[cfg(feature = "store")]
fn import_and_say(a: &SetupArgs) -> Result<(), String> {
    let done = setup::import_fixtures(&a.data_dir)?;
    out(&format!(
        "Fixture library in {}: {} snippet(s) imported, {} already there.",
        a.data_dir.display(),
        done.imported,
        done.skipped
    ));
    if a.latency {
        let done = setup::import_latency_fixtures(&a.data_dir)?;
        out(&format!(
            "Latency snippets: {} imported, {} already there (;laaa to ;lbml, immediate, exact case).",
            done.imported, done.skipped
        ));
    }
    Ok(())
}

fn cmd_setup(a: &SetupArgs) -> Result<(), String> {
    import_and_say(a)?;
    out(
        "The five snippets are ;cpt1 to ;cpt5 (immediate, exact case). Expected results: crates/iamb-testkit/fixtures/compat-expected.txt",
    );
    out("");
    for line in setup::start_instructions(&a.data_dir) {
        out(&line);
    }
    Ok(())
}

fn cmd_run(a: &RunArgs) -> Result<(), String> {
    let apps = apps_for(a.os)?;
    let kinds = types()?;
    let mut selected = Vec::new();
    for n in &a.types {
        let id = format!("T{n}");
        if !kinds.iter().any(|t| t.id == id) {
            return Err(format!("there is no snippet type {id}; use 1 to 5"));
        }
        selected.push(id);
    }
    let opts = Options {
        os: a.os,
        app: a.app.clone(),
        results: a.results.clone().unwrap_or_else(|| default_results(a.os)),
        mode: match a.mode {
            ModeArg::Ask => ModeChoice::Ask,
            ModeArg::Auto => ModeChoice::Auto,
            ModeArg::Manual => ModeChoice::Manual,
        },
        again: a.again,
        types: selected,
        reps: Some(a.reps),
        delay_secs: a.delay,
        settle_ms: a.settle,
    };
    let mut ui = StdUi;
    let native = Os::current() == Some(a.os);
    if !native {
        out(&format!(
            "Note: this system is not {}; the harness runs in manual mode only.",
            a.os.name()
        ));
    }
    let mut typist = if native { SystemTypist::native() } else { None };
    let clipboard = if native { native_clipboard() } else { None };
    let pause = |d: std::time::Duration| std::thread::sleep(d);
    let mut harness = Harness {
        ui: &mut ui,
        typist: typist.as_mut().map(|t| t as &mut dyn Typist),
        clipboard: clipboard.as_deref(),
        pause: &pause,
        utc_date: utc_date(),
    };
    let summary = run::run(&mut harness, &opts, &apps, &kinds)?;
    out(&format!(
        "\n{} cell(s) recorded, {} already done.{}",
        summary.recorded,
        summary.already_done,
        if summary.quit {
            " Stopped early; run again to resume."
        } else {
            ""
        }
    ));
    out(&format!(
        "Next: iamb-compat report --results {}",
        opts.results.display()
    ));
    Ok(())
}

fn default_probe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    Ok(exe.with_file_name(if cfg!(windows) {
        "iamb-probe.exe"
    } else {
        "iamb-probe"
    }))
}

fn open_in_browser(url: &str) -> Result<(), String> {
    let status = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
    } else {
        std::process::Command::new("open").arg(url).status()
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("could not open the browser ({s})")),
        Err(e) => Err(format!("could not open the browser: {e}")),
    }
}

fn cmd_latency(a: &LatencyArgs) -> Result<(), String> {
    if a.trials == 0 {
        return Err("--trials must be at least 1".to_owned());
    }
    let Some(mut typist) = SystemTypist::native() else {
        return Err(
            "the latency run types through the system's key sender: run it on Windows or macOS"
                .to_owned(),
        );
    };
    let program = match &a.probe {
        Some(p) => p.clone(),
        None => default_probe()?,
    };
    let mut args = Vec::new();
    if a.target == TargetArg::Browser {
        args.extend([
            "--browser".to_owned(),
            "--port".to_owned(),
            a.port.to_string(),
        ]);
    }
    let mut probe = ProbeProcess::spawn(&program, &args)?;
    let (surface, url) = match probe.call(&ProbeCommand::Hello)? {
        Reply::Hello { surface, url, .. } => (surface, url),
        other => return Err(format!("the probe answered {other:?}")),
    };
    if let Some(url) = &url {
        out(&format!("Probe page: {url}"));
        if a.open {
            open_in_browser(url)?;
        }
        out(
            "Open it in the browser under test (loopback only), click into the field, and keep its tab in front.",
        );
        match probe.call(&ProbeCommand::WaitReady {
            timeout_ms: 300_000,
        })? {
            Reply::Ok { .. } => out("The page is connected."),
            Reply::Error { message } => return Err(message),
            other => return Err(format!("the probe answered {other:?}")),
        }
    }
    let plan = latency::plan(a.trials, a.seed);
    out(&format!(
        "{} trials into the {surface} probe. Starting in {} s: do not touch the keyboard or the mouse.",
        plan.len(),
        a.delay
    ));
    std::thread::sleep(std::time::Duration::from_secs(a.delay));
    let pause = |d: std::time::Duration| std::thread::sleep(d);
    let opts = latency::DriveOptions {
        gap: std::time::Duration::from_millis(a.gap),
        settle: std::time::Duration::from_millis(a.settle),
        key_wait: std::time::Duration::from_millis(a.key_wait),
    };
    let outcome = latency::drive(&mut probe, &mut typist, &plan, &opts, &pause, &mut |l| {
        out(l);
    });
    let _ = probe.call(&ProbeCommand::Quit);
    let outcome = outcome?;
    let dir = a
        .out
        .clone()
        .unwrap_or_else(iamb_bench::results::default_results_dir);
    let written = latency::write_results(&outcome, &surface, &dir)?;
    let passed = outcome.trials.iter().filter(|t| t.sample.pass).count();
    out(&format!(
        "\n{passed} of {} trials matched within 2 s.",
        outcome.trials.len()
    ));
    for line in &written.lines {
        out(&format!(
            "{:<28} n={:<5} p95 {:>8.1} ms  {}",
            line.bench,
            line.count,
            ms(line.p95),
            if line.pass {
                "under 50 ms"
            } else {
                "NOT under 50 ms"
            }
        ));
    }
    if let Some(line) = written.lines.iter().find(|l| l.kpi.is_some()) {
        out(&format!(
            "KPI {}: {}",
            latency::KPI,
            if line.pass { "met" } else { "not met" }
        ));
    }
    out(&format!("Result: {}", written.result.display()));
    out(&format!("Trials: {}", written.trials.display()));
    Ok(())
}

#[allow(clippy::cast_precision_loss, reason = "a printed number")]
fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

fn cmd_report(a: &ReportArgs) -> Result<(), String> {
    let set = ResultSet::load(&a.results)?;
    let oses = match a.os {
        Some(os) => vec![os],
        None => set.oses(),
    };
    if oses.is_empty() {
        return Err(format!("{} holds no results", a.results.display()));
    }
    let kinds = types()?;
    for os in oses {
        let report = compute(os, &apps_for(os)?, &kinds, &set);
        out(&format!("## {} compatibility run\n", os.name()));
        out(render_markdown(&report).trim_end());
        out("");
    }
    Ok(())
}

fn cmd_exception(a: &ExceptionArgs) -> Result<(), String> {
    let path = a.results.clone().unwrap_or_else(|| default_results(a.os));
    let (app, typ) = a
        .cell
        .split_once('/')
        .ok_or("the cell must look like W9/T2")?;
    let apps = apps_for(a.os)?;
    if !apps.iter().any(|x| x.id == app) {
        return Err(format!("no app {app} in the {} matrix", a.os.name()));
    }
    if !types()?.iter().any(|t| t.id == typ) {
        return Err(format!("no snippet type {typ}"));
    }
    let set = ResultSet::load(&path)?;
    match set.cell(a.os, app, typ) {
        None => return Err(format!("{} has no result yet", cell_key(app, typ))),
        Some(c) if c.pass => {
            return Err(format!("{} passed; nothing to record", cell_key(app, typ)));
        }
        Some(_) => {}
    }
    let resolution = match a.resolution {
        ResolutionArg::Override => Resolution::Override,
        ResolutionArg::Limitation => Resolution::Limitation,
        ResolutionArg::Issue => Resolution::Issue,
    };
    if matches!(resolution, Resolution::Issue) && a.issue.is_none() {
        return Err("an issue exception needs --issue with the link".to_owned());
    }
    let id = set.next_exception_id(a.os);
    append(
        &path,
        &Record::Exception(ExceptionRecord {
            ts: now_secs(),
            os: a.os,
            id: id.clone(),
            cell: cell_key(app, typ),
            symptom: a.symptom.clone(),
            cause: a.cause.clone(),
            resolution,
            issue: a.issue.clone(),
            found: a.found.clone().unwrap_or_else(utc_date),
            retested: None,
        }),
    )?;
    out(&format!("Recorded {id} for {}.", cell_key(app, typ)));
    Ok(())
}
