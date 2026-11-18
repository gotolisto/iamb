//! The `iamb-soak` command line.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use super::canary::{self, Canary, Location};
use super::driver::{self, Kills, Options};
use super::host::{self, HostDirs};
use super::netcheck;
use super::parse_duration;

/// The soak harness and canary privacy scan (`docs/test-strategy.md` §5).
#[derive(Debug, Parser)]
#[command(
    name = "iamb-soak",
    version,
    about = "Runs the Iamb service on the mock platform for minutes to days under a seeded key stream, then scans for canaries and applies the pass rules.",
    after_help = "Typical use:\n  iamb-soak run --duration 30m --out target/soak/ci --net-check .\n  iamb-soak run --duration 20s --library 300 --keys-per-second 60 --sample-every 2s --edit-every 5s\n  iamb-soak scan --canaries target/soak/ci/input/canaries.json --location capture=/lab/capture.pcapng"
)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a soak and write its samples, scan and report under --out. Exit status 0 when
    /// every rule passes, 1 when one fails, 2 when the run could not be set up.
    Run(RunArgs),
    /// Scan locations for the canaries of an earlier run (lab: the user profile, crash dumps,
    /// the packet capture).
    Scan(ScanArgs),
    /// Check that iamb-service links no networking crate (`cargo tree`).
    NetCheck(NetCheckArgs),
    /// Internal: the supervisor process of a run.
    #[command(hide = true)]
    ServeSupervisor(HostArgs),
    /// Internal: the worker process of a run.
    #[command(hide = true)]
    ServeWorker(HostArgs),
}

/// Arguments of `run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// How long to run: 72h in the lab, 30m on CI, seconds for a smoke run.
    #[arg(long, default_value = "72h", value_parser = parse_duration)]
    pub duration: Duration,
    /// Where the run's files go (default: target/soak/<unix ms>). Must not hold an earlier run.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// The seed of the key stream, the library and the edits (default: from the clock;
    /// printed and kept in the report).
    #[arg(long)]
    pub seed: Option<u64>,
    /// Fixture snippets (at most 17576).
    #[arg(long, default_value_t = 10_000)]
    pub library: usize,
    /// Average typing rate; bursts go five times faster. Raise it for a short local run.
    #[arg(long, default_value_t = 3.0)]
    pub keys_per_second: f64,
    /// Sample interval.
    #[arg(long, default_value = "60s", value_parser = parse_duration)]
    pub sample_every: Duration,
    /// Library edit interval (one edit through IPC each time).
    #[arg(long, default_value = "60s", value_parser = parse_duration)]
    pub edit_every: Duration,
    /// Hard-kill the worker this often: `auto` (the run length / 12, at least 90 s), `off`, or
    /// a duration. Kills closer than 60 s apart meet the supervisor's backoff.
    #[arg(long, default_value = "auto")]
    pub kill_every: String,
    /// Canary rounds; each types one canary per context.
    #[arg(long, default_value_t = 3)]
    pub canary_rounds: usize,
    /// Also check, in this workspace, that iamb-service links no networking crate.
    #[arg(long, value_name = "WORKSPACE")]
    pub net_check: Option<PathBuf>,
    /// Another location to scan for canaries (repeatable).
    #[arg(long, value_name = "PATH")]
    pub scan: Vec<PathBuf>,
    /// No progress lines on standard error.
    #[arg(long)]
    pub quiet: bool,
}

/// Arguments of `scan`.
#[derive(Debug, Args)]
pub struct ScanArgs {
    /// The canaries file of a run (`<out>/input/canaries.json`).
    #[arg(long, value_name = "FILE")]
    pub canaries: PathBuf,
    /// A location as LABEL=PATH (repeatable).
    #[arg(long, value_name = "LABEL=PATH", required = true)]
    pub location: Vec<String>,
    /// Write the scan report here (JSON) as well as printing the summary.
    #[arg(long, value_name = "FILE")]
    pub report: Option<PathBuf>,
}

/// Arguments of `net-check`.
#[derive(Debug, Args)]
pub struct NetCheckArgs {
    /// The workspace root.
    #[arg(long, default_value = ".")]
    pub workspace: PathBuf,
}

/// Arguments of the internal service processes.
#[derive(Debug, Args)]
pub struct HostArgs {
    /// The data root.
    #[arg(long)]
    pub data_dir: PathBuf,
    /// The control endpoint's directory.
    #[arg(long)]
    pub control_dir: PathBuf,
    /// The IPC tap's directory.
    #[arg(long)]
    pub tap_dir: PathBuf,
}

