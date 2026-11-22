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
//! supervisor, which exits when one is already running (it tries for the lock for
//! [`LOCK_PATIENCE`] first, so that a request made while an old supervisor is still exiting finds
//! a new one). The running supervisor looks for the file every [`REQUEST_POLL`]: it removes it,
//! stops the worker it watches in order (it closes the worker's standard input, on which the
//! worker stops as when the supervisor is gone, restoring the clipboard and releasing what it
//! holds; it is killed only if it has not exited after [`STOP_GRACE`]) and starts a new one at
//! once, without counting a failure, whatever the old one's exit status: the supervisor asked,
//! so a status 0 here is not "stopped for good". While it waits out a backoff, it starts the
//! worker at once. A supervisor that starts removes a request left over from before. The file
//! carries nothing: the worst a process of the same user can do with it is what it can already
//! do by killing the worker.
//!
//! This is the minimal stand-in for the control pipe of ADR-0023, without that pipe's peer
//! verification (architecture §3.2, §8). It means "restart the worker" and nothing else; a stop,
//! or anything an update needs, waits for the pipe.
//!
//! # The UI host
//!
//! §3.2 has the supervisor start the UI host too, so that the menu-bar or tray icon appears after
//! login (the login item starts the supervisor alone). When [`SupervisorConfig::ui_host`] is set,
//! [`run`] starts it once it holds the lock, on a thread of its own, and supervises it by its end
//! only (no heartbeat): an abnormal end restarts it after the same backoff as the worker's, with
//! its own count. These end its supervision ([`UiHostEnd`]):
//!
//! - a quit: the UI host leaves `run/desktop.quit` when it ends on purpose (the tray's "Quit", or
//!   another UI host already runs for this data root and this one stepped aside). The supervisor
//!   removes the marker before each start. Where the exit status is the UI host's own, status 0
//!   counts as a quit as well;
//! - a usage error, status [`EXIT_USAGE`] (64): the UI host cannot run with what it was given (a
//!   data root that is not UTF-8), and never will;
//! - an end by `SIGTERM` (Unix): someone ended it on purpose (a task manager, an installer that
//!   replaces it), so it is not started again behind their back;
//! - [`MAX_QUICK_FAILURES`] starts in a row that each failed within [`QUICK_FAILURE`] (it cannot
//!   start: a missing web view runtime, a broken installation). This is logged as an error.
//!
//! On macOS the UI host is never the helper's child (the 2026-11 beta review's finding 1): the
//! helper holds the user's Accessibility and Input Monitoring permissions, and the system judges
//! a program the helper spawns as the helper. So there the supervisor has the system start
//! `Iamb.app`, as the UI host has the system start the helper (`/usr/bin/open -g -W -a <app>
//! --args --data-dir <root>`; `-W` waits for the app's end), and the app is its own responsible
//! process. `open` does not pass the app's exit status on ([`Program::launcher`]), so there only
//! the quit marker tells a quit from a crash. `main` checks the app first
//! ([`crate::paths::check_main_app`]).
//!
//! The UI host outlives a supervisor that stops: it then says "Iamb is not running" and offers
//! to start it again. `main` sets it to the installed UI host ([`crate::paths::ui_host_beside`],
//! [`crate::paths::main_app_of_helper`]) for the native platform, to the one `--ui-host` names
//! (development builds and the mock platform only), and to none with `--no-ui-host` or the mock
//! platform (tests and development).
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

/// How long a worker asked to stop (module docs, "Restart requests") has before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(1);

/// How long a starting supervisor tries for the lock before it leaves it to the one that
/// holds it (module docs, "Restart requests").
pub const LOCK_PATIENCE: Duration = Duration::from_secs(1);

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

/// A program the supervisor starts: its executable and arguments, and how its end is read
/// (module docs, "The UI host").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The executable.
    pub exe: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// The file the program leaves when it ends on purpose; removed before each start.
    pub quit_marker: Option<PathBuf>,
    /// `exe` is a launcher that starts the program through the system and waits for it
    /// (`open -W`): its exit status is not the program's, so only the marker tells a quit.
    pub launcher: bool,
}

impl Program {
    /// `exe args…`, started as a child, with no quit marker.
    #[must_use]
    pub fn child(exe: PathBuf, args: Vec<OsString>) -> Self {
        Self {
            exe,
            args,
            quit_marker: None,
            launcher: false,
        }
    }
}

/// The exit status of a usage error (`EX_USAGE`): the UI host cannot run with the arguments it
/// was given, so it is not started again (module docs, "The UI host").
pub const EXIT_USAGE: i32 = 64;

/// A start that ended within this long counts as one that could not start.
pub const QUICK_FAILURE: Duration = Duration::from_secs(1);

/// After this many such starts in a row, the UI host is given up on (module docs).
pub const MAX_QUICK_FAILURES: usize = 5;

/// Why the supervision of the UI host ended (module docs, "The UI host").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiHostEnd {
    /// It quit on purpose.
    Quit,
    /// It exited with [`EXIT_USAGE`].
    UsageError,
    /// It was ended by `SIGTERM`.
    Terminated,
    /// [`MAX_QUICK_FAILURES`] starts in a row failed at once.
    GaveUp,
}

