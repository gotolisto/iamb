//! Files on disk (architecture §3.3): the same data root as the service, so that the UI host
//! finds the service endpoint and writes its log beside the service's.
//!
//! The resolution order is the service's (`iamb_service::paths::DataDirs::resolve`):
//! `--data-dir`, else `IAMB_DATA_DIR`, else the per-OS default. It is repeated here because the
//! UI host does not depend on the service crate (architecture §2).

use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use iamb_platform::{EndpointKind, EndpointName};

/// The data root and the parts of it the UI host uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    root: PathBuf,
}

impl DataDirs {
    /// The directories of the data root `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `flag`, else `IAMB_DATA_DIR`, else the per-OS default (`~/Library/Application
    /// Support/Iamb`, `%APPDATA%\Iamb`, `$XDG_DATA_HOME/iamb`).
    ///
    /// # Errors
    ///
    /// When no location can be derived from the environment.
    pub fn resolve(flag: Option<&Path>) -> io::Result<Self> {
        if let Some(p) = flag {
            return Ok(Self::new(p));
        }
        default_root(|k| std::env::var_os(k))
            .map(Self::new)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot find a data directory; pass --data-dir or set IAMB_DATA_DIR",
                )
            })
    }

    /// The data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `run/`: locks and the service endpoint.
    #[must_use]
    pub fn run(&self) -> PathBuf {
        self.root.join("run")
    }

    /// `logs/`.
    #[must_use]
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// The service endpoint's name.
    #[must_use]
    pub fn service_endpoint(&self) -> EndpointName {
        EndpointName::new(EndpointKind::Service, self.run())
    }

    /// `run/desktop.quit`, which this UI host leaves when it ends on purpose (see
    /// [`DataDirs::mark_quit`]).
    #[must_use]
    pub fn quit_marker(&self) -> PathBuf {
        self.run().join(QUIT_MARKER)
    }

    /// Leaves `run/desktop.quit`: this UI host ends on purpose (the tray's "Quit", or a quit by
    /// the system, such as a logout on macOS). A UI host that steps aside for another one leaves
    /// none (`main`). On macOS, where the supervisor has the system start the UI host, this
    /// marker is the only way it can tell a quit from a crash; where it sees the UI host's own
    /// exit status, the status decides (`iamb_service::supervisor`, "The UI host").
    ///
    /// # Errors
    ///
    /// As [`create_flag_file`].
    pub fn mark_quit(&self) -> io::Result<()> {
        create_private_dir(&self.run())?;
        create_flag_file(&self.quit_marker())
    }

    /// Takes `run/desktop.lock`, or `None` when another UI host of this data root holds it.
    /// Two UI hosts would both hear `FormRequested` and show two forms.
    ///
    /// # Errors
    ///
    /// I/O errors other than the lock being held.
    pub fn take_instance_lock(&self) -> io::Result<Option<File>> {
        create_private_dir(&self.run())?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.run().join("desktop.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

/// The file name of [`DataDirs::quit_marker`]: the same name as
/// `iamb_service::paths::UI_HOST_QUIT_MARKER` (a test compares the two).
pub const QUIT_MARKER: &str = "desktop.quit";

/// Creates the empty file `path` as a signal to the supervisor (the quit marker, the restart
/// request), without following a link or opening what is already there (the 2026-11 beta
/// review's finding 5): the file is created new, so a symbolic link in its place is neither
/// followed nor truncated, and a FIFO is not opened (which would wait for a reader). Something
/// already in its place counts as the signal given, since the supervisor takes it by removing
/// it, except a directory, which it cannot remove that way.
///
/// # Errors
///
/// I/O errors, and `AlreadyExists` when a directory is in the file's place.
pub fn create_flag_file(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::symlink_metadata(path)?.is_dir() {
                Err(e)
            } else {
                Ok(())
            }
        }
        Err(e) => Err(e),
    }
}

