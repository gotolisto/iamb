//! `iamb-service`: the supervisor (`--supervise`) or the worker (`--worker`, the default).
//!
//! ```text
//! iamb-service --supervise [--data-dir PATH] [--platform native|mock]
//! iamb-service [--worker] [--data-dir PATH] [--platform native|mock] [--heartbeat-stdout]
//! ```
//!
//! `--platform mock` runs the worker against the in-memory mock platform: development and CI
//! only (it is the only choice on Linux). `--heartbeat-stdout` is passed by the supervisor: the
//! worker then writes its heartbeat to standard output and exits when standard input ends.
//!
//! On macOS the native worker gives its main thread to the application event loop
//! (`iamb_platform_macos::run_with_main_loop`), which the listener's keyboard-layout reads and
//! the hotkeys need, and runs on a thread beside it. The supervisor and the mock worker do not
//! need the loop.
//!
//! Before anything else, both processes opt out of the operating system's crash dumps, and
//! their panic hook writes a heap-free crash report and exits with `EXIT_PANIC` instead of
//! aborting (`iamb_service::crash`, architecture §8 rule 9).

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

use iamb_service::crash::{self, Hardening};
use iamb_service::log::Logger;
use iamb_service::paths::DataDirs;
use iamb_service::platform::{self, PlatformChoice};
use iamb_service::supervisor::{self, SupervisorConfig};
use iamb_service::worker::{ExitReason, Worker, WorkerConfig};
use iamb_service::{EXIT_RESTART, VERSION};

const USAGE: &str = "usage: iamb-service [--supervise | --worker] [--data-dir PATH] [--platform native|mock] [--heartbeat-stdout]";

#[derive(Debug, Default)]
struct Args {
    supervise: bool,
    data_dir: Option<PathBuf>,
    platform: Option<PlatformChoice>,
    heartbeat_stdout: bool,
}

fn parse(args: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let mut out = Args::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--supervise") => out.supervise = true,
            Some("--worker") => out.supervise = false,
            Some("--heartbeat-stdout") => out.heartbeat_stdout = true,
            Some("--data-dir") => {
                out.data_dir = Some(PathBuf::from(args.next().ok_or("--data-dir needs a path")?));
            }
            Some("--platform") => {
                let value = args.next().ok_or("--platform needs native or mock")?;
                out.platform = Some(
                    value
                        .to_str()
                        .and_then(PlatformChoice::parse)
                        .ok_or("--platform must be native or mock")?,
                );
            }
            Some("--version") => {
                say(&format!("iamb-service {VERSION}"));
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
    ExitCode::from(complain(message))
}

/// Writes `message` to standard error; returns the exit status of a failure.
fn complain(message: &str) -> u8 {
    eprintln!("iamb-service: {message}");
    1
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
    // The data root is the user's alone, wherever it is (§3.3; the 2.8 review's finding 20:
    // on Windows a root outside the profile would otherwise keep its parent's permissions).
    if let Err(e) = iamb_service::paths::create_private_dir(dirs.root()) {
        return fail(&format!(
            "cannot make the data root private to this user: {e}"
        ));
    }
    let choice = args
        .platform
        .unwrap_or_else(PlatformChoice::default_for_build);
    if args.supervise {
        return supervise(dirs, choice, &hardening);
    }
    let supervised = args.heartbeat_stdout;
    #[cfg(target_os = "macos")]
    if choice == PlatformChoice::Native {
        let code = iamb_platform_macos::run_with_main_loop(move || {
            i32::from(work(&dirs, choice, supervised, &hardening))
        });
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    ExitCode::from(work(&dirs, choice, supervised, &hardening))
}

/// The panic hook (by location only: a panic message could quote data) and the log line that
/// says which crash-dump opt-outs took effect.
fn crash_handling(log: &Arc<Logger>, dirs: &DataDirs, process: &'static str, h: &Hardening) {
    crash::install_panic_hook(log, &dirs.logs(), process);
    if h.complete() {
        log.info(format_args!("{}", h.summary()));
    } else {
        log.warn(format_args!("{}", h.summary()));
    }
}

fn supervise(dirs: DataDirs, choice: PlatformChoice, hardening: &Hardening) -> ExitCode {
    let log = Arc::new(Logger::open(&dirs.logs(), "supervisor"));
    crash_handling(&log, &dirs, "supervisor", hardening);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return fail(&format!("cannot find this executable: {e}")),
    };
    let worker_args: Vec<OsString> = vec![
        "--worker".into(),
        "--heartbeat-stdout".into(),
        "--data-dir".into(),
        dirs.root().as_os_str().to_owned(),
        "--platform".into(),
        choice.as_str().into(),
    ];
    let cfg = SupervisorConfig::new(dirs, exe, worker_args);
    match supervisor::run(&cfg, &log) {
        Ok(supervisor::Outcome::Stopped) => ExitCode::SUCCESS,
        Ok(supervisor::Outcome::AlreadyRunning) => {
            eprintln!("iamb-service: another supervisor is running");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("supervisor failed: {e}")),
    }
}

/// Test faults, read only by debug builds (`IAMB_TEST_FAULT`): `exit-at-start` exits with
/// status 1 at once; `panic-at-start` panics on a named thread with a message that must not
/// reach any file; `hang-once` stops the engine thread shortly after the first start in a
/// data root (a marker file in `run/` keeps later starts healthy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    ExitAtStart,
    PanicAtStart,
    HangOnce,
}

