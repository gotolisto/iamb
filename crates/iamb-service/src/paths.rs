//! Files on disk (architecture §3.3).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use iamb_platform::{EndpointKind, EndpointName};

/// The data root and the directories the service uses inside it.
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

    /// The data root: `--data-dir`, else `IAMB_DATA_DIR`, else the per-OS default
    /// (`~/Library/Application Support/Iamb`, `%APPDATA%\Iamb`, `$XDG_DATA_HOME/iamb`).
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

    /// `run/`: locks and the socket.
    #[must_use]
    pub fn run(&self) -> PathBuf {
        self.root.join("run")
    }

    /// `logs/`.
    #[must_use]
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `run/supervisor.lock`.
    #[must_use]
    pub fn supervisor_lock(&self) -> PathBuf {
        self.run().join("supervisor.lock")
    }

    /// The service endpoint's name.
    #[must_use]
    pub fn service_endpoint(&self) -> EndpointName {
        EndpointName::new(EndpointKind::Service, self.run())
    }
}

fn nonempty(v: Option<OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The default data root, reading the environment through `get`.
pub(crate) fn default_root(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
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

/// Creates `dir` (and its parents) readable by the user only.
///
/// # Errors
///
/// I/O errors.
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
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_variable_wins() {
        let root = default_root(|k| (k == "IAMB_DATA_DIR").then(|| OsString::from("/x")));
        assert_eq!(root, Some(PathBuf::from("/x")));
        let dirs = DataDirs::new("/x");
        assert_eq!(dirs.run(), Path::new("/x/run"));
        assert_eq!(dirs.logs(), Path::new("/x/logs"));
        assert_eq!(dirs.supervisor_lock(), Path::new("/x/run/supervisor.lock"));
        assert_eq!(
            dirs.service_endpoint().socket_path(),
            Path::new("/x/run/service.sock")
        );
    }

    #[test]
    fn nothing_known_is_none() {
        assert_eq!(default_root(|_| None), None);
    }
}
