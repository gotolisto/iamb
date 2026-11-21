//! The supervisor starts the UI host (`supervisor` module docs, "The UI host"; architecture §3.2):
//! at its own start, for its data root, again after an abnormal exit, and not again after status
//! 0. Runs the real `iamb-service` binary against the mock platform, with a shell script standing
//! in for `iamb-desktop`. Unix only, like `supervisor.rs`.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_iamb-service");

/// A stand-in UI host: appends its arguments to `starts.txt`, then exits with the status the
/// file `exit-codes` gives for this start (one line per start; past its end it stays up, as a
/// UI host does, until it is killed).
fn fake_ui_host(dir: &Path, codes: &[i32]) -> PathBuf {
    let exe = dir.join("iamb-desktop");
    let codes_file = dir.join("exit-codes");
    let lines: Vec<String> = codes.iter().map(i32::to_string).collect();
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    fs::write(&codes_file, text).expect("codes");
    let script = format!(
        r#"#!/bin/sh
dir='{dir}'
echo "$$ $*" >> "$dir/starts.txt"
n=$(wc -l < "$dir/starts.txt")
code=$(sed -n "${{n}}p" "$dir/exit-codes")
if [ -n "$code" ]; then exit "$code"; fi
exec sleep 60
"#,
        dir = dir.display()
    );
    fs::write(&exe, script).expect("script");
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).expect("chmod");
    exe
}

struct Supervisor {
    child: Child,
    data: TempDir,
    bin: TempDir,
}

impl Supervisor {
    fn start(ui: &[&str], codes: &[i32]) -> Self {
        let data = TempDir::new().expect("temp dir");
        let bin = TempDir::new().expect("temp dir");
        let host = fake_ui_host(bin.path(), codes);
        let mut cmd = Command::new(BIN);
        cmd.args(["--supervise", "--platform", "mock", "--data-dir"])
            .arg(data.path())
            .env_remove("IAMB_DATA_DIR")
            .env_remove("IAMB_TEST_FAULT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for a in ui {
            if *a == "HOST" {
                cmd.arg(&host);
            } else {
                cmd.arg(a);
            }
        }
        let child = cmd.spawn().expect("supervisor starts");
        Self { child, data, bin }
    }

    fn starts(&self) -> Vec<String> {
        fs::read_to_string(self.bin.path().join("starts.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.data.path().join("logs").join("supervisor.log")).unwrap_or_default()
    }

    fn wait_for(&self, what: &str, limit: Duration, done: impl Fn(&Self) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < limit,
                "{what} not within {limit:?}; starts {:?}; log:\n{}",
                self.starts(),
                self.log()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The stand-ins that stay up, and the worker, which follows the supervisor by itself.
        for line in self.starts() {
            if let Some(pid) = line.split(' ').next() {
                let _ = Command::new("kill").arg(pid).stderr(Stdio::null()).status();
            }
        }
    }
}

#[test]
fn the_ui_host_starts_with_the_supervisor_for_its_data_root() {
    let s = Supervisor::start(&["--ui-host", "HOST"], &[]);
    s.wait_for("the ui host's start", Duration::from_secs(10), |s| {
        !s.starts().is_empty()
    });
    let first = s.starts()[0].clone();
    let args: Vec<&str> = first.splitn(2, ' ').collect();
    assert_eq!(
        args[1],
        format!("--data-dir {}", s.data.path().display()),
        "{first}"
    );
    s.wait_for("the log line", Duration::from_secs(5), |s| {
        s.log().contains("ui host started: pid")
    });
    // It stays up, and nothing starts a second one.
    thread::sleep(Duration::from_millis(500));
    assert_eq!(s.starts().len(), 1);
}

#[test]
fn an_abnormal_exit_restarts_the_ui_host_and_status_zero_ends_it() {
    // First start crashes (status 3), the second quits normally (status 0).
    let s = Supervisor::start(&["--ui-host", "HOST"], &[3, 0]);
    s.wait_for("two starts", Duration::from_secs(10), |s| {
        s.starts().len() >= 2
    });
    s.wait_for("the end of its supervision", Duration::from_secs(5), |s| {
        s.log().contains("exited; not restarted")
    });
    assert!(s.log().contains("ui host"));
    assert!(s.log().contains("exited abnormally"));
    // Status 0 is not restarted, whatever the wait.
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(s.starts().len(), 2, "{}", s.log());
}

#[test]
fn no_ui_host_is_started_with_no_ui_host_or_on_the_mock_platform_by_default() {
    for ui in [&["--no-ui-host"][..], &[][..]] {
        let s = Supervisor::start(ui, &[]);
        s.wait_for("the supervisor's start", Duration::from_secs(10), |s| {
            s.log().contains("worker started")
        });
        assert!(s.log().contains("no ui host to start"), "{}", s.log());
        thread::sleep(Duration::from_millis(300));
        assert!(s.starts().is_empty());
    }
}
