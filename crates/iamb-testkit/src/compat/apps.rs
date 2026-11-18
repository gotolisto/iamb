//! The apps of the matrix, from `fixtures/compat-apps.toml`.
//!
//! `docs/compatibility-matrix.md` section 1 is the reference; `tests/compat.rs` checks that
//! the ids, names, surfaces and flags here agree with it.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// The app list, compiled in.
pub const APPS_TOML: &str = include_str!("../../fixtures/compat-apps.toml");

/// The operating system of a matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// Windows 11.
    Windows,
    /// macOS.
    Macos,
}

impl Os {
    /// The lower-case name used in files and on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
        }
    }

    /// The prefix of the app ids: `W` or `M`.
    pub const fn id_prefix(self) -> char {
        match self {
            Self::Windows => 'W',
            Self::Macos => 'M',
        }
    }

    /// The OS this binary runs on, if it has a matrix.
    pub const fn current() -> Option<Self> {
        if cfg!(windows) {
            Some(Self::Windows)
        } else if cfg!(target_os = "macos") {
            Some(Self::Macos)
        } else {
            None
        }
    }
}

/// One row of the matrix.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per column of the app file"
)]
pub struct App {
    /// `W1` to `W20`, `M1` to `M20`.
    pub id: String,
    /// The matrix it belongs to.
    pub os: Os,
    /// The application's name.
    pub name: String,
    /// What to open and where the caret goes.
    pub surface: String,
    /// T5 is checked for bold, link and image structure.
    #[serde(default)]
    pub rich: bool,
    /// A password field: nothing may expand.
    #[serde(default)]
    pub secure: bool,
    /// The harness may type and read back by itself.
    #[serde(default)]
    pub auto: bool,
    /// A one-line field: newlines are dropped from T2.
    #[serde(default)]
    pub single_line: bool,
    /// An extra check to do once on this row.
    #[serde(default)]
    pub extra: Option<String>,
    /// Known quirks, from the matrix.
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize)]
struct File {
    app: Vec<App>,
}

/// Every app of both matrices, in file order.
pub fn all_apps() -> Result<Vec<App>, String> {
    parse(APPS_TOML)
}

/// Parses an app list.
pub fn parse(text: &str) -> Result<Vec<App>, String> {
    let file: File = toml::from_str(text).map_err(|e| format!("compat-apps.toml: {e}"))?;
    Ok(file.app)
}

/// The apps of one OS, in matrix order.
pub fn apps_for(os: Os) -> Result<Vec<App>, String> {
    Ok(all_apps()?.into_iter().filter(|a| a.os == os).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_apps_per_os_with_the_right_prefix() {
        for os in [Os::Windows, Os::Macos] {
            let apps = apps_for(os).unwrap_or_default();
            assert_eq!(apps.len(), 20, "{os:?}");
            for (i, app) in apps.iter().enumerate() {
                assert_eq!(app.id, format!("{}{}", os.id_prefix(), i + 1));
            }
            assert_eq!(apps.iter().filter(|a| a.secure).count(), 2);
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let bad =
            "[[app]]\nid = \"W1\"\nos = \"windows\"\nname = \"x\"\nsurface = \"y\"\nsurfce = 1\n";
        assert!(parse(bad).is_err());
    }
}
