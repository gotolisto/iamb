//! The supervisor, running the real `iamb-service` binary against the mock platform and the
//! native endpoint: kill-and-restart, heartbeat timeout, single instance, backoff, and no
//! orphaned worker after the supervisor dies. Unix only (the tests kill processes with
//! `kill -9`).

#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use iamb_ipc::{Client, RequestBody, ResponseBody, Role, ServiceStatus, native_endpoint};
use iamb_platform::{EndpointKind, EndpointName, PeerPolicy};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_iamb-service");

struct Running {
    child: Child,
    dir: TempDir,
}

impl Running {
    fn start(env: &[(&str, &str)]) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let child = supervisor(dir.path(), env);
        Self { child, dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn log(&self, process: &str) -> String {
        fs::read_to_string(self.path().join("logs").join(format!("{process}.log")))
            .unwrap_or_default()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The worker follows when its input ends.
        let deadline = Instant::now() + Duration::from_secs(5);
        while status(self.path()).is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn supervisor(dir: &Path, env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(BIN);
    cmd.args(["--supervise", "--platform", "mock", "--data-dir"])
        .arg(dir)
        .env_remove("IAMB_DATA_DIR")
        .env_remove("IAMB_TEST_FAULT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.spawn().expect("supervisor starts")
}

fn status(dir: &Path) -> Option<ServiceStatus> {
    let name = EndpointName::new(EndpointKind::Service, dir.join("run"));
    let mut client = Client::connect(
        native_endpoint().as_ref(),
        &name,
        PeerPolicy::SameUser,
        Role::Cli,
        "test",
        Duration::from_secs(1),
    )
    .ok()?;
    client.set_call_timeout(Duration::from_secs(1));
    match client.call(RequestBody::GetStatus {}) {
        Ok(ResponseBody::Status { status }) => Some(status),
        _ => None,
    }
}

/// Polls until a worker other than `not` answers, and returns its status and the wait.
fn wait_for_worker(dir: &Path, not: Option<u32>, limit: Duration) -> (ServiceStatus, Duration) {
    let start = Instant::now();
    loop {
        if let Some(s) = status(dir)
            && Some(s.pid) != not
        {
            return (s, start.elapsed());
        }
        assert!(
            start.elapsed() < limit,
            "no new worker answered within {limit:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn kill9(pid: u32) {
    let ok = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill runs")
        .success();
    assert!(ok, "kill -9 {pid}");
}

#[test]
fn a_killed_worker_is_back_in_under_two_seconds() {
    let s = Running::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));

    kill9(first.pid);
    let (second, took) = wait_for_worker(s.path(), Some(first.pid), Duration::from_secs(5));
    eprintln!("restart after kill -9 (backoff 0 s): {took:?}");
    assert!(took < Duration::from_secs(2), "took {took:?}");

    // A second failure soon after: the backoff is now 1 s, still under 2 s in total.
    kill9(second.pid);
    let (_, took) = wait_for_worker(s.path(), Some(second.pid), Duration::from_secs(5));
    eprintln!("restart after a second kill -9 (backoff 1 s): {took:?}");
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert!(s.log("supervisor").contains("exited abnormally"));
}

#[test]
fn a_worker_without_heartbeat_is_killed_and_restarted() {
    let s = Running::start(&[("IAMB_TEST_FAULT", "hang-once")]);
    let start = Instant::now();
    // The first worker hangs 300 ms after it is ready; the second is healthy.
    let deadline = start + Duration::from_secs(20);
    while !s.log("supervisor").contains("attempt 2") {
        assert!(
            Instant::now() < deadline,
            "no restart: {}",
            s.log("supervisor")
        );
        thread::sleep(Duration::from_millis(20));
    }
    let restarted = start.elapsed();
    let (status, _) = wait_for_worker(s.path(), None, Duration::from_secs(10));
    eprintln!("hung worker replaced after {restarted:?} (3 s heartbeat timeout)");
    assert!(
        restarted >= Duration::from_secs(3),
        "restarted too early: {restarted:?}"
    );
    assert!(
        restarted < Duration::from_secs(8),
        "restarted too late: {restarted:?}"
    );
    let log = s.log("supervisor");
    assert!(log.contains("no heartbeat from worker"), "{log}");
    assert!(!status.paused);
    assert!(
        s.log("worker")
            .contains("test fault: the engine thread hangs")
    );
}

#[test]
fn a_second_supervisor_exits_and_leaves_the_first_alone() {
    let s = Running::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));
    let started = Instant::now();
    let out = Command::new(BIN)
        .args(["--supervise", "--platform", "mock", "--data-dir"])
        .arg(s.path())
        .env_remove("IAMB_DATA_DIR")
        .output()
        .expect("second supervisor runs");
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("another supervisor is running"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    let (again, _) = wait_for_worker(s.path(), None, Duration::from_secs(5));
    assert_eq!(again.pid, first.pid, "the running worker was not touched");
}

/// Milliseconds of the day from a log line's leading `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn ms_of_day(line: &str) -> u64 {
    let time = &line[11..23];
    let part = |r: std::ops::Range<usize>| time[r].parse::<u64>().expect("digits");
    ((part(0..2) * 60 + part(3..5)) * 60 + part(6..8)) * 1000 + part(9..12)
}

#[test]
fn a_crashing_worker_is_restarted_with_backoff() {
    let s = Running::start(&[("IAMB_TEST_FAULT", "exit-at-start")]);
    thread::sleep(Duration::from_millis(7500));
    let log = s.log("supervisor");
    let starts: Vec<u64> = log
        .lines()
        .filter(|l| l.contains("worker started"))
        .map(ms_of_day)
        .collect();
    assert_eq!(starts.len(), 4, "0 s, 1 s, 5 s, then 30 s:\n{log}");
    let gaps: Vec<u64> = starts.windows(2).map(|w| w[1] - w[0]).collect();
    eprintln!("gaps between starts (ms): {gaps:?}");
    assert!(gaps[0] < 600, "first restart at once: {gaps:?}");
    assert!((900..1700).contains(&gaps[1]), "then after 1 s: {gaps:?}");
    assert!((4900..5800).contains(&gaps[2]), "then after 5 s: {gaps:?}");
    assert!(log.contains("restarting the worker in 30000 ms"), "{log}");
}

#[test]
fn a_killed_supervisor_leaves_no_worker_behind() {
    let mut s = Running::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));
    s.child.kill().expect("kill the supervisor");
    let _ = s.child.wait();
    let start = Instant::now();
    while status(s.path()).is_some() {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "worker {} still answers",
            first.pid
        );
        thread::sleep(Duration::from_millis(20));
    }
    // Its store lock is released too.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match iamb_store::Store::open(s.path()) {
            Ok(_) => break,
            Err(e) => {
                assert!(Instant::now() < deadline, "store still locked: {e}");
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    assert!(s.log("worker").contains("the supervisor is gone"));
}