fn fault(dirs: &DataDirs) -> Fault {
    if !cfg!(debug_assertions) {
        return Fault::None;
    }
    match std::env::var("IAMB_TEST_FAULT").as_deref() {
        Ok("exit-at-start") => Fault::ExitAtStart,
        Ok("panic-at-start") => Fault::PanicAtStart,
        Ok("hang-once") => {
            let marker = dirs.run().join("test-fault-done");
            if marker.exists() || std::fs::write(&marker, b"").is_err() {
                Fault::None
            } else {
                Fault::HangOnce
            }
        }
        _ => Fault::None,
    }
}

/// The worker; returns its exit status.
fn work(dirs: &DataDirs, choice: PlatformChoice, supervised: bool, hardening: &Hardening) -> u8 {
    let log = Arc::new(Logger::open(&dirs.logs(), "worker"));
    crash_handling(&log, dirs, "worker", hardening);
    let _ = iamb_service::paths::create_private_dir(&dirs.run());
    let fault = fault(dirs);
    match fault {
        Fault::ExitAtStart => {
            log.warn(format_args!("test fault: exiting at start"));
            return 1;
        }
        Fault::PanicAtStart => {
            log.warn(format_args!("test fault: panicking at start"));
            let _ = thread::Builder::new()
                .name("iamb-test-fault".to_owned())
                .spawn(|| panic!("test fault zqxpaniccanary"))
                .map(thread::JoinHandle::join);
            return 1;
        }
        Fault::None | Fault::HangOnce => {}
    }
    let platform = match platform::select(choice) {
        Ok(p) => p,
        Err(e) => return complain(&e),
    };
    if choice == PlatformChoice::Mock {
        log.warn(format_args!(
            "running against the mock platform (development only): no real keyboard"
        ));
    }
    let mut config = WorkerConfig::new(dirs.clone(), platform, iamb_ipc::native_endpoint());
    config.logger = Arc::clone(&log);
    if supervised {
        config.heartbeat = Box::new(|| {
            let mut out = io::stdout().lock();
            let _ = out.write_all(b".");
            let _ = out.flush();
        });
    }
    let worker = match Worker::start(config) {
        Ok(w) => w,
        Err(e) => {
            log.error(format_args!("worker did not start: {e}"));
            return complain(&e.to_string());
        }
    };
    if supervised {
        // The supervisor never writes to our input: its end means the supervisor is gone.
        let watcher = Arc::clone(&log);
        let exit = worker_exit_handle(&worker);
        let _ = thread::Builder::new()
            .name("iamb-stdin".to_owned())
            .spawn(move || {
                let mut buf = [0_u8; 64];
                let mut input = io::stdin().lock();
                while matches!(input.read(&mut buf), Ok(n) if n > 0) {}
                watcher.info(format_args!("the supervisor is gone; stopping"));
                exit();
            });
    }
    if fault == Fault::HangOnce {
        thread::sleep(std::time::Duration::from_millis(300));
        #[cfg(debug_assertions)]
        worker.hang_engine();
    }
    let reason = worker.wait_exit(None).unwrap_or(ExitReason::Stop);
    worker.stop();
    match reason {
        ExitReason::Stop => 0,
        ExitReason::ListenerDead => EXIT_RESTART,
    }
}

/// A closure that asks `worker` to stop, usable from another thread.
fn worker_exit_handle(worker: &Worker) -> impl Fn() + Send + 'static {
    let handle = worker.exit_handle();
    move || handle.request(ExitReason::Stop)
}
