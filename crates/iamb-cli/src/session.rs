//! Locating the data directory and opening the store with a first-use profile.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iamb_core::model::{Folder, FolderDefaults, Id, Profile, ProfileKind, SyncMeta, Timestamp};
use iamb_store::{ListOptions, Store};

use crate::error::CliError;
use crate::ids;

/// How long the command-line tool waits for the store lock (architecture §6.3).
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// Name of the root folder created on first use.
pub const ROOT_FOLDER: &str = "Snippets";

/// An open store with the personal profile.
pub struct Session {
    /// The store.
    pub store: Store,
    /// The personal profile's id.
    pub profile: Id,
    /// Time of this command.
    pub now: Timestamp,
}

/// Picks the data directory: `--data-dir`, else `IAMB_DATA_DIR`, else the per-OS default.
pub fn resolve_data_dir(flag: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    data_dir_from_env(Os::current(), |k| std::env::var_os(k))
}

/// The operating-system families with their own default data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    /// `~/Library/Application Support/Iamb`.
    Macos,
    /// `%APPDATA%\Iamb`.
    Windows,
    /// `$XDG_DATA_HOME/iamb`, else `~/.local/share/iamb`.
    Unix,
}

impl Os {
    const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

fn nonempty(v: Option<OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The data directory for `os`, reading environment variables through `get` (so tests do not
/// touch the process environment).
fn data_dir_from_env(os: Os, get: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, CliError> {
    if let Some(p) = nonempty(get("IAMB_DATA_DIR")) {
        return Ok(p);
    }
    match os {
        Os::Macos => {
            if let Some(home) = nonempty(get("HOME")) {
                return Ok(home.join("Library/Application Support/Iamb"));
            }
        }
        Os::Windows => {
            if let Some(appdata) = nonempty(get("APPDATA")) {
                return Ok(appdata.join("Iamb"));
            }
        }
        Os::Unix => {
            if let Some(xdg) = nonempty(get("XDG_DATA_HOME")).filter(|p| p.is_absolute()) {
                return Ok(xdg.join("iamb"));
            }
            if let Some(home) = nonempty(get("HOME")) {
                return Ok(home.join(".local/share/iamb"));
            }
        }
    }
    Err(CliError::new(
        "cannot find a data directory; pass --data-dir or set IAMB_DATA_DIR",
    ))
}

impl Session {
    /// Opens the store (waiting up to 3 s for its lock) and makes sure a personal profile and a
    /// root folder exist.
    pub fn open(data_dir: &Path) -> Result<Self, CliError> {
        let mut store = Store::open_with_timeout(data_dir, LOCK_WAIT)?;
        let now = ids::now();
        let existing = store
            .list_profiles(ListOptions::LIVE)?
            .into_iter()
            .find(|p| p.kind == ProfileKind::Personal);
        let profile = if let Some(p) = existing {
            p.meta.id
        } else {
            let id = ids::new_id();
            store.put_profile(
                &Profile {
                    meta: SyncMeta::new(id, now),
                    kind: ProfileKind::Personal,
                    name: "Personal".to_owned(),
                    owner_user_id: ids::new_id(),
                },
                now,
            )?;
            id
        };
        let has_root = store
            .list_folders(ListOptions::LIVE)?
            .iter()
            .any(|f| f.profile_id == profile && f.parent_id.is_none());
        if !has_root {
            store.put_folder(&new_folder(profile, None, ROOT_FOLDER, 0, now), now)?;
        }
        Ok(Self {
            store,
            profile,
            now,
        })
    }
}

/// A new folder row.
pub fn new_folder(
    profile: Id,
    parent_id: Option<Id>,
    name: &str,
    position: i64,
    now: Timestamp,
) -> Folder {
    Folder {
        meta: SyncMeta::new(ids::new_id(), now),
        profile_id: profile,
        parent_id,
        name: name.to_owned(),
        position,
        member_permission: None,
        defaults: FolderDefaults::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    fn resolve(os: Os, pairs: &[(&str, &str)]) -> Option<PathBuf> {
        data_dir_from_env(os, env(pairs)).ok()
    }

    #[test]
    fn env_var_wins_on_every_os() {
        for os in [Os::Macos, Os::Windows, Os::Unix] {
            let vars = [
                ("IAMB_DATA_DIR", "/x"),
                ("HOME", "/h"),
                ("APPDATA", "/a"),
                ("XDG_DATA_HOME", "/xdg"),
            ];
            assert_eq!(resolve(os, &vars), Some(PathBuf::from("/x")), "{os:?}");
        }
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let vars = [("IAMB_DATA_DIR", ""), ("HOME", "/h")];
        assert_eq!(
            resolve(Os::Unix, &vars),
            Some(PathBuf::from("/h/.local/share/iamb"))
        );
    }

    #[test]
    fn macos_uses_home_library() {
        let vars = [("HOME", "/Users/ada"), ("XDG_DATA_HOME", "/xdg")];
        assert_eq!(
            resolve(Os::Macos, &vars),
            Some(PathBuf::from("/Users/ada/Library/Application Support/Iamb"))
        );
        assert_eq!(resolve(Os::Macos, &[("APPDATA", "/a")]), None);
    }

    #[test]
    fn windows_uses_appdata() {
        let vars = [("APPDATA", "/roaming"), ("HOME", "/h")];
        assert_eq!(
            resolve(Os::Windows, &vars),
            Some(PathBuf::from("/roaming").join("Iamb"))
        );
        assert_eq!(resolve(Os::Windows, &[("HOME", "/h")]), None);
    }

    #[test]
    fn unix_prefers_an_absolute_xdg_data_home_then_home() {
        assert_eq!(
            resolve(Os::Unix, &[("XDG_DATA_HOME", "/xdg"), ("HOME", "/h")]),
            Some(PathBuf::from("/xdg/iamb"))
        );
        // A relative XDG_DATA_HOME is ignored, as the XDG specification says.
        assert_eq!(
            resolve(Os::Unix, &[("XDG_DATA_HOME", "rel"), ("HOME", "/h")]),
            Some(PathBuf::from("/h/.local/share/iamb"))
        );
        assert_eq!(
            resolve(Os::Unix, &[("HOME", "/h")]),
            Some(PathBuf::from("/h/.local/share/iamb"))
        );
        assert_eq!(resolve(Os::Unix, &[("APPDATA", "/a")]), None);
    }

    #[test]
    fn missing_everything_is_an_error() {
        for os in [Os::Macos, Os::Windows, Os::Unix] {
            assert!(data_dir_from_env(os, env(&[])).is_err(), "{os:?}");
        }
    }

    #[test]
    fn the_flag_skips_the_environment() {
        let dir = resolve_data_dir(Some(Path::new("/flag")));
        assert_eq!(dir.ok(), Some(PathBuf::from("/flag")));
    }
}
