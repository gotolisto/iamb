//! The account's tokens: where they are kept and how the access token is renewed (§6.1,
//! architecture §3, ADR-0020).
//!
//! Tokens go to the operating system's secret store, behind [`TokenStore`]: the Keychain on macOS
//! and the Credential Manager on Windows ([`KeychainStore`]). On Linux, for development, they are
//! a file readable by the user alone in the data directory ([`FileStore`]), mode 0600 in a
//! directory of mode 0700. Only `iamb-sync` reads them. [`Tokens`] never prints a token: its
//! `Debug` says only whether each is there.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The session's tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// The access token (a JWT, one hour at most).
    pub access_token: String,
    /// The refresh token (used once and replaced).
    pub refresh_token: String,
    /// When the access token expires, Unix seconds.
    pub expires_at: i64,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Where the tokens are kept.
pub trait TokenStore: Send {
    /// The tokens, if there are any.
    ///
    /// # Errors
    ///
    /// The store cannot be read.
    fn load(&self) -> io::Result<Option<Tokens>>;
    /// Replaces the tokens.
    ///
    /// # Errors
    ///
    /// The store cannot be written.
    fn save(&self, tokens: &Tokens) -> io::Result<()>;
    /// Removes the tokens.
    ///
    /// # Errors
    ///
    /// The store cannot be written.
    fn clear(&self) -> io::Result<()>;
}

/// Tokens in a file readable by the user alone (Linux, for development).
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// The file `sync/tokens.json` under the data directory.
    #[must_use]
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("sync").join("tokens.json"),
        }
    }

    /// The file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

impl TokenStore for FileStore {
    fn load(&self) -> io::Result<Option<Tokens>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the token file is damaged")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn save(&self, tokens: &Tokens) -> io::Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("the token file has no directory"))?;
        private_dir(dir)?;
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(tokens).map_err(io::Error::other)?;
        write_private(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path)
    }

    fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Tokens in the operating system's secret store (the Keychain on macOS, the Credential Manager
/// on Windows), as one generic password named after the service and the account slot.
#[cfg(any(target_os = "macos", windows))]
pub struct KeychainStore {
    entry: keyring::Entry,
}

#[cfg(any(target_os = "macos", windows))]
impl KeychainStore {
    /// The entry `iamb-sync` / `session`.
    ///
    /// # Errors
    ///
    /// The platform's store is not available.
    pub fn new() -> io::Result<Self> {
        let entry = keyring::Entry::new("iamb-sync", "session").map_err(io::Error::other)?;
        Ok(Self { entry })
    }
}

#[cfg(any(target_os = "macos", windows))]
impl TokenStore for KeychainStore {
    fn load(&self) -> io::Result<Option<Tokens>> {
        match self.entry.get_password() {
            Ok(s) => serde_json::from_str(&s).map(Some).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "the stored tokens are damaged")
            }),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    fn save(&self, tokens: &Tokens) -> io::Result<()> {
        let s = serde_json::to_string(tokens).map_err(io::Error::other)?;
        self.entry.set_password(&s).map_err(io::Error::other)
    }

    fn clear(&self) -> io::Result<()> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(io::Error::other(e)),
        }
    }
}

/// The token store of this platform: the OS keychain on macOS and Windows, the private file on
/// other systems.
///
/// # Errors
///
/// The platform's store is not available.
pub fn platform_store(data_dir: &Path) -> io::Result<Box<dyn TokenStore>> {
    #[cfg(any(target_os = "macos", windows))]
    {
        let _ = data_dir;
        Ok(Box::new(KeychainStore::new()?))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(Box::new(FileStore::in_data_dir(data_dir)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_a_token() {
        let t = Tokens {
            access_token: "eyJ-secret".into(),
            refresh_token: "r-secret".into(),
            expires_at: 5,
        };
        let s = format!("{t:?}");
        assert!(!s.contains("secret"));
    }

    #[test]
    fn the_file_store_round_trips_and_is_private() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let store = FileStore::in_data_dir(dir.path());
        assert_eq!(store.load()?, None);
        let t = Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 9,
        };
        store.save(&t)?;
        assert_eq!(store.load()?, Some(t));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        store.clear()?;
        assert_eq!(store.load()?, None);
        Ok(())
    }
}
