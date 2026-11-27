//! Restart requests (`supervisor` module docs, "Restart requests"): the file the UI host leaves in
//! `run/` for "Start it again" and "Restart the helper" replaces a running worker at once, and
//! cuts a backoff short. Runs the real `iamb-service` binary against the mock platform. Unix only,
//! like `supervisor.rs`.

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

struct Supervisor {
    child: Child,
    dir: TempDir,
}

impl Supervisor {
    fn start(env: &[(&str, &str)]) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut cmd = Command::new(BIN);
        cmd.args(["--supervise", "--platform", "mock", "--data-dir"])
            .arg(dir.path())
            .env_remove("IAMB_DATA_DIR")
            .env_remove("IAMB_TEST_FAULT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("supervisor starts");
        Self { child, dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.path().join("logs").join("supervisor.log")).unwrap_or_default()
    }

    fn ask_for_restart(&self) {
        fs::write(self.path().join("run").join("restart-worker"), b"").expect("request");
    }

    fn wait_for_log(&self, needle: &str, limit: Duration) -> Duration {
        let start = Instant::now();
        while !self.log().contains(needle) {
            assert!(
                start.elapsed() < limit,
                "no {needle:?} within {limit:?}: {}",
                self.log()
            );
            thread::sleep(Duration::from_millis(20));
        }
        start.elapsed()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while status(self.path()).is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        reap(self.path());
    }
}

/// Waits for every process started for `dir` (the worker follows its supervisor when its input
/// ends; a UI host stand-in is killed by its test) and kills what is left after five seconds, so
/// that no test leaves a process behind, also when it panics (`Drop` runs while unwinding). It
/// runs before the data directory is removed, so that a worker always finds its own socket file
/// when it stops (before the beta-gap review's finding 10 was fixed, one without it never exited).
fn reap(dir: &Path) {
    let pattern = dir.display().to_string();
    let alive = || {
        Command::new("pgrep")
            .args(["-f", &pattern])
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if alive() {
        let _ = Command::new("pkill").args(["-9", "-f", &pattern]).status();
        let deadline = Instant::now() + Duration::from_secs(2);
        while alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
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

#[test]
fn a_restart_request_replaces_the_running_worker_at_once() {
    let s = Supervisor::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));

    s.ask_for_restart();
    let (second, took) = wait_for_worker(s.path(), Some(first.pid), Duration::from_secs(10));
    eprintln!("worker replaced on request in {took:?}");
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert!(s.log().contains("restart asked: worker"));
    // The request was taken, and a restart on request is not a failure: no backoff was logged.
    assert!(!s.path().join("run").join("restart-worker").exists());
    assert!(!s.log().contains("restarting the worker in"));

    // A second request works the same way.
    s.ask_for_restart();
    let (_, took) = wait_for_worker(s.path(), Some(second.pid), Duration::from_secs(10));
    assert!(took < Duration::from_secs(3), "took {took:?}");
}

#[test]
fn a_restart_request_cuts_the_backoff_short() {
    // Every worker fails at start, so the supervisor reaches the 30 s step of its backoff.
    let s = Supervisor::start(&[("IAMB_TEST_FAULT", "exit-at-start")]);
    s.wait_for_log("restarting the worker in 30000 ms", Duration::from_secs(20));
    let attempts = s.log().matches("worker started").count();

    s.ask_for_restart();
    let took = s.wait_for_log(
        "restart asked: starting the worker now",
        Duration::from_secs(5),
    );
    eprintln!("backoff cut short after {took:?}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while s.log().matches("worker started").count() <= attempts {
        assert!(Instant::now() < deadline, "no new attempt: {}", s.log());
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_request_left_from_before_is_dropped_at_start() {
    let dir = TempDir::new().expect("temp dir");
    let run = dir.path().join("run");
    fs::create_dir_all(&run).expect("run dir");
    fs::write(run.join("restart-worker"), b"").expect("stale request");
    let child = Command::new(BIN)
        .args(["--supervise", "--platform", "mock", "--data-dir"])
        .arg(dir.path())
        .env_remove("IAMB_DATA_DIR")
        .env_remove("IAMB_TEST_FAULT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("supervisor starts");
    // The guard stops it, and waits for its worker, also when an assertion fails.
    let s = Supervisor { child, dir };
    let dir = &s.dir;
    let (first, _) = wait_for_worker(dir.path(), None, Duration::from_secs(15));
    assert!(!run.join("restart-worker").exists());
    // The first worker stays: the old request did not restart it.
    thread::sleep(Duration::from_millis(600));
    assert_eq!(
        status(dir.path()).map(|s| s.pid),
        Some(first.pid),
        "the worker was restarted"
    );
    drop(s);
}

#[test]
fn a_worker_replaced_on_request_stops_in_order() {
    // Asked to stop through its input, the worker ends as on any orderly stop (it restores the
    // clipboard and releases what it holds) instead of being killed outright.
    let s = Supervisor::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));
    let worker_log =
        || fs::read_to_string(s.path().join("logs").join("worker.log")).unwrap_or_default();
    assert_eq!(worker_log().matches("worker stopped").count(), 0);

    s.ask_for_restart();
    let (_, took) = wait_for_worker(s.path(), Some(first.pid), Duration::from_secs(10));
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert_eq!(
        worker_log().matches("worker stopped").count(),
        1,
        "the old worker did not stop in order: {}",
        worker_log()
    );
    // Its status 0 was asked for: the supervisor stays and so does the new worker.
    thread::sleep(Duration::from_millis(500));
    assert!(status(s.path()).is_some());
    assert!(!s.log().contains("supervisor exits"), "{}", s.log());
}

#[test]
fn a_second_supervisor_leaves_the_request_to_the_running_one() {
    // The real flow of "Start it again" while a supervisor runs: the request, then a second
    // supervisor, which finds the lock taken and exits; the first replaces its worker.
    let s = Supervisor::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));
    s.ask_for_restart();
    let status = Command::new(BIN)
        .args(["--supervise", "--platform", "mock", "--data-dir"])
        .arg(s.path())
        .env_remove("IAMB_DATA_DIR")
        .env_remove("IAMB_TEST_FAULT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("second supervisor runs");
    assert!(status.success(), "the second supervisor: {status:?}");
    let (_, took) = wait_for_worker(s.path(), Some(first.pid), Duration::from_secs(10));
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert_eq!(s.log().matches("another supervisor is running").count(), 1);
}

#[test]
fn a_directory_in_the_requests_place_stalls_nothing() {
    let s = Supervisor::start(&[]);
    let (first, _) = wait_for_worker(s.path(), None, Duration::from_secs(15));
    let request = s.path().join("run").join("restart-worker");
    fs::create_dir(&request).expect("dir");
    thread::sleep(Duration::from_millis(800));
    assert_eq!(
        status(s.path()).map(|st| st.pid),
        Some(first.pid),
        "a directory is no request"
    );
    // Gone again, a real request works as before.
    fs::remove_dir(&request).expect("rmdir");
    s.ask_for_restart();
    wait_for_worker(s.path(), Some(first.pid), Duration::from_secs(10));
}
