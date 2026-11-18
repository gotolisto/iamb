//! A tiny file logger: `logs/<process>.log`, plain text, rotated at 1 MB with five files kept
//! (architecture §3.3).
//!
//! # Why in-house
//!
//! A logging framework would be one more dependency in the process that sees keystrokes, and
//! the popular ones bring optional network appenders or structured-data machinery the service
//! does not need. This is about a hundred lines: a mutex, a file, a size counter, a rename
//! chain.
//!
//! # What may be logged
//!
//! Never typed characters, form values, clipboard text, palette queries, dry-run input or IPC
//! message bodies (§8). Log lines name events, message names, sizes, durations, counts, ids,
//! error codes and process ids. Call sites pass `format_args!`; there is no way to log a
//! `Sensitive` value because it implements neither `Debug` nor `Display`, and IPC messages'
//! `Debug` prints only their names. `tests/log_scan.rs` types canary strings through the whole
//! service and checks that no log file contains them.
//!
//! # Files
//!
//! `<name>.log` is the current file; when a line would take it past [`MAX_FILE_BYTES`], it is
//! renamed to `<name>.1.log`, `<name>.1.log` to `<name>.2.log`, and so on up to
//! `<name>.4.log`, whose old content is dropped: at most [`KEPT_FILES`] files of 1 MB.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::clock;

/// The size at which the current file is rotated.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// How many files are kept, the current one included.
pub const KEPT_FILES: usize = 5;

/// How serious a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Normal operation.
    Info,
    /// Something went wrong and was handled.
    Warn,
    /// Something went wrong and was not.
    Error,
}

impl Level {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

struct Sink {
    dir: PathBuf,
    name: String,
    file: Option<File>,
    size: u64,
}

impl Sink {
    fn path(&self, n: usize) -> PathBuf {
        if n == 0 {
            self.dir.join(format!("{}.log", self.name))
        } else {
            self.dir.join(format!("{}.{n}.log", self.name))
        }
    }

    fn open(&mut self) {
        let path = self.path(0);
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        self.size = fs::metadata(&path).map_or(0, |m| m.len());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        let _ = fs::remove_file(self.path(KEPT_FILES - 1));
        for n in (0..KEPT_FILES - 1).rev() {
            let _ = fs::rename(self.path(n), self.path(n + 1));
        }
        self.open();
    }

    fn write(&mut self, line: &str) {
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if self.size > 0 && self.size + len > MAX_FILE_BYTES {
            self.rotate();
        }
        if let Some(file) = &mut self.file
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.size += len;
        }
    }
}

/// The logger of one process. Shared as `Arc<Logger>`; every method takes `&self`.
pub struct Logger {
    sink: Option<Mutex<Sink>>,
    process: String,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("process", &self.process)
            .field("enabled", &self.sink.is_some())
            .finish()
    }
}

impl Logger {
    /// A logger writing `dir/<process>.log`, creating `dir` (user-only) if needed. If the file
    /// cannot be opened, logging is silently off: the service must run without a log.
    #[must_use]
    pub fn open(dir: &Path, process: &str) -> Self {
        let _ = crate::paths::create_private_dir(dir);
        let mut sink = Sink {
            dir: dir.to_path_buf(),
            name: process.to_owned(),
            file: None,
            size: 0,
        };
        sink.open();
        Self {
            sink: Some(Mutex::new(sink)),
            process: process.to_owned(),
        }
    }

    /// A logger that writes nothing.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            sink: None,
            process: String::new(),
        }
    }

    /// Writes one line: time, level, process, message.
    pub fn log(&self, level: Level, message: fmt::Arguments<'_>) {
        let Some(sink) = &self.sink else {
            return;
        };
        let mut line = format!(
            "{} {} {}[{}]: {message}",
            clock::iso_utc(clock::now_ms()),
            level.as_str(),
            self.process,
            std::process::id()
        );
        // One record per line, whatever the message holds.
        line.retain(|c| c != '\n' && c != '\r');
        line.push('\n');
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(&line);
    }

    /// An info line.
    pub fn info(&self, message: fmt::Arguments<'_>) {
        self.log(Level::Info, message);
    }

    /// A warning.
    pub fn warn(&self, message: fmt::Arguments<'_>) {
        self.log(Level::Warn, message);
    }

    /// An error.
    pub fn error(&self, message: fmt::Arguments<'_>) {
        self.log(Level::Error, message);
    }
}

/// IPC frame records go to the log: name, direction, size, duration, error code.
impl iamb_ipc::MessageLog for Logger {
    fn record(&self, r: &iamb_ipc::MessageRecord) {
        let dir = match r.direction {
            iamb_ipc::Direction::In => "in",
            iamb_ipc::Direction::Out => "out",
        };
        match (r.duration, r.error) {
            (Some(d), Some(code)) => self.info(format_args!(
                "ipc {dir} {} {} bytes {} us error {code}",
                r.name,
                r.size,
                d.as_micros()
            )),
            (Some(d), None) => self.info(format_args!(
                "ipc {dir} {} {} bytes {} us",
                r.name,
                r.size,
                d.as_micros()
            )),
            (None, Some(code)) => {
                self.info(format_args!(
                    "ipc {dir} {} {} bytes error {code}",
                    r.name, r.size
                ));
            }
            (None, None) => self.info(format_args!("ipc {dir} {} {} bytes", r.name, r.size)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_one_megabyte_and_keeps_five_files() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let log = Logger::open(dir.path(), "test");
        let line = "x".repeat(1000);
        // About 6.5 MB in total: more than five files' worth.
        for i in 0..6500 {
            log.info(format_args!("{i} {line}"));
        }
        let mut files: Vec<String> = fs::read_dir(dir.path())
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "test.1.log",
                "test.2.log",
                "test.3.log",
                "test.4.log",
                "test.log"
            ]
        );
        for f in &files {
            let len = fs::metadata(dir.path().join(f)).expect("meta").len();
            assert!(len <= MAX_FILE_BYTES, "{f} is {len} bytes");
        }
        let current = fs::read_to_string(dir.path().join("test.log")).expect("read");
        assert!(current.lines().last().expect("a line").contains(" 6499 "));
        assert!(current.lines().all(|l| l.contains("INFO test[")));
    }

    #[test]
    fn one_record_per_line_and_disabled_writes_nothing() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let log = Logger::open(dir.path(), "p");
        log.warn(format_args!("two\nlines"));
        log.error(format_args!("e"));
        let text = fs::read_to_string(dir.path().join("p.log")).expect("read");
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("WARN p[") && text.contains("twolines"));
        Logger::disabled().info(format_args!("nowhere"));
        assert!(format!("{:?}", Logger::disabled()).contains("enabled: false"));
    }
}
