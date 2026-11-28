//! `iamb-service`: the supervisor (`--supervise`) or the worker (`--worker`, the default).
//!
//! On macOS the installed supervisor is a login-item helper app inside `Iamb.app` (ADR-0021),
//! which the system starts without arguments; there, with neither `--supervise` nor `--worker`
//! given, the default is `--supervise` (`iamb_service::paths::is_login_item_helper`). The
//! `-psn_…` argument older systems pass to apps they launch is ignored.
//!
//! ```text
//! iamb-service --supervise [--ui-host PATH | --no-ui-host] [--data-dir PATH] [--platform native|mock]
//! iamb-service [--worker] [--data-dir PATH] [--platform native|mock] [--heartbeat-stdout]
//! iamb-service --diagnose-focus SECONDS [--data-dir PATH]
//! ```
//!
//! `--diagnose-focus SECONDS` (Windows) watches the focus for that long, from 1 to 600 seconds,
//! and writes for each focus change what the secure-field check saw and decided, to standard
//! output and to `logs/focus-diagnosis-<unix ms>.txt` in the data root
//! (`iamb_platform_windows::diagnose_focus`, which says what a line holds: no text, no window
//! title, but the applications in front and when). The last five such files are kept, each at
//! most 2 MB, and the first line of each says what it holds (`iamb_service::diagnosis`); the
//! path is printed at the start on standard output and standard error. It runs beside the running service and changes nothing: no keyboard hook, no key
//! sent, no endpoint. A release build has no console window, so standard output is seen only
//! when it is redirected, as PowerShell does with `| Out-Host`; the file is written either way.
//!
//! The supervisor also starts the UI host (`iamb-desktop`, installed beside it) and restarts it
//! after an abnormal exit (`iamb_service::supervisor`, "The UI host"); `--no-ui-host` starts none
//! (the UI host passes it when it starts the supervisor itself). With `--platform mock` none is
//! started unless `--ui-host` names one. `--ui-host` is a switch for tests and development: a
//! release build refuses it with the usage error, whatever the platform, because on macOS the
//! helper holds the user's keyboard permissions and must not start a program of the caller's
//! choosing (the 2026-11 beta review's finding 1, and its re-verification: allowing it with
//! `--platform mock` left the hole open, since a release build parsed `mock` too). On macOS the
//! installed helper does not start the UI host as its child at all: it has the system open
//! `Iamb.app` (`ui_host_plan`).
//!
//! A release build of the macOS helper runs for the account's default data root only
//! (`iamb_service::paths::choose_data_root`): it is built from the home directory the user
//! database gives for this account (`getpwuid_r`), not from `HOME`, which whoever starts the
//! helper chooses (`open --env`), and is used whatever `HOME` says; `--data-dir` and
//! `IAMB_DATA_DIR` naming another root are refused. A refusal is written to the log of the
//! default root as well as to standard error, which nobody reads at login (round 2's R2).
//!
//! `--platform mock` runs the worker against the in-memory mock platform: development and CI
//! only. A release build refuses it where the system has a native platform (Windows, macOS):
//! nothing shipped uses it, and the installed helper must not run as something other than the
//! keyboard helper the user granted. On Linux, which has no native platform, it stays the
//! default and the only choice. `--heartbeat-stdout` is passed by the supervisor: the
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

// No console window on Windows in release builds: the supervisor is the login item (the Run
// key starts it at every login, where a console window would show, and closing it would end the
// supervisor). The worker's heartbeat pipes are handed over explicitly and work without one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
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

const USAGE: &str = "usage: iamb-service [--supervise [--ui-host PATH | --no-ui-host] | --worker | --diagnose-focus SECONDS] [--data-dir PATH] [--platform native|mock] [--heartbeat-stdout]";

/// The longest `--diagnose-focus` run, in seconds.
const DIAGNOSE_MAX_SECONDS: u64 = 600;

/// What `--ui-host` and `--no-ui-host` asked for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum UiHostChoice {
    /// Neither: the installed UI host on the native platform, none on the mock one.
    #[default]
    Default,
    /// `--ui-host PATH`.
    Path(PathBuf),
    /// `--no-ui-host`.
    None,
}

#[derive(Debug, Default)]
struct Args {
    /// `--supervise` (true) or `--worker` (false); `None` when neither was given.
    supervise: Option<bool>,
    ui_host: UiHostChoice,
    data_dir: Option<PathBuf>,
    platform: Option<PlatformChoice>,
    heartbeat_stdout: bool,
    /// `--diagnose-focus SECONDS`.
    diagnose_focus: Option<u64>,
}

