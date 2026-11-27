//! A UI host that finds another one running for its data root steps aside with status 0 and
//! leaves no quit marker (the beta review's re-verification, N2): the marker speaks for the UI
//! host the supervisor started, and a second start of Iamb while it runs (the Start menu on
//! Windows, a second copy on macOS) must not turn that one's next crash into a quit. Runs the
//! real `iamb-desktop` binary, which exits before it opens any window.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use iamb_desktop::paths::DataDirs;

#[test]
fn a_ui_host_that_steps_aside_leaves_no_quit_marker() {
    let data = tempfile::TempDir::new().expect("temp dir");
    let dirs = DataDirs::new(data.path());
    let _held = dirs
        .take_instance_lock()
        .expect("lock")
        .expect("the lock is free");
    let mut child = Command::new(env!("CARGO_BIN_EXE_iamb-desktop"))
        .arg("--data-dir")
        .arg(data.path())
        .env_remove("IAMB_DATA_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("runs");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("the second ui host did not step aside");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let log =
        std::fs::read_to_string(data.path().join("logs").join("desktop.log")).unwrap_or_default();
    assert!(status.success(), "{status}; log:\n{log}");
    assert!(
        log.contains("another ui host is running"),
        "it stepped aside; log:\n{log}"
    );
    assert!(
        !dirs.quit_marker().exists(),
        "a ui host that stepped aside left the quit marker"
    );
}
