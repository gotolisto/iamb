//! Who belongs to the same installation as this process, and when a peer of the local endpoint
//! is trusted under [`PeerPolicy::InstalledBinary`](crate::PeerPolicy) (architecture §6.1, the
//! 2.8 privacy review's finding 11).
//!
//! The OS crates gather the facts (the peer's executable as the system reports it, its
//! canonical path, its file identity, its signature) and decide with the pure functions here,
//! so that the decision is tested on every system.
//!
//! # The installation
//!
//! An installation is found from this process's own canonical executable:
//!
//! - **macOS**: `<X>.app/Contents/MacOS/iamb-desktop` (the UI host) or
//!   `<X>.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service` (the
//!   helper). Its members are those two, and `iamb` and `iamb-sync` in `<X>.app/Contents/MacOS`
//!   should they ever ship there.
//! - **Windows**: an executable whose directory is the per-user installation directory the
//!   installer uses, `%LOCALAPPDATA%\Iamb`. Its members are `iamb-desktop.exe`,
//!   `iamb-service.exe`, `iamb.exe` and `iamb-sync.exe` in that directory.
//!
//! Anywhere else (a development build, a copy elsewhere) this process belongs to no
//! installation and trusts no peer under that policy.
//!
//! # The decision
//!
//! | This process | The peer | Trusted |
//! | --- | --- | --- |
//! | Not in an installation | anything | no |
//! | Signed (macOS: a team identifier; Windows: a valid Authenticode signature) | a member, reported without a link, signed by the same signer (macOS: satisfies `anchor apple generic`, the team identifier and Iamb's bundle identifiers; Windows: the same leaf certificate) | yes |
//! | Signed | a member, but unsigned, ad hoc, invalid or another signer | **no** |
//! | Unsigned (macOS: ad hoc or none; Windows: no valid signature) | a member, reported without a link, and the same file as the member (macOS: device and inode) | yes |
//! | either | not a member: another folder, a copy of `iamb-desktop` elsewhere, a link to a member, a hard link outside the installation | no |
//!
//! "Unsigned" is decided from this process's own image only, so a signed installation never
//! falls back to the path check: it cannot be talked into accepting an unsigned peer. An
//! unsigned installation trusts what is at its members' paths; whoever can write there can
//! replace the whole installation anyway.

use std::path::{Path, PathBuf};

/// The macOS bundle identifiers of an installation's code, for the signed requirement.
pub const MAC_BUNDLE_IDS: [&str; 2] = ["org.iamb.Iamb", "org.iamb.Iamb.Service"];

/// The helper app's path inside the main app bundle (ADR-0021).
pub const MAC_HELPER: &str = "Contents/Library/LoginItems/Iamb Service.app";

/// The clients and the service, by executable name without `.exe`.
pub const MEMBER_NAMES: [&str; 4] = ["iamb-desktop", "iamb-service", "iamb", "iamb-sync"];

/// The per-user installation directory under `%LOCALAPPDATA%` on Windows (the installer's,
/// `apps/desktop/release/tauri.windows.json`).
pub const WINDOWS_INSTALL_DIR: &str = "Iamb";

/// One executable of an installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Its path.
    pub exe: PathBuf,
    /// The app bundle it is the main executable of (macOS), which the code-signing API may
    /// report instead of the executable.
    pub bundle: Option<PathBuf>,
}

/// The members of the macOS installation that `own` (this process's canonical executable)
/// belongs to; `None` when it is in none.
#[must_use]
pub fn macos_members(own: &Path) -> Option<Vec<Member>> {
    let root = macos_root(own)?;
    let macos = root.join("Contents/MacOS");
    let helper = root.join(MAC_HELPER);
    Some(vec![
        Member {
            exe: macos.join("iamb-desktop"),
            bundle: Some(root.clone()),
        },
        Member {
            exe: helper.join("Contents/MacOS/iamb-service"),
            bundle: Some(helper),
        },
        Member {
            exe: macos.join("iamb"),
            bundle: None,
        },
        Member {
            exe: macos.join("iamb-sync"),
            bundle: None,
        },
    ])
}

/// The main app bundle around `own`, when `own` is the UI host or the helper of one.
fn macos_root(own: &Path) -> Option<PathBuf> {
    let name = own.file_name()?.to_str()?;
    let dir = own.parent()?;
    let root = match name {
        "iamb-desktop" => dir.strip_suffix_path("Contents/MacOS")?,
        "iamb-service" => dir
            .strip_suffix_path("Contents/MacOS")?
            .strip_suffix_path(MAC_HELPER)?,
        _ => return None,
    };
    root.extension()
        .is_some_and(|e| e == "app")
        .then(|| root.to_path_buf())
}