fn parse(args: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let mut out = Args::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--supervise") => out.supervise = Some(true),
            Some("--worker") => out.supervise = Some(false),
            Some(psn) if psn.starts_with("-psn_") => {}
            Some("--heartbeat-stdout") => out.heartbeat_stdout = true,
            Some("--diagnose-focus") => {
                let seconds = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse::<u64>().ok()))
                    .filter(|s| (1..=DIAGNOSE_MAX_SECONDS).contains(s))
                    .ok_or("--diagnose-focus needs a number of seconds from 1 to 600")?;
                out.diagnose_focus = Some(seconds);
            }
            Some("--no-ui-host") => out.ui_host = UiHostChoice::None,
            Some("--ui-host") => {
                out.ui_host =
                    UiHostChoice::Path(PathBuf::from(args.next().ok_or("--ui-host needs a path")?));
            }
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

/// `--diagnose-focus SECONDS` (module docs; the files: `iamb_service::diagnosis`).
#[cfg(windows)]
fn diagnose_focus(dirs: &DataDirs, seconds: u64) -> ExitCode {
    use iamb_service::diagnosis;
    let logs = dirs.logs();
    if let Err(e) = std::fs::create_dir_all(&logs) {
        return fail(&format!("cannot create the logs folder: {e}"));
    }
    diagnosis::prune(&logs);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = diagnosis::path_for(&logs, stamp);
    let mut file = match diagnosis::Capped::create(&path) {
        Ok(file) => file,
        Err(e) => return fail(&format!("cannot create {}: {e}", path.display())),
    };
    // On both streams: whichever the person sees (a release build has no console).
    let start = format!("writing to {}", path.display());
    say(&start);
    eprintln!("iamb-service: {start}");
    let mut out = |line: &str| {
        say(line);
        file.line(line);
    };
    match iamb_platform_windows::diagnose_focus(std::time::Duration::from_secs(seconds), &mut out) {
        Ok(()) => {
            say(&format!("done; the lines are in {}", path.display()));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("the focus diagnosis could not run: {e}")),
    }
}

