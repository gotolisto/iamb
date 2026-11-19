//! The UI host's log: `logs/desktop.log`, plain text, rotated at 1 MB with five files kept
//! (architecture §3.3), like the service's own small logger.
//!
//! # What may be logged (architecture §8)
//!
//! Event names, IPC message names, durations, counts, ids, error codes, process ids. Never what
//! the user typed or entered: form values, palette queries, preview values, clipboard text, and
//! never an IPC message body. Call sites pass `format_args!`; the IPC types print only their
//! names through `Debug`, and user-entered values are `iamb_ipc::Secret`s, which print
//! `<redacted>`. `tests/crash_isolation.rs` checks the log after a session with form values.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

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
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        self.file = options.open(&path).ok();
        self.size = fs::metadata(&path).map_or(0, |m| m.len());
    }

    fn rotate(&mut self) {
        self.file = None;
        for n in (1..KEPT_FILES).rev() {
            let _ = fs::rename(self.path(n - 1), self.path(n));
        }
        self.open();
    }

    fn write(&mut self, line: &str) {
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if self.size > 0 && self.size.saturating_add(len) > MAX_FILE_BYTES {
            self.rotate();
        }
        if let Some(file) = self.file.as_mut()
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.size = self.size.saturating_add(len);
        }
    }
}

/// A file logger. Writing never fails the caller: a log that cannot be written is skipped.
pub struct Logger {
    sink: Mutex<Option<Sink>>,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger").finish_non_exhaustive()
    }
}

impl Logger {
    /// Logs to `<dir>/<name>.log`, creating `dir` user-only.
    #[must_use]
    pub fn open(dir: &Path, name: &str) -> Self {
        let _ = crate::paths::create_private_dir(dir);
        let mut sink = Sink {
            dir: dir.to_path_buf(),
            name: name.to_owned(),
            file: None,
            size: 0,
        };
        sink.open();
        Self {
            sink: Mutex::new(Some(sink)),
        }
    }

    /// A logger that drops everything.
    #[must_use]
    pub const fn discard() -> Self {
        Self {
            sink: Mutex::new(None),
        }
    }

    /// Writes one line at `level`.
    pub fn line(&self, level: Level, args: fmt::Arguments<'_>) {
        let mut guard = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(sink) = guard.as_mut() else {
            return;
        };
        // One line per record: a line break in the arguments cannot forge another record.
        let text = args.to_string().replace(['\r', '\n'], " ");
        sink.write(&format!("{} {} {text}\n", timestamp(), level.as_str()));
    }

    /// Normal operation.
    pub fn info(&self, args: fmt::Arguments<'_>) {
        self.line(Level::Info, args);
    }

    /// Something went wrong and was handled.
    pub fn warn(&self, args: fmt::Arguments<'_>) {
        self.line(Level::Warn, args);
    }

    /// Something went wrong and was not.
    pub fn error(&self, args: fmt::Arguments<'_>) {
        self.line(Level::Error, args);
    }
}

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for `unix_ms`.
#[must_use]
pub fn iso_utc(unix_ms: i64) -> String {
    let secs = unix_ms.div_euclid(1000);
    let ms = unix_ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn timestamp() -> String {
    iso_utc(now_ms())
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
const fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc(1_763_400_000_500), "2025-11-17T17:20:00.500Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn lines_are_single_and_files_rotate() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let log = Logger::open(dir.path(), "desktop");
        log.info(format_args!("first\nWARN forged"));
        let text = fs::read_to_string(dir.path().join("desktop.log")).expect("log");
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("INFO first WARN forged"));
        let big = "x".repeat(200_000);
        for _ in 0..30 {
            log.warn(format_args!("{big}"));
        }
        let files = fs::read_dir(dir.path()).expect("dir").count();
        assert_eq!(files, KEPT_FILES);
        Logger::discard().error(format_args!("dropped"));
    }
}
