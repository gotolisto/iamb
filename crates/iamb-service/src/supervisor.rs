//! The supervisor (`iamb-service --supervise`, architecture §3.2, ADR-0011).
//!
//! It holds no hooks and no user data. It takes `run/supervisor.lock` (single instance),
//! starts the worker as a child process, and:
//!
//! - restarts it when it exits abnormally (any status but 0, or killed by a signal), after the
//!   backoff 0 s, 1 s, 5 s, then every 30 s; a worker that ran for [`HEALTHY_AFTER`] before
//!   failing starts the count again;
//! - kills and restarts it when no heartbeat arrives for 3 s. The worker's engine thread writes
//!   one byte to its standard output every 500 ms; a new worker gets [`STARTUP_GRACE`] for its
//!   first one, because it may wait up to 5 s for the store lock;
//! - stops when the worker exits with status 0 (it was asked to stop).
//!
//! The worker's standard input is a pipe that the supervisor never writes: when the supervisor
//! dies, the worker reads the end of it and exits, so a killed supervisor leaves no orphan that
//! holds the store lock.
//!
//! # Restart requests
//!
//! "Start it again" and "Restart the helper" in the UI host (UX specification §4.2, §4.6) ask for
//! a worker started afresh now, without waiting for a backoff or a heartbeat timeout. The UI host
//! creates the empty file [`RESTART_REQUEST`] in `run/`, the directory only the user can open
//! (0700 on Unix, a DACL for the user alone on Windows; architecture §3.3), and then starts the
//! supervisor, which exits at once when one is already running. The running supervisor looks for
//! the file every [`REQUEST_POLL`]: it removes it, stops the worker it watches (killed: a worker
//! asked over IPC to stop could not be told apart from one that exits for good) and starts a new
//! one at once, without counting a failure; while it waits out a backoff, it starts the worker at
//! once. A supervisor that starts removes a request left over from before. The file carries
//! nothing: the worst a process of the same user can do with it is what it can already do by
//! killing the worker. This is the minimal stand-in for the control pipe of ADR-0023, which
//! updates still need.
//!
//! # The UI host
//!
//! §3.2 has the supervisor start the UI host too, so that the menu-bar or tray icon appears after
//! login (the login item starts the supervisor alone). When [`SupervisorConfig::ui_host`] is set,
//! [`run`] starts it once it holds the lock, on a thread of its own, and supervises it by its exit
//! only (no heartbeat): an abnormal exit restarts it after the same backoff as the worker's, with
//! its own count; status 0 (the user quit it, or another UI host already runs for this data root
//! and this one stepped aside) ends its supervision. The UI host outlives a supervisor that stops:
//! it then says "Iamb is not running" and offers to start it again. `main` sets it to the
//! installed UI host ([`crate::paths::ui_host_beside`]) for the native platform, to the one
//! `--ui-host` names, and to none with `--no-ui-host` or the mock platform (tests and
//! development).
//!
//! # Hook points
//!
//! - **Sync agent.** §3.2 has the supervisor start and restart `iamb-sync` too. It will be
//!   supervised as the UI host is ([`supervise_by_exit`]).
//! - **Updates** (ADR-0023): the UI host will ask the supervisor, over a private pipe with the
//!   peer verification of ADR-0006, to stop the worker and to start it again.

use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child as Process, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use crate::log::Logger;
use crate::paths::{DataDirs, create_private_dir};

/// How long a running worker may go without a heartbeat.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a new worker has for its first heartbeat.
pub const STARTUP_GRACE: Duration = Duration::from_secs(10);

/// A worker that ran this long before failing resets the backoff.
pub const HEALTHY_AFTER: Duration = Duration::from_mins(1);

/// The restart delays (§3.2): before the first, second, third restart, and every later one.
pub const BACKOFF: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

/// The file in `run/` that asks for a worker started afresh (module docs, "Restart requests").
pub const RESTART_REQUEST: &str = "restart-worker";

/// How often the supervisor looks for [`RESTART_REQUEST`].
pub const REQUEST_POLL: Duration = Duration::from_millis(200);

/// The delay before restart number `failures` (1 for the first restart).
#[must_use]
pub fn backoff_delay(schedule: &[Duration], failures: usize) -> Duration {
    let i = failures
        .saturating_sub(1)
        .min(schedule.len().saturating_sub(1));
    schedule.get(i).copied().unwrap_or_default()
}

/// How the supervisor runs.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// The data root (for the lock and the log).
    pub dirs: DataDirs,
    /// The worker executable (normally this binary).
    pub worker: PathBuf,
    /// Its arguments (normally `--worker --heartbeat-stdout --data-dir … --platform …`).
    pub args: Vec<OsString>,
    /// Normally [`HEARTBEAT_TIMEOUT`].
    pub heartbeat_timeout: Duration,
    /// Normally [`STARTUP_GRACE`].
    pub startup_grace: Duration,
    /// Normally [`BACKOFF`].
    pub backoff: Vec<Duration>,
    /// Normally [`HEALTHY_AFTER`].
    pub healthy_after: Duration,
    /// The UI host to start and supervise by its exit (module docs, "The UI host"); none by
    /// default.
    pub ui_host: Option<Program>,
}