/// `--diagnose-focus` exists on Windows only.
#[cfg(not(windows))]
fn diagnose_focus(_dirs: &DataDirs, _seconds: u64) -> ExitCode {
    fail("--diagnose-focus is for Windows")
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
    // Next, from the running code, before anything could replace the file it came from (the
    // installed-binary check; the strict review of finding 11, finding 1).
    let _ = OWN_SIGNING.set(iamb_ipc::read_own_signing());
    let args = match parse(std::env::args_os().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => return fail(&e),
    };
    let restricted = launched_from_helper() && !cfg!(debug_assertions);
    let dirs = match iamb_service::paths::choose_data_root(
        args.data_dir.as_deref(),
        |k| std::env::var_os(k),
        restricted,
        account_home().as_deref(),
    ) {
        iamb_service::paths::RootChoice::Use(root) => DataDirs::new(root),
        iamb_service::paths::RootChoice::Refuse { reason, log_to } => {
            if let Some(root) = log_to {
                let process = if args.supervise.unwrap_or_else(launched_as_login_item) {
                    "supervisor"
                } else {
                    "worker"
                };
                Logger::open(&DataDirs::new(root).logs(), process)
                    .error(format_args!("not started: {reason}"));
            }
            return fail(reason);
        }
    };
    let choice = args
        .platform
        .unwrap_or_else(PlatformChoice::default_for_build);
    if let UiHostChoice::Path(_) = &args.ui_host
        && !ui_host_switch_allowed(cfg!(debug_assertions))
    {
        return fail(&format!("--ui-host is for development builds; {USAGE}"));
    }
    if !platform_allowed(choice, cfg!(debug_assertions), HAS_NATIVE_PLATFORM) {
        return fail(&format!(
            "--platform mock is for development builds on this system; {USAGE}"
        ));
    }
    // The data root is the user's alone, wherever it is (§3.3; the 2.8 review's finding 20:
    // on Windows a root outside the profile would otherwise keep its parent's permissions).
    if let Err(e) = iamb_service::paths::create_private_dir(dirs.root()) {
        return fail(&format!(
            "cannot make the data root private to this user: {e}"
        ));
    }
    if let Some(seconds) = args.diagnose_focus {
        return diagnose_focus(&dirs, seconds);
    }
    if args.supervise.unwrap_or_else(launched_as_login_item) {
        return supervise(dirs, choice, &args.ui_host, &hardening);
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

/// True when this process is the macOS login-item helper started by the system (module docs).
fn launched_as_login_item() -> bool {
    cfg!(target_os = "macos") && launched_from_helper()
}

/// True when this executable lives in the macOS login-item helper's layout (ADR-0021).
fn launched_from_helper() -> bool {
    iamb_service::paths::canonical_exe()
        .is_ok_and(|exe| iamb_service::paths::is_login_item_helper(&exe))
}

/// Whether `--ui-host` may be used (module docs): development builds only.
const fn ui_host_switch_allowed(debug: bool) -> bool {
    debug
}

/// This system has a native platform (module docs, `--platform mock`).
const HAS_NATIVE_PLATFORM: bool = cfg!(any(windows, target_os = "macos"));

/// Whether `platform` may be used (module docs): the mock in development builds, or where the
/// system has no native platform.
fn platform_allowed(platform: PlatformChoice, debug: bool, has_native: bool) -> bool {
    platform == PlatformChoice::Native || debug || !has_native
}

/// The home directory the user database gives for this account (macOS; module docs).
fn account_home() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        iamb_platform_macos::account_home()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
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
    if let Some(signing) = OWN_SIGNING.get() {
        log.info(format_args!("installed-binary check: {signing}"));
    }
}

/// This process's own signing state, read first in `main` (`iamb_ipc::read_own_signing`).
static OWN_SIGNING: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// How the supervisor starts the UI host (`supervisor` module docs, "The UI host").
#[derive(Debug, Clone, PartialEq, Eq)]
enum UiHostPlan {
    /// None is started; the reason, for the log.
    None(&'static str),
    /// Started as a child: `--ui-host`, and the installed UI host on Windows and Linux.
    Child(PathBuf),
    /// `Iamb.app` opened through the system (macOS): the helper never starts it as its child.
    Open(PathBuf),
}

/// The UI host for a supervisor whose executable is `exe` (canonical): the one `--ui-host`
/// names (already allowed by `ui_host_switch_allowed`); none with `--no-ui-host` or on the mock
/// platform; otherwise the installed one. On macOS that is the main app around the helper,
/// opened through the system after `check_main_app`, and a development build outside the
/// helper layout starts the `iamb-desktop` beside it as a child; a release build outside it
/// starts none.
fn ui_host_plan(
    choice: &UiHostChoice,
    platform: PlatformChoice,
    exe: &Path,
    macos: bool,
    debug: bool,
) -> UiHostPlan {
    use iamb_service::paths::{check_main_app, main_app_of_helper, ui_host_beside};
    match choice {
        UiHostChoice::Path(p) => UiHostPlan::Child(p.clone()),
        UiHostChoice::None => UiHostPlan::None("--no-ui-host"),
        UiHostChoice::Default if platform != PlatformChoice::Native => {
            UiHostPlan::None("the mock platform")
        }
        UiHostChoice::Default if macos => match main_app_of_helper(exe) {
            Some(app) => match check_main_app(&app, exe) {
                Ok(()) => UiHostPlan::Open(app),
                Err(_) => UiHostPlan::None("the app around the helper failed its check"),
            },
            None if debug => ui_host_beside(exe).filter(|p| p.is_file()).map_or(
                UiHostPlan::None("no ui host beside this executable"),
                UiHostPlan::Child,
            ),
            None => UiHostPlan::None("not the installed helper"),
        },
        UiHostChoice::Default => ui_host_beside(exe).filter(|p| p.is_file()).map_or(
            UiHostPlan::None("no ui host beside this executable"),
            UiHostPlan::Child,
        ),
    }
}

/// The program that carries out `plan` for the data root of `dirs`.
fn ui_host_program(plan: UiHostPlan, dirs: &DataDirs) -> Option<supervisor::Program> {
    let root = dirs.root().as_os_str().to_owned();
    let marker = Some(dirs.ui_host_quit_marker());
    match plan {
        UiHostPlan::None(_) => None,
        UiHostPlan::Child(exe) => Some(supervisor::Program {
            exe,
            args: vec!["--data-dir".into(), root],
            quit_marker: marker,
            launcher: false,
        }),
        UiHostPlan::Open(app) => Some(supervisor::Program {
            exe: PathBuf::from("/usr/bin/open"),
            args: vec![
                "-g".into(),
                "-W".into(),
                "-a".into(),
                app.into_os_string(),
                "--args".into(),
                "--data-dir".into(),
                root,
            ],
            quit_marker: marker,
            launcher: true,
        }),
    }
}

fn supervise(
    dirs: DataDirs,
    choice: PlatformChoice,
    ui: &UiHostChoice,
    hardening: &Hardening,
) -> ExitCode {
    let log = Arc::new(Logger::open(&dirs.logs(), "supervisor"));
    crash_handling(&log, &dirs, "supervisor", hardening);
    let exe = match iamb_service::paths::canonical_exe() {
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
    let ui = match ui {
        // Resolved here, so that a relative path or a link names one fixed file.
        UiHostChoice::Path(p) => match iamb_service::paths::canonical_absolute(p) {
            Ok(p) if p.is_file() => UiHostChoice::Path(p),
            _ => return fail(&format!("--ui-host must name a file; {USAGE}")),
        },
        other => other.clone(),
    };
    let plan = ui_host_plan(
        &ui,
        choice,
        &exe,
        cfg!(target_os = "macos"),
        cfg!(debug_assertions),
    );
    if let UiHostPlan::None(why) = &plan {
        log.info(format_args!("no ui host to start: {why}"));
    }
    let ui_host = ui_host_program(plan, &dirs);
    let mut cfg = SupervisorConfig::new(dirs, exe, worker_args);
    cfg.ui_host = ui_host;
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
        // The supervisor never writes to our input: its end means the supervisor is gone, or
        // asked for a stop (a restart request, `supervisor` module docs).
        let watcher = Arc::clone(&log);
        let exit = worker_exit_handle(&worker);
        let _ = thread::Builder::new()
            .name("iamb-stdin".to_owned())
            .spawn(move || {
                let mut buf = [0_u8; 64];
                let mut input = io::stdin().lock();
                while matches!(input.read(&mut buf), Ok(n) if n > 0) {}
                watcher.info(format_args!(
                    "standard input ended (the supervisor is gone or asked for a stop); stopping"
                ));
                exit();
            });
    }
    if fault == Fault::HangOnce {
        thread::sleep(std::time::Duration::from_millis(300));
        #[cfg(debug_assertions)]
        worker.hang_engine();
    }
    let reason = worker.wait_exit(None).unwrap_or(ExitReason::Stop);
    let scrub = worker.scrubber();
    if !stop_within(worker, STOP_LIMIT) {
        // A thread that never ends must not keep the process (the 2026-11 beta review's
        // finding 10): the listener and the endpoint are stopped first, so nothing is read or
        // answered any more; the two buffers of typed characters are zeroed here instead.
        scrub();
        log.warn(format_args!(
            "the worker did not stop within {} s; exiting",
            STOP_LIMIT.as_secs()
        ));
    }
    match reason {
        ExitReason::Stop => 0,
        ExitReason::ListenerDead => EXIT_RESTART,
    }
}

/// How long the worker's orderly stop may take before the process exits without it.
const STOP_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Stops `worker` on a thread of its own; false when that has not finished within `limit`.
fn stop_within(worker: Worker, limit: std::time::Duration) -> bool {
    let (done, finished) = std::sync::mpsc::channel();
    let stopping = thread::Builder::new()
        .name("iamb-stop".to_owned())
        .spawn(move || {
            worker.stop();
            let _ = done.send(());
        });
    stopping.is_ok() && finished.recv_timeout(limit).is_ok()
}

/// A closure that asks `worker` to stop, usable from another thread.
fn worker_exit_handle(worker: &Worker) -> impl Fn() + Send + 'static {
    let handle = worker.exit_handle();
    move || handle.request(ExitReason::Stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER: &str = "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service";

    fn parsed(args: &[&str]) -> Result<Option<Args>, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn the_focus_diagnosis_takes_a_number_of_seconds() {
        let args = parsed(&["--diagnose-focus", "30"])
            .expect("parses")
            .expect("runs");
        assert_eq!(args.diagnose_focus, Some(30));
        assert_eq!(
            parsed(&["--diagnose-focus", "600"])
                .expect("parses")
                .expect("runs")
                .diagnose_focus,
            Some(600)
        );
        for bad in [
            &["--diagnose-focus"][..],
            &["--diagnose-focus", "0"],
            &["--diagnose-focus", "601"],
            &["--diagnose-focus", "ten"],
        ] {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parsed(&[]).expect("parses").expect("runs").diagnose_focus,
            None
        );
    }

    #[test]
    fn the_ui_host_switch_is_refused_by_a_release_build_on_either_platform() {
        assert!(!ui_host_switch_allowed(false));
        assert!(ui_host_switch_allowed(true));
    }

    #[test]
    fn the_mock_platform_is_refused_by_a_release_build_where_there_is_a_native_one() {
        use PlatformChoice::{Mock, Native};
        // Windows and macOS: development builds only.
        assert!(!platform_allowed(Mock, false, true));
        assert!(platform_allowed(Mock, true, true));
        // Linux has no native platform: the mock is its only choice.
        assert!(platform_allowed(Mock, false, false));
        for debug in [false, true] {
            for native in [false, true] {
                assert!(platform_allowed(Native, debug, native));
            }
        }
    }

    #[test]
    fn the_installed_helper_opens_the_app_and_never_spawns_it() {
        let native = PlatformChoice::Native;
        let helper = Path::new(HELPER);
        // The bundle is not on this disk, so its check fails: nothing is started, and in no
        // case is the program beside the helper started as a child.
        for debug in [false, true] {
            assert!(matches!(
                ui_host_plan(&UiHostChoice::Default, native, helper, true, debug),
                UiHostPlan::None(_)
            ));
        }
        // A release build outside the helper layout starts none on macOS.
        assert!(matches!(
            ui_host_plan(
                &UiHostChoice::Default,
                native,
                Path::new("/opt/iamb/iamb-service"),
                true,
                false
            ),
            UiHostPlan::None(_)
        ));
        assert!(matches!(
            ui_host_plan(&UiHostChoice::None, native, helper, true, true),
            UiHostPlan::None(_)
        ));
        assert!(matches!(
            ui_host_plan(
                &UiHostChoice::Default,
                PlatformChoice::Mock,
                helper,
                true,
                true
            ),
            UiHostPlan::None(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_checked_app_is_opened_through_the_system_and_its_end_read_from_the_marker() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let root = iamb_service::paths::canonical_absolute(dir.path()).expect("canonical");
        let app = root.join("Iamb.app");
        let helper =
            app.join("Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service");
        std::fs::create_dir_all(helper.parent().expect("parent")).expect("helper dir");
        std::fs::write(&helper, b"").expect("helper");
        std::fs::create_dir_all(app.join("Contents/MacOS")).expect("dir");
        std::fs::write(app.join("Contents/MacOS/iamb-desktop"), b"").expect("ui host");
        std::fs::write(
            app.join("Contents/Info.plist"),
            "<key>CFBundleIdentifier</key><string>org.iamb.Iamb</string>",
        )
        .expect("plist");
        let plan = ui_host_plan(
            &UiHostChoice::Default,
            PlatformChoice::Native,
            &helper,
            true,
            false,
        );
        assert_eq!(plan, UiHostPlan::Open(app.clone()));
        let dirs = DataDirs::new("/data/iamb");
        let program = ui_host_program(plan, &dirs).expect("a program");
        assert_eq!(program.exe, Path::new("/usr/bin/open"));
        let mut expected: Vec<OsString> = ["-g", "-W", "-a"].map(OsString::from).to_vec();
        expected.push(app.into_os_string());
        expected.extend(["--args", "--data-dir", "/data/iamb"].map(OsString::from));
        assert_eq!(program.args, expected);
        assert!(program.launcher);
        assert_eq!(
            program.quit_marker.as_deref(),
            Some(Path::new("/data/iamb/run/desktop.quit"))
        );
    }

    #[test]
    fn elsewhere_the_installed_ui_host_beside_the_supervisor_is_a_child() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let exe = dir.path().join("iamb-service");
        let host = dir.path().join(iamb_service::paths::UI_HOST_EXE);
        assert!(matches!(
            ui_host_plan(
                &UiHostChoice::Default,
                PlatformChoice::Native,
                &exe,
                false,
                false
            ),
            UiHostPlan::None(_)
        ));
        std::fs::write(&host, b"").expect("ui host");
        let plan = ui_host_plan(
            &UiHostChoice::Default,
            PlatformChoice::Native,
            &exe,
            false,
            false,
        );
        assert_eq!(plan, UiHostPlan::Child(host.clone()));
        let program = ui_host_program(plan, &DataDirs::new("/d")).expect("a program");
        assert!(!program.launcher);
        assert_eq!(program.exe, host);
        assert_eq!(program.args, ["--data-dir", "/d"].map(OsString::from));
    }
}
