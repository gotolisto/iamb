//! "Start it again" and "Restart the helper" (UX specification §4.2, §4.6): the UI host leaves
//! the restart request where the supervisor looks for it and starts the supervisor for its own
//! data root (`iamb_desktop::onboarding::LaunchSupervisor`).

use std::path::Path;
use std::time::{Duration, Instant};

use iamb_desktop::onboarding::{Launch, LaunchSupervisor, RESTART_REQUEST, Supervisor};

#[test]
fn the_request_has_the_name_the_supervisor_looks_for() {
    assert_eq!(RESTART_REQUEST, iamb_service::supervisor::RESTART_REQUEST);
    let dirs = iamb_service::paths::DataDirs::new("/data");
    assert_eq!(
        dirs.restart_request(),
        Path::new("/data").join("run").join(RESTART_REQUEST)
    );
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
            && text.lines().count() >= 3
        {
            break text;
        }
        assert!(Instant::now() < deadline, "the supervisor was not started");
        std::thread::sleep(Duration::from_millis(20));
    };
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines[0], "--supervise");
    assert_eq!(lines[1], "--data-dir");
    assert_eq!(Path::new(lines[2]), data.path());

    let mode = std::fs::metadata(data.path().join("run"))
        .expect("run dir")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "run/ is the user's alone");
}

#[test]
fn a_supervisor_that_cannot_start_is_unavailable() {
    let data = tempfile::TempDir::new().expect("temp dir");
    let supervisor = LaunchSupervisor {
        launch: Some(Launch::Direct(data.path().join("no-such-service"))),
        root: data.path().to_owned(),
    };
    let e = supervisor.restart_worker().expect_err("cannot start");
    assert_eq!(e.code, "unavailable");
}
