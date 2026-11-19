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

/// Creates `dir` (and its parents) readable by the user only, as the service does: the Linux
/// endpoint refuses a `run` directory with a looser mode.
///
/// # Errors
///
/// I/O errors.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
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
}
