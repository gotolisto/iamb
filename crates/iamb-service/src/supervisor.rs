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
//! # Hook points
//!
//! - **UI host and sync agent.** §3.2 has the supervisor start and restart `iamb-desktop` and
//!   `iamb-sync` too. They do not exist yet. Each will be one more [`Child`] supervised by the
//!   same loop (their own backoff counters; no heartbeat, exit-only supervision), added in
//!   [`run`] where the comment says so.
//! - **Updates** (ADR-0023): the UI host will ask the supervisor, over a private pipe with the
//!   peer verification of ADR-0006, to stop the worker and to start it again.

use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child as Process, ChildStdin, Command, ExitStatus, Stdio};
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

/// Takes `run/supervisor.lock`, or `None` when another supervisor holds it.
///
/// # Errors
///
/// I/O errors other than the lock being held.
pub fn take_lock(dirs: &DataDirs) -> io::Result<Option<File>> {
    create_private_dir(&dirs.run())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dirs.supervisor_lock())?;
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
    let mut timeout = cfg.startup_grace;
    loop {
        match beats.recv_timeout(timeout) {
            Ok(()) => timeout = cfg.heartbeat_timeout,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.process.kill();
                let _ = child.process.wait();
                return Ok(Ended::Hung);
            }
            Err(RecvTimeoutError::Disconnected) => {
                return child.process.wait().map(Ended::Exited);
            }
        }
    }
}

/// Runs the supervisor until the worker stops normally.
///
/// # Errors
///
/// I/O errors taking the lock.
pub fn run(cfg: &SupervisorConfig, log: &Logger) -> io::Result<Outcome> {
    let Some(_lock) = take_lock(&cfg.dirs)? else {
        log.info(format_args!("another supervisor is running; exiting"));
        return Ok(Outcome::AlreadyRunning);
    };
    log.info(format_args!(
        "supervisor {} started, pid {}",
        crate::VERSION,
        std::process::id()
    ));
    // Hook point: start the UI host and the sync agent here once they exist (module docs).
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
        thread::sleep(delay);
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
    fn the_lock_admits_one_supervisor() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        let first = take_lock(&dirs).expect("io").expect("free");
        assert!(take_lock(&dirs).expect("io").is_none());
        drop(first);
        assert!(take_lock(&dirs).expect("io").is_some());
    }
}
