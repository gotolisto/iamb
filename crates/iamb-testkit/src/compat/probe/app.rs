//! The `iamb-probe` program (feature `probe`): parses its arguments, opens the native window or
//! starts the browser page, and answers the driver on standard input and output.
//!
//! On macOS the native window needs the main thread, so the program runs under
//! `run_with_main_loop`, whose application event loop serves the window.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
#[cfg(any(windows, target_os = "macos"))]
use std::time::Duration;

use clap::Parser;

use super::browser::{BrowserSurface, PAGE_SLACK};
use super::serve::{Probe, ServeOptions, Surface, serve};

/// The probe window of the end-to-end latency measurement (test tooling, never shipped).
#[derive(Debug, Clone, Parser)]
#[command(
    name = "iamb-probe",
    version,
    about = "A window with one text control that times expansions for iamb-compat latency. It reads commands on standard input (one JSON object per line).",
    after_help = "Normally started by `iamb-compat latency`. By hand:\n  iamb-probe --self-test\n  iamb-probe --browser --port 8765"
)]
pub struct Args {
    /// Serve the browser page on 127.0.0.1 instead of opening the native window.
    #[arg(long)]
    pub browser: bool,
    /// The page's port (0: any free port).
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// Also write the sorted latencies here (one integer per line, ns) at each `report`.
    #[arg(long, value_name = "FILE")]
    pub samples: Option<PathBuf>,
    /// Type a short string into the native window with synthetic keys, print the sample and
    /// exit: checks the window and its stamps on this machine, without Iamb.
    #[arg(long, conflicts_with = "browser")]
    pub self_test: bool,
}

#[cfg(windows)]
use iamb_platform_windows::probe as os;

#[cfg(target_os = "macos")]
use iamb_platform_macos::probe as os;

/// The window of the OS crate as a [`Surface`].
#[cfg(any(windows, target_os = "macos"))]
struct Native {
    window: os::ProbeWindow,
}

#[cfg(any(windows, target_os = "macos"))]
impl Native {
    fn open(probe: &Arc<Probe>) -> Result<Self, String> {
        let into = Arc::clone(probe);
        let sink: os::Sink = Arc::new(move |event: os::Event<'_>| match event {
            os::Event::Key { at, ch } => into.key_at(at, ch),
            os::Event::Changed { at, text } => into.text_at(at, text),
        });
        os::ProbeWindow::open("Iamb probe", sink).map(|window| Self { window })
    }

    fn type_synthetic(&self, text: &str) -> Result<(), String> {
        self.window.type_synthetic(text)
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Surface for Native {
    fn kind(&self) -> &'static str {
        "native"
    }

    fn reset(&self) -> Result<bool, String> {
        self.window.reset()
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn say(line: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

#[cfg(any(windows, target_os = "macos"))]
fn self_test(probe: &Probe, native: &Native) -> Result<(), String> {
    use super::session::Trial;
    const TYPED: &str = "probe self test";
    if !native.reset()? {
        return Err("the probe window could not take the keyboard focus".to_owned());
    }
    probe.arm(
        Trial {
            cell: "self-test".to_owned(),
            typed: TYPED.to_owned(),
            expected: TYPED.to_owned(),
        },
        Duration::from_secs(5),
    )?;
    native.type_synthetic(TYPED)?;
    let sample = probe.wait(Duration::from_millis(200))?;
    say(&format!(
        "self-test: {}; T1 - T0 = {} us; still matching after 200 ms: {}",
        if sample.pass {
            "pass".to_owned()
        } else {
            format!(
                "FAIL ({})",
                sample
                    .failure
                    .map_or("unknown", super::session::Failure::as_str)
            )
        },
        sample.latency_ns / 1000,
        sample
            .stable
            .map_or("not checked", |s| if s { "yes" } else { "no" })
    ));
    if sample.pass {
        Ok(())
    } else {
        Err("the self-test failed".to_owned())
    }
}

fn open_native() -> Result<(Arc<Probe>, Box<dyn Surface>), String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let probe = Arc::new(Probe::new(Duration::ZERO));
        let native = Native::open(&probe)?;
        Ok((probe, Box::new(native)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(NO_WINDOW.to_owned())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
const NO_WINDOW: &str = "this system has no native probe window; use --browser";

fn run_self_test() -> Result<(), String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let probe = Arc::new(Probe::new(Duration::ZERO));
        let native = Native::open(&probe)?;
        self_test(&probe, &native)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(NO_WINDOW.to_owned())
    }
}

/// Runs the probe; returns the exit status.
fn run(args: &Args) -> u8 {
    if args.self_test {
        return match run_self_test() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("iamb-probe: {e}");
                1
            }
        };
    }
    let opened = if args.browser {
        let probe = Arc::new(Probe::new(PAGE_SLACK));
        BrowserSurface::start(Arc::clone(&probe), args.port)
            .map(|s| (probe, Box::new(s) as Box<dyn Surface>))
    } else {
        open_native()
    };
    let (probe, surface) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            eprintln!("iamb-probe: {e}");
            return 1;
        }
    };
    let opts = ServeOptions {
        samples_file: args.samples.clone(),
    };
    let stdin = std::io::stdin();
    match serve(
        &probe,
        surface.as_ref(),
        stdin.lock(),
        std::io::stdout(),
        &opts,
    ) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("iamb-probe: {e}");
            1
        }
    }
}

/// Runs `iamb-probe` with `args` (the program name first).
pub fn main_with(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    #[cfg(target_os = "macos")]
    if !args.browser {
        let code = iamb_platform_macos::run_with_main_loop(move || i32::from(run(&args)));
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    ExitCode::from(run(&args))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_consistent() {
        Args::command().debug_assert();
        let a = Args::try_parse_from(["iamb-probe", "--browser", "--port", "9"]).unwrap();
        assert!(a.browser && a.port == 9);
        assert!(Args::try_parse_from(["iamb-probe", "--browser", "--self-test"]).is_err());
    }
}