/// `Path` without a trailing run of components.
trait StripSuffixPath {
    fn strip_suffix_path(&self, suffix: &str) -> Option<&Path>;
}

impl StripSuffixPath for Path {
    fn strip_suffix_path(&self, suffix: &str) -> Option<&Path> {
        let mut path = self;
        for part in Path::new(suffix).components().rev() {
            if path.file_name()? != part.as_os_str() {
                return None;
            }
            path = path.parent()?;
        }
        Some(path)
    }
}

/// The members of the Windows installation that `own` (this process's canonical executable)
/// belongs to, given `%LOCALAPPDATA%`; `None` when `own` is not one of them in the per-user
/// installation directory.
#[must_use]
pub fn windows_members(own: &Path, local_app_data: Option<&Path>) -> Option<Vec<Member>> {
    let own_text = windows_key(own);
    let (dir, name) = own_text.rsplit_once('\\')?;
    if !MEMBER_NAMES
        .iter()
        .any(|m| name == format!("{m}.exe").as_str())
    {
        return None;
    }
    let expected = windows_key(&local_app_data?.join(WINDOWS_INSTALL_DIR));
    if dir != expected {
        return None;
    }
    // Built from the text, not with `Path::parent`, so that a Windows path is read the same
    // way on every system (the tests run everywhere).
    let original = own.to_string_lossy().replace('/', "\\");
    let (dir, _) = original.rsplit_once('\\')?;
    Some(
        MEMBER_NAMES
            .iter()
            .map(|m| Member {
                exe: PathBuf::from(format!("{dir}\\{m}.exe")),
                bundle: None,
            })
            .collect(),
    )
}

/// A Windows path as compared here: `\` separators, the `\\?\` prefix of a canonical path
/// dropped (`\\?\UNC\` back to `\\`), no trailing separator, in lower case (NTFS compares
/// names without regard to case).
#[must_use]
pub fn windows_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        text
    };
    text.trim_end_matches('\\').to_lowercase()
}

/// A macOS path as compared here: in lower case, since the default volume format compares
/// names without regard to case (the file identity check then tells two files apart on a
/// volume that does not).
fn macos_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// How paths are compared on the system being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRules {
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
}

impl PathRules {
    fn key(self, path: &Path) -> String {
        match self {
            Self::MacOs => macos_key(path),
            Self::Windows => windows_key(path),
        }
    }
}

/// The member the peer's executable is, from `reported` (the path the system reports for the
/// running process) and `canonical` (that path with every link resolved, `None` when it could
/// not be resolved). A path reached through a link is no member: `canonical` must be
/// `reported` itself.
#[must_use]
pub fn member_for<'m>(
    members: &'m [Member],
    reported: &Path,
    canonical: Option<&Path>,
    rules: PathRules,
) -> Option<&'m Member> {
    let reported_key = rules.key(reported);
    if rules.key(canonical?) != reported_key {
        return None;
    }
    members.iter().find(|m| rules.key(&m.exe) == reported_key)
}

/// The member whose code the code-signing API names with `code_path` (macOS: the bundle for an
/// app's main executable, the executable otherwise).
#[must_use]
pub fn member_for_code_path<'m>(members: &'m [Member], code_path: &Path) -> Option<&'m Member> {
    let key = macos_key(code_path);
    members.iter().find(|m| {
        macos_key(&m.exe) == key || m.bundle.as_deref().is_some_and(|b| macos_key(b) == key)
    })
}

/// The macOS code requirement a peer of a signed installation must satisfy: Apple's anchor,
/// this installation's team identifier and one of [`MAC_BUNDLE_IDS`]. `None` for a team
/// identifier that is not the ten letters and digits Apple issues (it could not be quoted).
#[must_use]
pub fn macos_requirement(team: &str) -> Option<String> {
    if team.len() != 10 || !team.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let ids = MAC_BUNDLE_IDS
        .iter()
        .map(|id| format!("identifier \"{id}\""))
        .collect::<Vec<_>>()
        .join(" or ");
    Some(format!(
        "anchor apple generic and certificate leaf[subject.OU] = \"{team}\" and ({ids})"
    ))
}