/// A program the supervisor starts: its executable and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The executable.
    pub exe: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
}

impl SupervisorConfig {
    /// The defaults for a worker started as `worker args…`.
    #[must_use]
    pub fn new(dirs: DataDirs, worker: PathBuf, args: Vec<OsString>) -> Self {
        Self {
            dirs,
            worker,
            args,
            heartbeat_timeout: HEARTBEAT_TIMEOUT,
            startup_grace: STARTUP_GRACE,
            backoff: BACKOFF.to_vec(),
            healthy_after: HEALTHY_AFTER,
            ui_host: None,
        }
    }
}

/// How the supervisor ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Another supervisor holds `run/supervisor.lock`.
    AlreadyRunning,
    /// The worker exited with status 0.
    Stopped,
}

/// Takes `run/supervisor.lock`, or `None` when another supervisor holds it. The file is
/// readable by the user only (`0600` on Unix, as the store's lock), also when an older build
/// created it with the default mode.
///
/// # Errors
///
/// I/O errors other than the lock being held.
pub fn take_lock(dirs: &DataDirs) -> io::Result<Option<File>> {
    create_private_dir(&dirs.run())?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(dirs.supervisor_lock())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// One supervised process.
struct Child {
    process: Process,
    /// Held open so the child does not read the end of its input while we live.
    _stdin: Option<ChildStdin>,
}

/// Why a worker run ended.
enum Ended {
    Exited(ExitStatus),
    Hung,
    /// The UI host asked for a restart (module docs, "Restart requests").
    Asked,
}

/// Takes the restart request, if one waits: true when there was one.
fn take_request(dirs: &DataDirs) -> bool {
    std::fs::remove_file(dirs.restart_request()).is_ok()
}

/// Sleeps for `delay`, or less when a restart request arrives; true when one did.
fn sleep_or_request(dirs: &DataDirs, delay: Duration) -> bool {
    let until = Instant::now() + delay;
    loop {
        if take_request(dirs) {
            return true;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        thread::sleep(left.min(REQUEST_POLL));
    }
}

fn start_worker(cfg: &SupervisorConfig) -> io::Result<(Child, std::sync::mpsc::Receiver<()>)> {
    let mut process = Command::new(&cfg.worker)
        .args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = process.stdin.take();
    let mut stdout = process
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("the worker has no standard output"))?;
    let (beat, beats) = channel();
    thread::Builder::new()
        .name("iamb-heartbeat".to_owned())
        .spawn(move || {
            let mut buf = [0_u8; 64];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 || beat.send(()).is_err() {
                    break;
                }
            }
        })?;
    Ok((
        Child {
            process,
            _stdin: stdin,
        },
        beats,
    ))
}

fn watch(
    child: &mut Child,
    beats: &std::sync::mpsc::Receiver<()>,
    cfg: &SupervisorConfig,
) -> io::Result<Ended> {
    let mut deadline = Instant::now() + cfg.startup_grace;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match beats.recv_timeout(left.min(REQUEST_POLL)) {
            Ok(()) => deadline = Instant::now() + cfg.heartbeat_timeout,
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                let _ = child.process.kill();
                let _ = child.process.wait();
                return Ok(Ended::Hung);
            }
            Err(RecvTimeoutError::Timeout) => {
                if take_request(&cfg.dirs) {
                    let _ = child.process.kill();
                    let _ = child.process.wait();
                    return Ok(Ended::Asked);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return child.process.wait().map(Ended::Exited);
            }
        }
    }
}

/// Starts the thread that supervises the UI host (module docs, "The UI host").
fn start_ui_host(program: Program, cfg: &SupervisorConfig, log: &Arc<Logger>) {
    let backoff = cfg.backoff.clone();
    let healthy_after = cfg.healthy_after;
    let thread_log = Arc::clone(log);
    if let Err(e) = thread::Builder::new()
        .name("iamb-ui-host".to_owned())
        .spawn(move || supervise_by_exit("ui host", &program, &backoff, healthy_after, &thread_log))
    {
        log.error(format_args!("cannot supervise the ui host: {e}"));
    }
}