fn say(line: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

fn kills(text: &str) -> Result<Kills, String> {
    match text {
        "auto" => Ok(Kills::Auto),
        "off" | "0" => Ok(Kills::Off),
        other => parse_duration(other).map(Kills::Every),
    }
}

fn run(args: RunArgs) -> ExitCode {
    let seed = args
        .seed
        .unwrap_or_else(|| super::unix_ms() ^ 0x5EED_0F1A_3B00);
    let out = args
        .out
        .unwrap_or_else(|| PathBuf::from("target/soak").join(super::unix_ms().to_string()));
    let kills = match kills(&args.kill_every) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("iamb-soak: --kill-every: {e}");
            return ExitCode::from(2);
        }
    };
    if !(args.keys_per_second > 0.0 && args.keys_per_second.is_finite()) {
        eprintln!("iamb-soak: --keys-per-second must be above 0");
        return ExitCode::from(2);
    }
    if args.sample_every.is_zero() || args.edit_every.is_zero() || args.duration.is_zero() {
        eprintln!("iamb-soak: --duration, --sample-every and --edit-every must be above 0");
        return ExitCode::from(2);
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("iamb-soak: cannot find this executable: {e}");
            return ExitCode::from(2);
        }
    };
    let opts = Options {
        exe,
        duration: args.duration,
        out: out.clone(),
        seed,
        library: args.library,
        keys_per_second: args.keys_per_second,
        sample_every: args.sample_every,
        edit_every: args.edit_every,
        kills,
        canary_rounds: args.canary_rounds,
        net_check: args.net_check,
        extra_scan: args.scan,
        quiet: args.quiet,
    };
    match driver::run(&opts) {
        Ok(report) => {
            let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap_or_default();
            say(summary.trim_end());
            say(&format!("files: {}", out.display()));
            if report.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("iamb-soak: {e}");
            ExitCode::from(2)
        }
    }
}

fn scan(args: &ScanArgs) -> ExitCode {
    let canaries: Vec<Canary> = match std::fs::read(&args.canaries)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("iamb-soak: cannot read {}: {e}", args.canaries.display());
            return ExitCode::from(2);
        }
    };
    let mut locations = Vec::new();
    for l in &args.location {
        let Some((label, path)) = l.split_once('=') else {
            eprintln!("iamb-soak: --location must be LABEL=PATH");
            return ExitCode::from(2);
        };
        locations.push(Location {
            label: label.to_owned(),
            path: PathBuf::from(path),
        });
    }
    let report = canary::scan(&canaries, &locations, std::slice::from_ref(&args.canaries));
    if let Some(path) = &args.report {
        let ok = serde_json::to_vec_pretty(&report)
            .map_err(|e| e.to_string())
            .and_then(|b| std::fs::write(path, b).map_err(|e| e.to_string()));
        if let Err(e) = ok {
            eprintln!("iamb-soak: cannot write {}: {e}", path.display());
            return ExitCode::from(2);
        }
    }
    for l in &report.locations {
        say(&format!(
            "{:<16} {:>6} files {:>12} bytes{}",
            l.label,
            l.files,
            l.bytes,
            if l.exists { "" } else { "  (absent)" }
        ));
    }
    for h in &report.hits {
        say(&format!(
            "HIT canary #{} ({}) as {} in {}/{} at byte {}",
            h.canary,
            h.context.as_str(),
            h.form,
            h.location,
            h.file,
            h.offset
        ));
    }
    for u in &report.unreadable {
        say(&format!("UNREADABLE {u}"));
    }
    say(&format!(
        "{} canaries, {} patterns, {} hits",
        report.canaries,
        report.patterns,
        report.hits.len()
    ));
    if report.hits.is_empty() && report.unreadable.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs `iamb-soak` with `args` (the program name first).
pub fn main_with(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    match cli.command {
        Command::Run(args) => run(args),
        Command::Scan(args) => scan(&args),
        Command::NetCheck(args) => {
            let check = netcheck::run(&args.workspace);
            say(&check.command);
            match (&check.error, check.denied.is_empty()) {
                (Some(e), _) => say(&format!("could not check: {e}")),
                (None, true) => say(&format!("{} packages, no networking crate", check.packages)),
                (None, false) => say(&format!("networking crates: {}", check.denied.join(", "))),
            }
            if check.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::ServeSupervisor(a) => host::serve_supervisor(&HostDirs {
            data: &a.data_dir,
            control: &a.control_dir,
            tap: &a.tap_dir,
        }),
        Command::ServeWorker(a) => host::serve_worker(&HostDirs {
            data: &a.data_dir,
            control: &a.control_dir,
            tap: &a.tap_dir,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn kill_schedules_parse() {
        assert_eq!(kills("auto"), Ok(Kills::Auto));
        assert_eq!(kills("off"), Ok(Kills::Off));
        assert_eq!(kills("5m"), Ok(Kills::Every(Duration::from_mins(5))));
        assert!(kills("soon").is_err());
    }
}
