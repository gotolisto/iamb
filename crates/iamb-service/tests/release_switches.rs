//! What a release build of `iamb-service` refuses (the beta-gap review's finding 1 and its
//! re-verification): `--ui-host` on either platform, and `--platform mock` where the system has
//! a native platform. Only a release build has anything to check here: CI runs this file with
//! `cargo test --release` in its "Build the service alone (release)" step.

#![cfg(not(debug_assertions))]

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_iamb-service");

/// Runs the binary with `args`; its exit code (`None`: still running after 5 s, then killed)
/// and what it wrote to standard error.
fn run(args: &[&str], data: &std::path::Path) -> (Option<i32>, String) {
    let mut child = Command::new(BIN)
        .args(args)
        .arg("--data-dir")
        .arg(data)
        .env_remove("IAMB_DATA_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("runs");
    let start = Instant::now();
    let code = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status.code();
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut err = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut err);
    }
    (code, err)
}

#[test]
fn a_release_build_refuses_the_ui_host_switch_on_either_platform() {
    let data = tempfile::TempDir::new().expect("temp dir");
    for platform in ["mock", "native"] {
        let (code, err) = run(
            &["--supervise", "--platform", platform, "--ui-host", BIN],
            data.path(),
        );
        assert_eq!(code, Some(1), "--platform {platform}: {err}");
        assert!(
            err.contains("--ui-host is for development builds"),
            "--platform {platform}: {err}"
        );
    }
    assert!(
        !data.path().join("logs").exists(),
        "nothing ran for the data root"
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_release_build_refuses_the_mock_platform_where_there_is_a_native_one() {
    let data = tempfile::TempDir::new().expect("temp dir");
    for mode in ["--supervise", "--worker"] {
        let (code, err) = run(&[mode, "--platform", "mock"], data.path());
        assert_eq!(code, Some(1), "{mode}: {err}");
        assert!(
            err.contains("--platform mock is for development builds"),
            "{mode}: {err}"
        );
    }
}
