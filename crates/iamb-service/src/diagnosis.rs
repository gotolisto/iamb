//! The files of `iamb-service --diagnose-focus` (`main.rs`): where they go, how many are kept,
//! how large one may grow, and what the first line says.
//!
//! A run writes `logs/focus-diagnosis-<unix ms>.txt` in the data root (`%APPDATA%\Iamb\logs` on
//! Windows), the user-only folder of the logs. Such a file names the applications that were in
//! front and when, which the service's own log never writes (the late-answer review's L5); so
//! (the cached-probe review's L5):
//!
//! - at most [`KEEP`] are kept: before a run writes its file, older ones are removed so that,
//!   with the new one, [`KEEP`] remain (the crash reports' rule, with a smaller number);
//! - a file stops at [`MAX_BYTES`], and its last line says so;
//! - its first line ([`HEADER`]) says what it holds before anyone sends it.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The most diagnosis files kept, the new one included.
pub const KEEP: usize = 5;

/// The largest a diagnosis file grows.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The start of a diagnosis file's name; the rest is the unix milliseconds of the run and
/// `.txt`.
pub const PREFIX: &str = "focus-diagnosis-";

/// The first line of every diagnosis file.
pub const HEADER: &str = "Iamb focus diagnosis. This file names the applications that were in front, with their window classes and the times; it holds no text, no window title and nothing typed. Send it only to whom you choose.";

/// The unix milliseconds in the name of a diagnosis file, if it is one.
fn stamp_of(name: &str) -> Option<u128> {
    name.strip_prefix(PREFIX)?
        .strip_suffix(".txt")?
        .parse()
        .ok()
}

/// Removes the oldest diagnosis files in `dir`, so that at most `KEEP - 1` remain for the new
/// one to make `KEEP`. Other files are not touched; a file that cannot be removed is left.
pub fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<(u128, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let stamp = stamp_of(entry.file_name().to_str()?)?;
            Some((stamp, entry.path()))
        })
        .collect();
    found.sort();
    let surplus = found.len().saturating_sub(KEEP - 1);
    for (_, path) in found.into_iter().take(surplus) {
        let _ = fs::remove_file(path);
    }
}

/// The path of a new diagnosis file in `dir`, for a run at `stamp` unix milliseconds.
#[must_use]
pub fn path_for(dir: &Path, stamp: u128) -> PathBuf {
    dir.join(format!("{PREFIX}{stamp}.txt"))
}

/// A diagnosis file that stops at a size limit.
#[derive(Debug)]
pub struct Capped<W: Write> {
    out: W,
    written: u64,
    limit: u64,
    full: bool,
}

impl Capped<File> {
    /// Creates the file at `path`, never over an existing one, and writes [`HEADER`].
    ///
    /// # Errors
    ///
    /// When the file cannot be created or written.
    pub fn create(path: &Path) -> io::Result<Self> {
        Self::new(File::create_new(path)?, MAX_BYTES)
    }
}

impl<W: Write> Capped<W> {
    /// Writes to `out`, at most `limit` bytes; [`HEADER`] first.
    ///
    /// # Errors
    ///
    /// When the header cannot be written.
    pub fn new(out: W, limit: u64) -> io::Result<Self> {
        let mut capped = Self {
            out,
            written: 0,
            limit,
            full: false,
        };
        capped.write_line(HEADER)?;
        Ok(capped)
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.out, "{line}")?;
        self.written += u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        Ok(())
    }

    /// Writes `line` if it fits; once one does not, writes the line that says the file stopped
    /// there, and nothing after. False once the file is full.
    pub fn line(&mut self, line: &str) -> bool {
        if self.full {
            return false;
        }
        let len = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if self.written.saturating_add(len) > self.limit {
            self.full = true;
            let _ = self.write_line("stopped here: the file reached its size limit");
            return false;
        }
        self.write_line(line).is_ok()
    }

    /// What was written.
    pub fn into_inner(self) -> W {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_files_go_and_others_stay() {
        let dir = tempfile::tempdir().expect("dir");
        for stamp in [5_u128, 1, 3, 2, 4, 6] {
            fs::write(path_for(dir.path(), stamp), "x").expect("written");
        }
        fs::write(dir.path().join("worker.log"), "x").expect("written");
        fs::write(dir.path().join("focus-diagnosis-old.txt"), "x").expect("written");
        prune(dir.path());
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .expect("listed")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "focus-diagnosis-3.txt",
                "focus-diagnosis-4.txt",
                "focus-diagnosis-5.txt",
                "focus-diagnosis-6.txt",
                "focus-diagnosis-old.txt",
                "worker.log",
            ]
        );
    }

    #[test]
    fn a_file_says_what_it_holds_and_stops_at_its_limit() {
        let mut file = Capped::new(Vec::new(), 400).expect("header");
        let mut kept = 0;
        while file.line("a line of the diagnosis") {
            kept += 1;
        }
        assert!(!file.line("after the end"));
        let text = String::from_utf8(file.into_inner()).expect("text");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], HEADER);
        assert!(HEADER.contains("names the applications that were in front"));
        assert_eq!(
            lines.last(),
            Some(&"stopped here: the file reached its size limit")
        );
        assert_eq!(lines.len(), kept + 2);
        assert!(text.len() <= 400 + 60, "{}", text.len());
        assert!(!text.contains("after the end"));
    }
}
