//! `iamb-desktop`: the UI host.
//!
//! ```text
//! iamb-desktop [--data-dir PATH] [--open-library]
//!              [--measure-open RUNS | --measure-form RUNS | --measure-palette RUNS]
//!              [--budget-ms MS]
//! ```
//!
//! Started hidden at login by the service's supervisor (architecture §3.2). `--open-library`
//! opens the library window at start (development, and desktops without a tray).
//! `--measure-open` times the cold open of the library window and exits; `--measure-form` times
//! the fill-in form from the request to its first paint and exits, `--measure-palette` the
//! search palette from the shortcut to its first paint (`docs/test-strategy.md` §8).
//!
//! Before anything else the process opts out of the operating system's crash dumps, and its
//! panic hook writes a heap-free crash report and exits with status 70 (`iamb_desktop::crash`,
//! architecture §8 rule 9).

// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use iamb_desktop::app::{self, Options};
use iamb_desktop::crash;
use iamb_desktop::log::Logger;
use iamb_desktop::paths::DataDirs;

const USAGE: &str = "usage: iamb-desktop [--data-dir PATH] [--open-library] \
     [--measure-open RUNS | --measure-form RUNS | --measure-palette RUNS] [--budget-ms MS]";

struct Args {
    data_dir: Option<PathBuf>,
    measure_runs: Option<usize>,
    measure_form_runs: Option<usize>,
    measure_palette_runs: Option<usize>,
    budget_ms: Option<f64>,
    open_library: bool,
}

fn parse(args: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let mut out = Args {
        data_dir: None,
        measure_runs: None,
        measure_form_runs: None,
        measure_palette_runs: None,
        budget_ms: None,
        open_library: false,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut value = |what: &str| {
            args.next()
                .and_then(|v| v.into_string().ok())
                .ok_or_else(|| format!("{what} needs a value; {USAGE}"))
        };
        match arg.to_str() {
            Some("--data-dir") => out.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            Some("--open-library") => out.open_library = true,
            Some("--measure-open") => {
                let runs = value("--measure-open")?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--measure-open needs a positive number of runs")?;
                out.measure_runs = Some(runs);
            }
            Some("--measure-form") => {
                let runs = value("--measure-form")?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--measure-form needs a positive number of runs")?;
                out.measure_form_runs = Some(runs);
            }
            Some("--measure-palette") => {
                let runs = value("--measure-palette")?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--measure-palette needs a positive number of runs")?;
                out.measure_palette_runs = Some(runs);
            }
            Some("--budget-ms") => {
                let ms = value("--budget-ms")?
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .ok_or("--budget-ms needs a positive number")?;
                out.budget_ms = Some(ms);
            }
            Some("--version") => {
                say(&format!("iamb-desktop {}", iamb_desktop::VERSION));
                return Ok(None);
            }
            Some("--help" | "-h") => {
                say(USAGE);
                return Ok(None);
            }
            _ => return Err(format!("unknown argument; {USAGE}")),
        }
    }
    Ok(Some(out))
}

fn say(line: &str) {
    let _ = writeln!(io::stdout().lock(), "{line}");
}

fn fail(message: &str) -> ExitCode {
    eprintln!("iamb-desktop: {message}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    // First, before any other thread exists: no OS crash dump of this process's memory.
    let hardening = crash::harden_process();
    let args = match parse(std::env::args_os().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => return fail(&e),
    };
    let dirs = match DataDirs::resolve(args.data_dir.as_deref()) {
        Ok(dirs) => dirs,
        Err(e) => return fail(&e.to_string()),
    };
    let log = Arc::new(Logger::open(&dirs.logs(), crash::PROCESS));
    crash::install_panic_hook(&log, &dirs.logs());
    if hardening.complete() {
        log.info(format_args!("{}", hardening.summary()));
    } else {
        log.warn(format_args!("{}", hardening.summary()));
    }
    log.info(format_args!(
        "ui host {} starting, pid {}",
        iamb_desktop::VERSION,
        std::process::id()
    ));
    let _instance = match dirs.take_instance_lock() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            log.info(format_args!("another ui host is running; exiting"));
            return ExitCode::SUCCESS;
        }
        Err(e) => return fail(&format!("cannot take the instance lock: {e}")),
    };
    let options = Options {
        measure_runs: args.measure_runs,
        measure_form_runs: args.measure_form_runs,
        measure_palette_runs: args.measure_palette_runs,
        budget_ms: args.budget_ms,
        open_library: args.open_library,
    };
    match app::run(dirs, &options, &log) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            log.error(format_args!("the ui host did not start: {e}"));
            fail(&format!("the ui host did not start: {e}"))
        }
    }
}