/// How this process is signed, from its own image (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnSigning {
    /// Signed: macOS with a team identifier, Windows with a valid Authenticode signature.
    Signed,
    /// Ad hoc or not signed.
    Unsigned,
}

/// The peer's signature, compared with this process's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSignature {
    /// Valid, and by the same signer (macOS: satisfies [`macos_requirement`]; Windows: the
    /// same leaf certificate).
    SameSigner,
    /// Anything else: unsigned, ad hoc, invalid, or another signer.
    Other,
}

/// Whether the peer is trusted under the installed-binary policy (the table in the module
/// docs). `member`: [`member_for`] found it (so not through a link). `same_file`: its
/// executable is the same file as the member's path (macOS: device and inode; Windows: true,
/// the canonical path being the file's own name there).
#[must_use]
pub const fn trusted(
    installed: bool,
    own: OwnSigning,
    signature: PeerSignature,
    member: bool,
    same_file: bool,
) -> bool {
    if !installed || !member {
        return false;
    }
    match own {
        OwnSigning::Signed => matches!(signature, PeerSignature::SameSigner),
        OwnSigning::Unsigned => same_file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "/Applications/Iamb.app";

    fn helper_exe(app: &str) -> PathBuf {
        Path::new(app)
            .join(MAC_HELPER)
            .join("Contents/MacOS/iamb-service")
    }

    #[test]
    fn the_macos_installation_is_found_from_the_ui_host_or_the_helper() {
        let from_helper = macos_members(&helper_exe(APP)).expect("the helper is installed");
        let from_ui = macos_members(Path::new(
            "/Applications/Iamb.app/Contents/MacOS/iamb-desktop",
        ))
        .expect("the UI host is installed");
        assert_eq!(from_helper, from_ui);
        assert_eq!(
            from_ui[0],
            Member {
                exe: PathBuf::from("/Applications/Iamb.app/Contents/MacOS/iamb-desktop"),
                bundle: Some(PathBuf::from(APP)),
            }
        );
        assert_eq!(from_ui[1].exe, helper_exe(APP));
        // Installed elsewhere, under another name: still an installation.
        assert!(
            macos_members(Path::new(
                "/Users/ada/Applications/Iamb 2.app/Contents/MacOS/iamb-desktop"
            ))
            .is_some()
        );
        // Not an installation: a development build, a loose binary, another name in the bundle.
        for own in [
            "/Users/ada/iamb/target/release/iamb-service",
            "/usr/local/bin/iamb-desktop",
            "/Applications/Iamb.app/Contents/MacOS/other",
            "/Applications/Iamb/Contents/MacOS/iamb-desktop",
            "/Applications/Iamb.app/Contents/Library/LoginItems/Other.app/Contents/MacOS/iamb-service",
            "/Applications/Iamb.app/Contents/MacOS/iamb-service",
        ] {
            assert_eq!(macos_members(Path::new(own)), None, "{own}");
        }
    }

    #[test]
    fn the_windows_installation_is_the_per_user_directory() {
        let local = Path::new(r"C:\Users\Ada\AppData\Local");
        let members = windows_members(
            Path::new(r"\\?\C:\Users\Ada\AppData\Local\Iamb\iamb-service.exe"),
            Some(local),
        )
        .expect("installed");
        assert_eq!(members.len(), 4);
        assert_eq!(
            windows_key(&members[0].exe),
            r"c:\users\ada\appdata\local\iamb\iamb-desktop.exe"
        );
        // Any case, either separator, with or without the canonical prefix.
        assert!(
            windows_members(
                Path::new("C:/users/ada/appdata/local/IAMB/IAMB-DESKTOP.EXE"),
                Some(local)
            )
            .is_some()
        );
        // Not the installation: another folder, a copy, another name, no %LOCALAPPDATA%.
        for own in [
            r"C:\Program Files\Iamb\iamb-service.exe",
            r"C:\Users\Ada\Downloads\iamb-service.exe",
            r"C:\Users\Ada\AppData\Local\Iamb\sub\iamb-service.exe",
            r"C:\Users\Ada\AppData\Local\Iamb\other.exe",
        ] {
            assert_eq!(windows_members(Path::new(own), Some(local)), None, "{own}");
        }
        assert_eq!(
            windows_members(
                Path::new(r"C:\Users\Ada\AppData\Local\Iamb\iamb-service.exe"),
                None
            ),
            None
        );
        assert_eq!(
            windows_key(Path::new(r"\\?\UNC\server\share\x\")),
            r"\\server\share\x"
        );
    }

    // macOS paths: a Windows host joins them with its own separator, and this check only runs
    // on macOS.
    #[cfg(not(windows))]
    #[test]
    fn a_peer_is_a_member_only_at_its_own_path_and_without_a_link() {
        let members = macos_members(&helper_exe(APP)).expect("installed");
        let ui = Path::new("/Applications/Iamb.app/Contents/MacOS/iamb-desktop");
        let rules = PathRules::MacOs;
        assert_eq!(member_for(&members, ui, Some(ui), rules), Some(&members[0]));
        // The system reports the case on disk; the member path is built from ours.
        let upper = Path::new("/Applications/IAMB.app/Contents/MacOS/iamb-desktop");
        assert_eq!(
            member_for(&members, upper, Some(upper), rules),
            Some(&members[0])
        );
        // A copy of iamb-desktop in another folder, another program in the bundle.
        let copy = Path::new("/tmp/Iamb.app/Contents/MacOS/iamb-desktop");
        assert_eq!(member_for(&members, copy, Some(copy), rules), None);
        let other = Path::new("/Applications/Iamb.app/Contents/MacOS/Other");
        assert_eq!(member_for(&members, other, Some(other), rules), None);
        // Reached through a link: its canonical path is elsewhere, or unknown.
        assert_eq!(member_for(&members, ui, Some(copy), rules), None);
        assert_eq!(member_for(&members, ui, None, rules), None);
        // The code-signing API names the bundle of the UI host and of the helper.
        assert_eq!(
            member_for_code_path(&members, Path::new(APP)),
            Some(&members[0])
        );
        assert_eq!(
            member_for_code_path(&members, &Path::new(APP).join(MAC_HELPER)),
            Some(&members[1])
        );
        assert_eq!(
            member_for_code_path(&members, Path::new("/tmp/Iamb.app")),
            None
        );

        let local = Path::new(r"C:\Users\Ada\AppData\Local");
        let win = windows_members(
            Path::new(r"C:\Users\Ada\AppData\Local\Iamb\iamb-service.exe"),
            Some(local),
        )
        .expect("installed");
        let desktop = Path::new(r"C:\Users\Ada\AppData\Local\Iamb\iamb-desktop.exe");
        let canonical = Path::new(r"\\?\C:\Users\Ada\AppData\Local\Iamb\IAMB-DESKTOP.EXE");
        assert_eq!(
            member_for(&win, desktop, Some(canonical), PathRules::Windows),
            Some(&win[0])
        );
        let elsewhere = Path::new(r"C:\Users\Ada\Downloads\iamb-desktop.exe");
        assert_eq!(
            member_for(&win, elsewhere, Some(elsewhere), PathRules::Windows),
            None
        );
        assert_eq!(
            member_for(&win, desktop, Some(elsewhere), PathRules::Windows),
            None
        );
    }

    #[test]
    fn an_unsigned_installation_trusts_its_own_members_and_a_signed_one_its_signer_only() {
        use OwnSigning::{Signed, Unsigned};
        use PeerSignature::{Other, SameSigner};
        // The owner's test builds: unsigned, the UI host of the same installation.
        assert!(trusted(true, Unsigned, Other, true, true));
        // ... but not a file that is not the member (a hard link elsewhere, another volume).
        assert!(!trusted(true, Unsigned, Other, true, false));
        // A signed installation accepts its signer only, never an unsigned member.
        assert!(trusted(true, Signed, SameSigner, true, true));
        assert!(!trusted(true, Signed, Other, true, true));
        // Not a member, or not installed: no one, whatever the signature.
        for own in [Signed, Unsigned] {
            for signature in [SameSigner, Other] {
                assert!(!trusted(true, own, signature, false, true));
                assert!(!trusted(false, own, signature, true, true));
            }
        }
    }

    #[test]
    fn the_signed_requirement_names_the_team_and_both_bundles() {
        assert_eq!(
            macos_requirement("ABCDE12345").as_deref(),
            Some(
                "anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\" and \
                 (identifier \"org.iamb.Iamb\" or identifier \"org.iamb.Iamb.Service\")"
            )
        );
        for team in ["", "ABC", "ABCDE1234\"", "ABCDE 2345", "ABCDE123456"] {
            assert_eq!(macos_requirement(team), None, "{team}");
        }
    }
}