/// Starts `program` and starts it again after an abnormal exit, with the backoff `schedule`,
/// until it exits with status 0. `name` is for the log.
pub fn supervise_by_exit(
    name: &str,
    program: &Program,
    schedule: &[Duration],
    healthy_after: Duration,
    log: &Logger,
) {
    let mut failures = 0_usize;
    loop {
        let started = Instant::now();
        let spawned = Command::new(&program.exe)
            .args(&program.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn();
        match spawned {
            Ok(mut child) => {
                let pid = child.id();
                log.info(format_args!("{name} started: pid {pid}"));
                match child.wait() {
                    Ok(status) if status.success() => {
                        log.info(format_args!("{name} {pid} exited; not restarted"));
                        return;
                    }
                    Ok(status) => {
                        log.warn(format_args!("{name} {pid} exited abnormally: {status}"));
                    }
                    Err(e) => log.error(format_args!("cannot wait for {name} {pid}: {e}")),
                }
            }
            Err(e) => log.error(format_args!("cannot start the {name}: {e}")),
        }
        if started.elapsed() >= healthy_after {
            failures = 0;
        }
        failures += 1;
        let delay = backoff_delay(schedule, failures);
        log.info(format_args!(
            "restarting the {name} in {} ms (failure {failures})",
            delay.as_millis()
        ));
        thread::sleep(delay);
    }
}

/// Runs the supervisor until the worker stops normally.
///
/// # Errors
///
/// I/O errors taking the lock.
pub fn run(cfg: &SupervisorConfig, log: &Arc<Logger>) -> io::Result<Outcome> {
    let Some(_lock) = take_lock(&cfg.dirs)? else {
        log.info(format_args!("another supervisor is running; exiting"));
        return Ok(Outcome::AlreadyRunning);
    };
    log.info(format_args!(
        "supervisor {} started, pid {}",
        crate::VERSION,
        std::process::id()
    ));
    // A request from before this supervisor: the worker it starts now is the fresh one.
    let _ = take_request(&cfg.dirs);
    if let Some(ui) = &cfg.ui_host {
        start_ui_host(ui.clone(), cfg, log);
    }
    // Hook point: start the sync agent here once it exists (module docs).
    let mut failures = 0_usize;
    let mut attempt = 0_u64;
    loop {
        attempt += 1;
        let started = Instant::now();
        match start_worker(cfg) {
            Ok((mut child, beats)) => {
                log.info(format_args!(
                    "worker started: pid {}, attempt {attempt}",
                    child.process.id()
                ));
                let pid = child.process.id();
                match watch(&mut child, &beats, cfg) {
                    Ok(Ended::Asked) => {
                        log.info(format_args!(
                            "restart asked: worker {pid} stopped, starting a new one"
                        ));
                        failures = 0;
                        continue;
                    }
                    Ok(Ended::Hung) => {
                        log.warn(format_args!(
                            "no heartbeat from worker {pid} for {} ms: killed",
                            cfg.heartbeat_timeout.as_millis()
                        ));
                    }
                    Ok(Ended::Exited(status)) if status.success() => {
                        log.info(format_args!("worker {pid} stopped; supervisor exits"));
                        return Ok(Outcome::Stopped);
                    }
                    Ok(Ended::Exited(status)) => {
                        log.warn(format_args!("worker {pid} exited abnormally: {status}"));
                    }
                    Err(e) => {
                        log.error(format_args!("cannot wait for worker {pid}: {e}"));
                    }
                }
            }
            Err(e) => {
                log.error(format_args!("cannot start the worker: {e}"));
            }
        }
        if started.elapsed() >= cfg.healthy_after {
            failures = 0;
        }
        failures += 1;
        let delay = backoff_delay(&cfg.backoff, failures);
        log.info(format_args!(
            "restarting the worker in {} ms (failure {failures})",
            delay.as_millis()
        ));
        if sleep_or_request(&cfg.dirs, delay) {
            log.info(format_args!("restart asked: starting the worker now"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_follows_the_architecture() {
        let delays: Vec<u64> = (1..=6)
            .map(|n| backoff_delay(&BACKOFF, n).as_secs())
            .collect();
        assert_eq!(delays, [0, 1, 5, 30, 30, 30]);
        assert_eq!(backoff_delay(&BACKOFF, 0), Duration::ZERO);
        assert_eq!(backoff_delay(&[], 3), Duration::ZERO);
    }

    #[test]
    fn a_restart_request_is_taken_once_and_cuts_a_wait_short() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        create_private_dir(&dirs.run()).expect("run dir");
        assert!(!take_request(&dirs));
        std::fs::write(dirs.restart_request(), b"").expect("request");
        assert!(take_request(&dirs));
        assert!(!take_request(&dirs));

        // Without a request the whole delay passes.
        let start = Instant::now();
        assert!(!sleep_or_request(&dirs, Duration::from_millis(250)));
        assert!(start.elapsed() >= Duration::from_millis(250));
        // With one, the wait ends at the next look.
        std::fs::write(dirs.restart_request(), b"").expect("request");
        let start = Instant::now();
        assert!(sleep_or_request(&dirs, Duration::from_secs(30)));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(!dirs.restart_request().exists());
    }

    #[test]
    fn the_lock_admits_one_supervisor() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        let first = take_lock(&dirs).expect("io").expect("free");
        assert!(take_lock(&dirs).expect("io").is_none());
        drop(first);
        assert!(take_lock(&dirs).expect("io").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        let mode = |dirs: &DataDirs| {
            std::fs::metadata(dirs.supervisor_lock())
                .expect("lock file")
                .permissions()
                .mode()
                & 0o777
        };
        drop(take_lock(&dirs).expect("io"));
        assert_eq!(mode(&dirs), 0o600);
        // A file an older build left with the default mode is tightened.
        std::fs::set_permissions(
            dirs.supervisor_lock(),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("chmod");
        drop(take_lock(&dirs).expect("io"));
        assert_eq!(mode(&dirs), 0o600);
    }
}
