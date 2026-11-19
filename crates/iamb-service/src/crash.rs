//! Crashes without heap contents (architecture §8 rule 9).
//!
//! Two halves, both installed by each service process (supervisor and worker) at the start of
//! `main`:
//!
//! - [`harden_process`] opts the process out of the operating system's crash dumps, before any
//!   other thread exists. The OS calls live in the platform crates; this function only picks
//!   the one for the build:
//!
//!   | System | What is set | Where |
//!   | --- | --- | --- |
//!   | Windows | Error mode `SEM_NOGPFAULTERRORBOX` (Windows Error Reporting is not invoked), `WerSetFlags(NOHEAP)` | `iamb_platform_windows::harden_process` |
//!   | macOS | `RLIMIT_CORE` 0, soft and hard (no core file) | `iamb_platform_macos::harden_process` |
//!   | Linux (development and CI only) | `/proc/self/coredump_filter` = 0: a core file still appears but holds no memory mapping, only the registers and the list of mappings | here (a file write; no `unsafe`) |
//!
//!   Linux has no platform crate and the service forbids `unsafe`, so neither the core limit
//!   (`setrlimit`) nor `PR_SET_DUMPABLE` can be set there; the coredump filter is what a plain
//!   file write can do. It also applies when the kernel hands the core to a program
//!   (`core_pattern` starting with `|`). Linux is not a shipped target.
//!
//! - [`install_panic_hook`] replaces the panic hook. A panic writes a local crash report
//!   (`logs/crash-<process>-<unix ms>.txt`, the user-only log directory) and a log line, and
//!   then ends the process with [`EXIT_PANIC`] instead of aborting: a Rust panic never becomes
//!   an operating-system crash, and the supervisor restarts the worker as after any abnormal
//!   exit. The report holds only metadata: the process, the version, the system, the time, the
//!   thread's name and the source location. Never the panic message (it can quote data), a
//!   backtrace with argument values, or any memory. At most [`KEPT_REPORTS`] reports are kept
//!   per process.
//!
//! What remains: a crash that is not a Rust panic (a fault in the OS crates' `unsafe` code, a
//! signal) ends the process the operating system's way, with the opt-outs above in force; and
//! a dump taken from outside the process (a debugger, an administrator's full-memory dump
//! policy) is not something a process can refuse (rule 9's stated limit).

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::log::Logger;
use crate::{VERSION, clock};

/// The exit status of a process that panicked (`EX_SOFTWARE`). The supervisor restarts a
/// worker that exits with it, as after any status but 0.
pub const EXIT_PANIC: u8 = 70;

/// Crash reports kept per process; older ones are removed when a new one is written.
pub const KEPT_REPORTS: usize = 10;

/// What [`harden_process`] did: each step's name and its error, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hardening {
    /// `(step, None)` when it took effect, `(step, Some(error))` when not.
    pub steps: Vec<(&'static str, Option<String>)>,
}

impl Hardening {
    /// Whether every step took effect (and there was one).
    #[must_use]
    pub fn complete(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|(_, e)| e.is_none())
    }

    /// One log line: `crash dumps: step ok, step failed (error)`; names and error codes only.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut s = String::from("crash dumps:");
        if self.steps.is_empty() {
            s.push_str(" no opt-out on this system");
        }
        for (i, (step, error)) in self.steps.iter().enumerate() {
            let sep = if i == 0 { " " } else { ", " };
            match error {
                None => {
                    let _ = write!(s, "{sep}{step} ok");
                }
                Some(e) => {
                    let _ = write!(s, "{sep}{step} failed ({e})");
                }
            }
        }
        s
    }
}

/// Opts this process out of the operating system's crash dumps (module docs). Call it first in
/// `main`, before any thread starts.
#[must_use]
pub fn harden_process() -> Hardening {
    #[cfg(windows)]
    let steps = from_platform(iamb_platform_windows::harden_process());
    #[cfg(target_os = "macos")]
    let steps = from_platform(iamb_platform_macos::harden_process());
    #[cfg(target_os = "linux")]
    let steps = vec![(
        "coredump-filter-0",
        set_coredump_filter(Path::new("/proc/self/coredump_filter"))
            .err()
            .map(|e| format!("{:?}", e.kind())),
    )];
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let steps = Vec::new();
    Hardening { steps }
}

#[cfg(any(windows, target_os = "macos"))]
fn from_platform(
    steps: Vec<(&'static str, iamb_platform::Result<()>)>,
) -> Vec<(&'static str, Option<String>)> {
    steps
        .into_iter()
        .map(|(step, r)| (step, r.err().map(|e| e.to_string())))
        .collect()
}

/// Writes 0 to a coredump filter file: no memory mapping of any kind goes into a core file.
#[cfg(target_os = "linux")]
fn set_coredump_filter(file: &Path) -> io::Result<()> {
    fs::write(file, b"0")?;
    let now = fs::read_to_string(file)?;
    if u32::from_str_radix(now.trim(), 16) == Ok(0) {
        Ok(())
    } else {
        Err(io::Error::other("the filter did not take"))
    }
}

/// What a crash report holds. Only metadata (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    /// `supervisor` or `worker`.
    pub process: &'static str,
    /// The thread's name (`?` for an unnamed thread).
    pub thread: String,
    /// The source file and line of the panic, when known.
    pub location: Option<(String, u32)>,
    /// When, in milliseconds since the Unix epoch.
    pub unix_ms: i64,
}