fn nonempty(v: Option<OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The default data root, reading the environment through `get`.
fn default_root(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = nonempty(get("IAMB_DATA_DIR")) {
        return Some(p);
    }
    if cfg!(target_os = "macos") {
        nonempty(get("HOME")).map(|h| h.join("Library/Application Support/Iamb"))
    } else if cfg!(windows) {
        nonempty(get("APPDATA")).map(|a| a.join("Iamb"))
    } else {
        nonempty(get("XDG_DATA_HOME"))
            .filter(|p| p.has_root())
            .map(|x| x.join("iamb"))
            .or_else(|| nonempty(get("HOME")).map(|h| h.join(".local/share/iamb")))
    }
}

/// Creates `dir` (and its parents) readable by the user only, and makes an existing `dir` so,
/// as the service does (`iamb_service::paths::create_private_dir`; the 2026-11 beta review's
/// finding 5): mode `0700` on Unix, where the Linux endpoint refuses a `run` directory with a
/// looser mode; on Windows a protected DACL for the user alone
/// (`iamb_platform_windows::restrict_to_current_user`). The UI host can be the first to create
/// `run/`.
///
/// # Errors
///
/// I/O errors, and the system's error when the permissions cannot be set.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(dir)?;
        iamb_platform_windows::restrict_to_current_user(dir)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_wins_and_empty_values_are_ignored() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        assert_eq!(
            default_root(env(&[("IAMB_DATA_DIR", "/data"), ("HOME", "/home/ada")])),
            Some(PathBuf::from("/data"))
        );
        let home = default_root(env(&[("IAMB_DATA_DIR", ""), ("HOME", "/home/ada")]));
        if cfg!(windows) {
            assert_eq!(home, None);
        } else {
            assert!(home.is_some_and(|p| p.starts_with("/home/ada")));
        }
    }

    #[test]
    fn one_ui_host_per_data_root() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        let first = dirs.take_instance_lock().expect("io").expect("free");
        assert!(dirs.take_instance_lock().expect("io").is_none());
        drop(first);
        assert!(dirs.take_instance_lock().expect("io").is_some());
        assert_eq!(
            dirs.service_endpoint().socket_path(),
            dir.path().join("run/service.sock")
        );
    }

    #[test]
    fn the_quit_marker_is_left_in_run() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dirs = DataDirs::new(dir.path());
        assert_eq!(dirs.quit_marker(), dir.path().join("run").join(QUIT_MARKER));
        dirs.mark_quit().expect("marked");
        assert!(dirs.quit_marker().is_file());
        // A second quit before the supervisor took the first is fine.
        dirs.mark_quit().expect("marked again");
    }

    #[cfg(unix)]
    #[test]
    fn a_flag_file_never_follows_a_link_or_opens_a_fifo() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("temp dir");
        // A link to a file of the user's: neither followed nor truncated.
        let precious = dir.path().join("precious");
        std::fs::write(&precious, b"keep me").expect("file");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&precious, &link).expect("link");
        create_flag_file(&link).expect("counts as given");
        assert_eq!(std::fs::read(&precious).expect("read"), b"keep me");
        // A dangling link: not followed to create its target.
        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("target"), &dangling).expect("link");
        create_flag_file(&dangling).expect("counts as given");
        assert!(!dir.path().join("target").exists());
        // A FIFO: returns at once instead of waiting for a reader.
        let fifo = dir.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        create_flag_file(&fifo).expect("counts as given");
        // A directory cannot be taken by the supervisor: an error.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("dir");
        assert_eq!(
            create_flag_file(&sub).expect_err("a directory").kind(),
            io::ErrorKind::AlreadyExists
        );
        // A new file is the user's alone.
        let new = dir.path().join("new");
        create_flag_file(&new).expect("created");
        let mode = std::fs::metadata(&new).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_run_directory_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("temp dir");
        let run = dir.path().join("run");
        std::fs::create_dir(&run).expect("dir");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        create_private_dir(&run).expect("private");
        let mode = std::fs::metadata(&run).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