/// How one run of a supervised program ended, as [`supervise_by_exit`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunEnd {
    /// Not restarted, for this reason.
    Final(UiHostEnd),
    /// An abnormal end: restarted.
    Failed,
}

/// Reads the end of one run: `status` (`None`: it could not be started or waited for),
/// whether the quit marker was found, and whether `status` is a launcher's.
fn judge(status: Option<ExitStatus>, marker: bool, launcher: bool) -> RunEnd {
    if marker {
        return RunEnd::Final(UiHostEnd::Quit);
    }
    let Some(status) = status else {
        return RunEnd::Failed;
    };
    if launcher {
        // `open -W` returns when the app ends, whatever its status: without the marker that
        // is a crash, and a failing `open` is a failed start.
        return RunEnd::Failed;
    }
    if status.success() {
        return RunEnd::Final(UiHostEnd::Quit);
    }
    if status.code() == Some(EXIT_USAGE) {
        return RunEnd::Final(UiHostEnd::UsageError);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // SIGTERM: ended on purpose by someone.
        if status.signal() == Some(15) {
            return RunEnd::Final(UiHostEnd::Terminated);
        }
    }
    RunEnd::Failed
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

/// Takes the lock as [`take_lock`] does, trying again for up to `patience` while another
/// supervisor holds it (module docs, "Restart requests").
///
/// # Errors
///
/// As [`take_lock`].
pub fn take_lock_within(dirs: &DataDirs, patience: Duration) -> io::Result<Option<File>> {
    let until = Instant::now() + patience;
    loop {
        if let Some(lock) = take_lock(dirs)? {
            return Ok(Some(lock));
        }
        if Instant::now() >= until {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// One supervised process.
struct Child {
    process: Process,
    /// Held open so the child does not read the end of its input while we live; closed to ask
    /// it to stop.
    stdin: Option<ChildStdin>,
}

impl Child {
    /// Asks the worker to stop by closing its input, and kills it if it has not exited after
    /// `grace` (module docs, "Restart requests").
    fn stop_in_order(&mut self, grace: Duration) {
        drop(self.stdin.take());
        let until = Instant::now() + grace;
        while Instant::now() < until {
            match self.process.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
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
    Ok((Child { process, stdin }, beats))
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
                    child.stop_in_order(STOP_GRACE);
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
        .spawn(move || {
            supervise_by_exit("ui host", &program, &backoff, healthy_after, &thread_log);
        })
    {
        log.error(format_args!("cannot supervise the ui host: {e}"));
    }
}

/// Starts `program` and starts it again after an abnormal end, with the backoff `schedule`,
/// until one of the ends of [`UiHostEnd`] (module docs, "The UI host"), which it returns.
/// `name` is for the log.
pub fn supervise_by_exit(
    name: &str,
    program: &Program,
    schedule: &[Duration],
    healthy_after: Duration,
    log: &Logger,
) -> UiHostEnd {
    let mut failures = 0_usize;
    let mut quick = 0_usize;
    loop {
        if let Some(marker) = &program.quit_marker {
            let _ = std::fs::remove_file(marker);
        }
        let started = Instant::now();
        let spawned = Command::new(&program.exe)
            .args(&program.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn();
        let status = match spawned {
            Ok(mut child) => {
                let pid = child.id();
                log.info(format_args!("{name} started: pid {pid}"));
                match child.wait() {
                    Ok(status) => Some((pid, status)),
                    Err(e) => {
                        log.error(format_args!("cannot wait for {name} {pid}: {e}"));
                        None
                    }
                }
            }
            Err(e) => {
                log.error(format_args!("cannot start the {name}: {e}"));
                None
            }
        };
        // Removing it takes it: a file, a link or a FIFO in its place counts, a directory does
        // not (it cannot be removed this way, and the UI host never makes one).
        let marker = program
            .quit_marker
            .as_ref()
            .is_some_and(|m| std::fs::remove_file(m).is_ok());
        let end = judge(status.map(|(_, s)| s), marker, program.launcher);
        let pid = status.map_or(0, |(pid, _)| pid);
        match (end, status) {
            (RunEnd::Final(why), _) => {
                log.info(format_args!("{name} {pid} ended ({why:?}); not restarted"));
                return why;
            }
            (RunEnd::Failed, Some((_, status))) => {
                log.warn(format_args!("{name} {pid} exited abnormally: {status}"));
            }
            (RunEnd::Failed, None) => {}
        }
        if started.elapsed() < QUICK_FAILURE {
            quick += 1;
            if quick >= MAX_QUICK_FAILURES {
                log.error(format_args!(
                    "the {name} failed {quick} times in a row at its start; not started again"
                ));
                return UiHostEnd::GaveUp;
            }
        } else {
            quick = 0;
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
    let Some(_lock) = take_lock_within(&cfg.dirs, LOCK_PATIENCE)? else {
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

    /// Held by each test that writes a script and runs it: a script written while another
    /// thread forks can fail to start with "text file busy".
    #[cfg(unix)]
    static SCRIPTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SCRIPTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A stand-in program: appends a line to `starts` in `dir`, then runs `body`.
    #[cfg(unix)]
    fn script(dir: &std::path::Path, body: &str) -> Program {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("ui-host");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho x >> '{}'\n{body}\n",
                dir.join("starts").display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        Program {
            exe,
            args: Vec::new(),
            quit_marker: Some(dir.join("desktop.quit")),
            launcher: false,
        }
    }

    #[cfg(unix)]
    fn supervised(program: &Program, dir: &std::path::Path) -> (UiHostEnd, usize) {
        let log = Logger::open(&dir.join("logs"), "test");
        let end = supervise_by_exit("ui host", program, &[Duration::ZERO], HEALTHY_AFTER, &log);
        let starts = std::fs::read_to_string(dir.join("starts"))
            .unwrap_or_default()
            .lines()
            .count();
        (end, starts)
    }

    #[cfg(unix)]
    #[test]
    fn a_ui_host_that_fails_at_every_start_is_given_up_on() {
        let _serial = serial();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let program = script(dir.path(), "exit 3");
        assert_eq!(
            supervised(&program, dir.path()),
            (UiHostEnd::GaveUp, MAX_QUICK_FAILURES)
        );
        // A program that cannot be started at all counts the same.
        let missing = Program::child(dir.path().join("no-such-program"), Vec::new());
        assert_eq!(supervised(&missing, dir.path()).0, UiHostEnd::GaveUp);
    }

    #[cfg(unix)]
    #[test]
    fn a_usage_error_and_a_sigterm_end_the_supervision_at_once() {
        let _serial = serial();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let program = script(dir.path(), &format!("exit {EXIT_USAGE}"));
        assert_eq!(supervised(&program, dir.path()), (UiHostEnd::UsageError, 1));
        let dir = tempfile::TempDir::new().expect("temp dir");
        let program = script(dir.path(), "kill -TERM $$");
        assert_eq!(supervised(&program, dir.path()), (UiHostEnd::Terminated, 1));
    }

    #[cfg(unix)]
    #[test]
    fn behind_a_launcher_only_the_quit_marker_is_a_quit() {
        let _serial = serial();
        // `open -W` exits 0 when the app ends, crashed or not: the first run (no marker) is
        // restarted, the second (which leaves the marker) is not. A marker left from before
        // the first start is removed first.
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("desktop.quit"), b"").expect("stale marker");
        let mut program = script(
            dir.path(),
            &format!(
                "if [ \"$(wc -l < '{starts}')\" -ge 2 ]; then : > '{marker}'; fi\nexit 0",
                starts = dir.path().join("starts").display(),
                marker = dir.path().join("desktop.quit").display()
            ),
        );
        program.launcher = true;
        assert_eq!(supervised(&program, dir.path()), (UiHostEnd::Quit, 2));
        assert!(
            !dir.path().join("desktop.quit").exists(),
            "the marker is taken"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_end_of_a_run_is_read_from_the_marker_then_the_status() {
        use std::os::unix::process::ExitStatusExt;
        let code = |c: i32| Some(ExitStatus::from_raw(c << 8));
        let signal = |s: i32| Some(ExitStatus::from_raw(s));
        assert_eq!(judge(code(0), false, false), RunEnd::Final(UiHostEnd::Quit));
        assert_eq!(judge(code(1), true, false), RunEnd::Final(UiHostEnd::Quit));
        assert_eq!(judge(code(1), false, false), RunEnd::Failed);
        assert_eq!(judge(code(70), false, false), RunEnd::Failed);
        assert_eq!(
            judge(code(EXIT_USAGE), false, false),
            RunEnd::Final(UiHostEnd::UsageError)
        );
        assert_eq!(
            judge(signal(15), false, false),
            RunEnd::Final(UiHostEnd::Terminated)
        );
        assert_eq!(judge(signal(9), false, false), RunEnd::Failed);
        assert_eq!(judge(signal(11), false, false), RunEnd::Failed);
        // Behind a launcher the status says nothing about the app.
        assert_eq!(judge(code(0), false, true), RunEnd::Failed);
        assert_eq!(judge(code(EXIT_USAGE), false, true), RunEnd::Failed);
        assert_eq!(judge(code(0), true, true), RunEnd::Final(UiHostEnd::Quit));
        assert_eq!(judge(None, false, false), RunEnd::Failed);
    }

    #[test]
    fn a_starting_supervisor_waits_a_moment_for_the_lock() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        let held = take_lock(&dirs).expect("io").expect("free");
        // Held for good: given up after the patience.
        let start = Instant::now();
        assert!(
            take_lock_within(&dirs, Duration::from_millis(300))
                .expect("io")
                .is_none()
        );
        assert!(start.elapsed() >= Duration::from_millis(300));
        // Released while it waits (an old supervisor exiting): taken.
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        assert!(
            take_lock_within(&dirs, Duration::from_secs(2))
                .expect("io")
                .is_some()
        );
        releaser.join().expect("joined");
    }
}