impl CrashReport {
    /// The report's text.
    #[must_use]
    pub fn text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "iamb crash report");
        let _ = writeln!(s, "process: {}", self.process);
        let _ = writeln!(s, "version: {VERSION}");
        let _ = writeln!(
            s,
            "system: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let _ = writeln!(s, "time: {}", clock::iso_utc(self.unix_ms));
        let _ = writeln!(s, "pid: {}", std::process::id());
        let _ = writeln!(s, "kind: panic");
        let _ = writeln!(s, "thread: {}", self.thread);
        match &self.location {
            Some((file, line)) => {
                let _ = writeln!(s, "location: {file}:{line}");
            }
            None => {
                let _ = writeln!(s, "location: unknown");
            }
        }
        let _ = writeln!(
            s,
            "Only metadata is recorded: no panic message, no memory (architecture §8 rule 9)."
        );
        s
    }

    /// Writes the report to `dir` (created user-only if needed) and removes the oldest reports
    /// of the same process beyond [`KEPT_REPORTS`]; returns the new file.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn write(&self, dir: &Path) -> io::Result<PathBuf> {
        crate::paths::create_private_dir(dir)?;
        let prefix = format!("crash-{}-", self.process);
        let mut n = 0;
        let (path, mut file) = loop {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            let path = dir.join(format!("{prefix}{}{suffix}.txt", self.unix_ms));
            match create_private_file(&path) {
                Ok(f) => break (path, f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 100 => n += 1,
                Err(e) => return Err(e),
            }
        };
        file.write_all(self.text().as_bytes())?;
        file.sync_all()?;
        prune(dir, &prefix);
        Ok(path)
    }
}

fn create_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Removes the oldest `<prefix>*.txt` files beyond [`KEPT_REPORTS`] (names sort by time).
fn prune(dir: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut reports: Vec<(i64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stamp = name.strip_prefix(prefix)?.strip_suffix(".txt")?;
            let ms = stamp.split('-').next()?.parse::<i64>().ok()?;
            Some((ms, e.path()))
        })
        .collect();
    reports.sort();
    let excess = reports.len().saturating_sub(KEPT_REPORTS);
    for (_, path) in reports.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// Replaces the panic hook of this process (module docs): a panic writes a crash report to
/// `logs`, logs one line by thread and location, and exits with [`EXIT_PANIC`].
pub fn install_panic_hook(log: &Arc<Logger>, logs: &Path, process: &'static str) {
    let log = Arc::clone(log);
    let logs = logs.to_path_buf();
    std::panic::set_hook(Box::new(move |info| {
        let report = CrashReport {
            process,
            thread: std::thread::current().name().unwrap_or("?").to_owned(),
            location: info.location().map(|l| (l.file().to_owned(), l.line())),
            unix_ms: clock::now_ms(),
        };
        let at = report
            .location
            .as_ref()
            .map_or_else(String::new, |(file, line)| format!(" at {file}:{line}"));
        let written = report.write(&logs).map_or_else(
            |e| format!("no crash report ({:?})", e.kind()),
            |path| {
                format!(
                    "crash report {}",
                    path.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                )
            },
        );
        log.error(format_args!(
            "panic in thread {}{at}; {written}; exiting with status {EXIT_PANIC}",
            report.thread
        ));
        std::process::exit(i32::from(EXIT_PANIC));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(ms: i64) -> CrashReport {
        CrashReport {
            process: "worker",
            thread: "iamb-engine".to_owned(),
            location: Some(("src/worker/engine.rs".to_owned(), 42)),
            unix_ms: ms,
        }
    }

    #[test]
    fn a_report_holds_metadata_only_and_old_ones_are_pruned() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let logs = dir.path().join("logs");
        let first = report(1_000).write(&logs).expect("written");
        let text = fs::read_to_string(&first).expect("readable");
        for line in [
            "process: worker",
            &format!("version: {VERSION}"),
            "thread: iamb-engine",
            "location: src/worker/engine.rs:42",
            "kind: panic",
        ] {
            assert!(text.contains(line), "{line:?} in {text}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&first).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // The same millisecond twice gets a second name.
        let again = report(1_000).write(&logs).expect("written");
        assert_ne!(first, again);
        for ms in 2_000..2_000 + i64::try_from(KEPT_REPORTS).expect("small") {
            report(ms).write(&logs).expect("written");
        }
        let left: Vec<String> = fs::read_dir(&logs)
            .expect("dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), KEPT_REPORTS);
        assert!(left.iter().all(|n| !n.starts_with("crash-worker-1000")));
    }

    #[test]
    fn the_summary_names_steps_and_errors_only() {
        let h = Hardening {
            steps: vec![("a", None), ("b", Some("PermissionDenied".to_owned()))],
        };
        assert_eq!(
            h.summary(),
            "crash dumps: a ok, b failed (PermissionDenied)"
        );
        assert!(!h.complete());
        assert!(!Hardening::default().complete());
        assert_eq!(
            Hardening::default().summary(),
            "crash dumps: no opt-out on this system"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_coredump_filter_is_written_and_checked() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let file = dir.path().join("coredump_filter");
        fs::write(&file, b"00000033\n").expect("write");
        set_coredump_filter(&file).expect("set");
        assert_eq!(fs::read_to_string(&file).expect("read"), "0");
    }
}
