//! Crashes without heap contents (architecture §8 rule 9), with the real `iamb-service`
//! binary on the mock platform: a panic writes a metadata-only crash report and exits with a
//! status instead of aborting, and on Linux the worker keeps its memory out of core files.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use iamb_service::crash::EXIT_PANIC;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_iamb-service");

fn worker(dir: &Path, fault: Option<&str>) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(["--worker", "--platform", "mock", "--data-dir"])
        .arg(dir)
        .env_remove("IAMB_DATA_DIR")
        .env_remove("IAMB_TEST_FAULT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(fault) = fault {
        cmd.env("IAMB_TEST_FAULT", fault);
    }
    cmd
}

fn every_file(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            every_file(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The fault exists in debug builds only.
#[cfg(debug_assertions)]
#[test]
fn a_panic_writes_a_metadata_only_report_and_exits_with_a_status() {
    let dir = TempDir::new().expect("temp dir");
    let out = worker(dir.path(), Some("panic-at-start"))
        .output()
        .expect("the worker runs");
    // An exit status, not a signal: the panic never became an operating-system crash.
    assert_eq!(
        out.status.code(),
        Some(i32::from(EXIT_PANIC)),
        "{:?}",
        out.status
    );

    let logs = dir.path().join("logs");
    let reports: Vec<_> = fs::read_dir(&logs)
        .expect("logs")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("crash-worker-"))
        })
        .collect();
    assert_eq!(reports.len(), 1, "one crash report");
    let report = fs::read_to_string(&reports[0]).expect("report");
    for line in [
        "process: worker",
        &format!("version: {}", iamb_service::VERSION),
        "thread: iamb-test-fault",
        "location: crates/iamb-service/src/main.rs:",
        "kind: panic",
    ] {
        assert!(report.contains(line), "{line:?} in the report");
    }
    let log = fs::read_to_string(logs.join("worker.log")).expect("log");
    assert!(log.contains("crash dumps:"), "the opt-outs are logged");
    assert!(log.contains("panic in thread iamb-test-fault at "));
    assert!(log.contains("crash report crash-worker-"));

    // The panic message is nowhere: not in a file, not on either output stream.
    let mut files = Vec::new();
    every_file(dir.path(), &mut files);
    assert!(files.len() >= 2);
    for file in files {
        let bytes = fs::read(&file).expect("readable");
        assert!(
            !String::from_utf8_lossy(&bytes).contains("zqxpaniccanary"),
            "{} holds the panic message",
            file.display()
        );
    }
    for stream in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(stream).contains("zqxpaniccanary"));
    }
}

/// Linux (development only): the coredump filter of the running worker is 0, so a core file
/// holds no memory mapping. The default filter is 0x33 (private and shared anonymous memory,
/// ELF headers, private huge pages).
#[cfg(target_os = "linux")]
#[test]
fn the_worker_keeps_its_memory_out_of_core_files() {
    use std::thread;
    use std::time::{Duration, Instant};

    let dir = TempDir::new().expect("temp dir");
    let mut child = worker(dir.path(), None)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the worker starts");
    let log = dir.path().join("logs").join("worker.log");
    let deadline = Instant::now() + Duration::from_secs(20);
    let line = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        if let Some(line) = text.lines().find(|l| l.contains("crash dumps:")) {
            break line.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "the worker logged no crash-dump line"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let filter = fs::read_to_string(format!("/proc/{}/coredump_filter", child.id()));
    let _ = child.kill();
    let _ = child.wait();
    assert!(line.contains("coredump-filter-0 ok"), "{line}");
    let filter = filter.expect("the filter is readable");
    assert_eq!(
        u32::from_str_radix(filter.trim(), 16),
        Ok(0),
        "coredump filter {filter:?}"
    );
}
