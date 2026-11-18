//! The command-line tool with a running service (architecture §6.3): while the worker holds
//! `run/store.lock`, every command goes through the service endpoint with the role `cli`; once
//! the worker is gone, the tool opens the store itself.
//!
//! The worker runs in this process against the mock platform, serving the real (native)
//! endpoint, so the `iamb` binary connects to it as it would to the installed service. Linux
//! only: the endpoints of the OS crates are tested on their own systems.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iamb_platform::mock::{MockPlatform, MockWindow};
use iamb_platform::{FocusToken, PeerPolicy};
use iamb_service::paths::DataDirs;
use iamb_service::worker::{Worker, WorkerConfig};
use tempfile::TempDir;

fn iamb(dir: &Path, args: &[&str]) -> (Output, Duration) {
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_iamb"))
        .arg("--data-dir")
        .arg(dir)
        .env_remove("IAMB_DATA_DIR")
        .args(args)
        .output()
        .expect("run iamb");
    (out, start.elapsed())
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, took) = iamb(dir, args);
    assert!(
        out.status.success(),
        "iamb {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Opening the store directly would wait 3 s for the worker's lock and fail: a fast
    // success means the command went through the service.
    assert!(took < Duration::from_secs(2), "iamb {args:?} took {took:?}");
    String::from_utf8(out.stdout).unwrap()
}

struct Service {
    os: MockPlatform,
    editor: FocusToken,
    worker: Option<Worker>,
}

impl Service {
    fn start(dir: &Path) -> Self {
        let os = MockPlatform::new();
        let editor = os.open_window(MockWindow::new("org.example.editor", 100));
        os.focus(editor).unwrap();
        let mut config = WorkerConfig::new(
            DataDirs::new(dir),
            Arc::new(os.clone()),
            iamb_ipc::native_endpoint(),
        );
        config.policy = PeerPolicy::SameUser;
        let worker = Worker::start(config).expect("worker starts");
        worker.settle();
        Self {
            os,
            editor,
            worker: Some(worker),
        }
    }

    fn worker(&self) -> &Worker {
        self.worker.as_ref().unwrap()
    }

    fn stop(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop();
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn commands_go_through_the_service_while_it_holds_the_store() {
    let d = TempDir::new().unwrap();
    let mut svc = Service::start(d.path());

    let id = ok(
        d.path(),
        &["add", "brb", "be right back", "--label", "Back"],
    );
    assert_eq!(id.trim().len(), 36, "an id: {id}");
    ok(
        d.path(),
        &["add", "sig", "Kind regards", "--folder", "Work / Email"],
    );

    // The worker rebuilt its snapshot: typing the new abbreviation expands it.
    svc.worker().settle();
    svc.os.type_text("brb ");
    svc.worker().settle();
    assert_eq!(svc.os.text(svc.editor).unwrap(), "be right back ");

    let list = ok(d.path(), &["list"]);
    assert!(list.contains("brb") && list.contains("Back"), "{list}");
    assert!(list.contains("Work / Email"), "{list}");
    let json = ok(d.path(), &["list", "--folder", "Work", "--json"]);
    assert!(
        json.contains("\"sig\"") && !json.contains("\"brb\""),
        "{json}"
    );

    // A conflict is reported like the local tool reports it.
    let (out, _) = iamb(d.path(), &["add", "BRB", "x"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("already used by snippet") && err.contains("(Back)"),
        "{err}"
    );

    assert_eq!(ok(d.path(), &["expand", "brb "]), "be right back \n");
    let (out, _) = iamb(d.path(), &["expand", "nothing here"]);
    assert_eq!(out.status.code(), Some(3));

    // Export and import travel as paths, resolved here.
    let export = d.path().join("out.json");
    let out = ok(d.path(), &["export", export.to_str().unwrap()]);
    assert!(out.contains("Exported 2 snippets"), "{out}");
    ok(d.path(), &["remove", "brb"]);
    assert!(!ok(d.path(), &["list"]).contains("brb"));
    let out = ok(d.path(), &["import", export.to_str().unwrap()]);
    assert!(out.contains("Imported 1 snippets (1 skipped)"), "{out}");
    assert!(ok(d.path(), &["list"]).contains("brb"));

    // Without the service the tool opens the store itself and sees the same library.
    svc.stop();
    let list = ok(d.path(), &["list"]);
    assert!(list.contains("brb") && list.contains("sig"), "{list}");
}

#[test]
fn a_relative_export_path_is_resolved_by_the_tool() {
    let d = TempDir::new().unwrap();
    let _svc = Service::start(d.path());
    ok(d.path(), &["add", "omw", "on my way"]);
    let out = Command::new(env!("CARGO_BIN_EXE_iamb"))
        .current_dir(d.path())
        .arg("--data-dir")
        .arg(d.path())
        .args(["export", "relative.csv"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(d.path().join("relative.csv").exists());
}
