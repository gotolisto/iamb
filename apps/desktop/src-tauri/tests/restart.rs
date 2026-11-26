//! "Start it again" and "Restart the helper" (UX specification §4.2, §4.6): the UI host leaves
//! the restart request where the supervisor looks for it and starts the supervisor for its own
//! data root (`iamb_desktop::onboarding::LaunchSupervisor`).

use std::path::Path;
#[cfg(unix)]
use std::time::{Duration, Instant};

use iamb_desktop::onboarding::{Launch, LaunchSupervisor, RESTART_REQUEST, Supervisor};
// Used only by the Unix tests below (they run shell scripts).
#[cfg(unix)]
use iamb_desktop::onboarding::{start_supervisor, supervisor_running};

#[test]
fn the_request_has_the_name_the_supervisor_looks_for() {
    assert_eq!(RESTART_REQUEST, iamb_service::supervisor::RESTART_REQUEST);
    assert_eq!(
        iamb_desktop::paths::QUIT_MARKER,
        iamb_service::paths::UI_HOST_QUIT_MARKER
    );
    let dirs = iamb_service::paths::DataDirs::new("/data");
    assert_eq!(
        dirs.restart_request(),
        Path::new("/data").join("run").join(RESTART_REQUEST)
    );
}

/// Held by each test that writes a script and runs it: a script written while another thread of
/// this process forks can fail to start with "text file busy" (the child holds the write end
/// until it execs).
static SCRIPTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SCRIPTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A stand-in for `iamb-service` that writes the arguments it was given to a file.
#[cfg(unix)]
fn fake_service(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let exe = dir.join("iamb-service");
    let out = dir.join("args.txt");
    std::fs::write(
        &exe,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", out.display()),
    )
    .expect("script");
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    exe
}

#[cfg(unix)]
#[test]
fn a_restart_leaves_the_request_and_starts_the_supervisor_for_the_data_root() {
    use std::os::unix::fs::PermissionsExt;
    let _serial = serial();
    let bin = tempfile::TempDir::new().expect("temp dir");
    let data = tempfile::TempDir::new().expect("temp dir");
    let supervisor = LaunchSupervisor {
        launch: Some(Launch::Direct(fake_service(bin.path()))),
        root: data.path().to_owned(),
    };
    supervisor.restart_worker().expect("restart asked");
    assert!(data.path().join("run").join(RESTART_REQUEST).is_file());

    let args = bin.path().join("args.txt");
    let deadline = Instant::now() + Duration::from_secs(10);
    let written = loop {
        if let Ok(text) = std::fs::read_to_string(&args)
            && text.lines().count() >= 4
        {
            break text;
        }
        assert!(Instant::now() < deadline, "the supervisor was not started");
        std::thread::sleep(Duration::from_millis(20));
    };
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines[..3], ["--supervise", "--no-ui-host", "--data-dir"]);
    assert_eq!(Path::new(lines[3]), data.path());

    let mode = std::fs::metadata(data.path().join("run"))
        .expect("run dir")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "run/ is the user's alone");
}

#[test]
fn a_supervisor_that_cannot_start_is_unavailable() {
    let _serial = serial();
    let data = tempfile::TempDir::new().expect("temp dir");
    let supervisor = LaunchSupervisor {
        launch: Some(Launch::Direct(data.path().join("no-such-service"))),
        root: data.path().to_owned(),
    };
    let e = supervisor.restart_worker().expect_err("cannot start");
    assert_eq!(e.code, "unavailable");
}

/// Waits until the fake service wrote its arguments: it was started.
#[cfg(unix)]
fn started(bin: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::fs::read_to_string(bin.join("args.txt")).is_ok_and(|t| t.lines().count() >= 4) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(unix)]
#[test]
fn a_request_that_cannot_be_written_still_starts_a_supervisor_that_does_not_run() {
    let _serial = serial();
    let bin = tempfile::TempDir::new().expect("temp dir");
    let data = tempfile::TempDir::new().expect("temp dir");
    // A directory in the request's place: the request cannot be made.
    std::fs::create_dir_all(data.path().join("run").join(RESTART_REQUEST)).expect("dir");
    let supervisor = LaunchSupervisor {
        launch: Some(Launch::Direct(fake_service(bin.path()))),
        root: data.path().to_owned(),
    };
    supervisor
        .restart_worker()
        .expect("no supervisor runs: starting one is the restart");
    assert!(started(bin.path()), "the supervisor was not started");

    // With a supervisor running, the failed request is reported (after the start was tried).
    let bin = tempfile::TempDir::new().expect("temp dir");
    let lock =
        std::fs::File::create(data.path().join("run").join("supervisor.lock")).expect("lock");
    lock.lock().expect("locked");
    assert!(supervisor_running(&data.path().join("run")));
    let supervisor = LaunchSupervisor {
        launch: Some(Launch::Direct(fake_service(bin.path()))),
        root: data.path().to_owned(),
    };
    let e = supervisor
        .restart_worker()
        .expect_err("the request is missing");
    assert_eq!(e.code, "unavailable");
    assert!(started(bin.path()), "the supervisor was not started");
}

#[cfg(unix)]
#[test]
fn a_request_never_follows_a_link_or_waits_on_a_fifo() {
    let _serial = serial();
    let bin = tempfile::TempDir::new().expect("temp dir");
    let data = tempfile::TempDir::new().expect("temp dir");
    let run = data.path().join("run");
    std::fs::create_dir_all(&run).expect("run");
    let precious = data.path().join("precious");
    std::fs::write(&precious, b"keep me").expect("file");
    std::os::unix::fs::symlink(&precious, run.join(RESTART_REQUEST)).expect("link");
    let supervisor = LaunchSupervisor {
        launch: Some(Launch::Direct(fake_service(bin.path()))),
        root: data.path().to_owned(),
    };
    supervisor.restart_worker().expect("asked");
    assert_eq!(std::fs::read(&precious).expect("read"), b"keep me");

    std::fs::remove_file(run.join(RESTART_REQUEST)).expect("remove link");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(run.join(RESTART_REQUEST))
            .status()
            .expect("mkfifo")
            .success()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(supervisor.restart_worker().is_ok());
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(true),
        "the request waited on a FIFO"
    );
}

#[cfg(unix)]
#[test]
fn a_launcher_that_fails_is_unavailable() {
    let _serial = serial();
    // `open` returns at once with the system's answer: its status is waited for.
    let e = start_supervisor("/usr/bin/false".into(), &[], true).expect_err("it failed");
    assert_eq!(e.code, "unavailable");
    start_supervisor("/usr/bin/true".into(), &[], true).expect("it worked");
}
