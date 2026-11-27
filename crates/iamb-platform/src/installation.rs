//! Who belongs to the same installation as this process, and when a peer of the local endpoint
//! is trusted under [`PeerPolicy::InstalledBinary`](crate::PeerPolicy) (architecture §6.1, the
//! 2.8 privacy review's finding 11, and the strict review of its fix,
//! `docs/reviews/finding-11-security-review.md`).
//!
//! The OS crates gather the facts (this process's own signing state, the peer's executable as
//! the system reports it, with its links resolved, its file identity and its signature) and
//! decide with the pure functions here, so that the decision is tested on every system.
//!
//! # The installation
//!
//! An installation is found from this process's own canonical executable:
//!
//! - **macOS**: `<X>.app/Contents/MacOS/iamb-desktop` (the UI host) or
//!   `<X>.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service` (the
//!   helper). Those two are its members.
//! - **Windows**: an executable whose canonical directory is the canonical per-user
//!   installation directory, `<LocalAppData>\Iamb`, the known folder taken from the system,
//!   not from the environment. Its members are `iamb-desktop.exe` and `iamb-service.exe` there.
//!
//! Only what the installer puts there is a member: the command-line tool and the sync agent do
//! not ship yet, and a path where nothing ships would be free for any program to take. Each is
//! added here, with its macOS bundle identifier in [`MAC_BUNDLE_IDS`], when it ships.
//!
//! Anywhere else (a development build, a copy elsewhere) this process belongs to no
//! installation and trusts no peer under that policy.
//!
//! # This process's own signing state
//!
//! Read once, at the start of the process, from the running code, never from the file on disk
//! later ([`OwnState`]); a build that the release pipeline signs has its signer compiled in
//! instead ([`pinned`]: the team identifier on macOS, the certificate thumbprint on Windows), and
//! then never asks. Three states, so that an error is never taken for "unsigned":
//!
//! - **signed**, by a signer: macOS, the running code is valid, anchored at Apple and has a team
//!   identifier; Windows, the image verifies and its leaf certificate has a thumbprint;
//! - **unsigned**: macOS, no signature (`errSecCSUnsigned`) or an ad hoc one (valid, no team
//!   identifier); Windows, no signature (`TRUST_E_NOSIGNATURE`);
//! - **could not tell**: anything else (an API error, a signature that does not verify, a
//!   malformed pin). It trusts no peer, and is logged.
//!
//! # The decision
//!
//! | This process | The peer | Trusted |
//! | --- | --- | --- |
//! | Not in an installation, or could not tell how it is signed | anything | no |
//! | Signed | a member's path, signed by the same signer (macOS: the code its audit token names satisfies `anchor apple generic`, the team identifier and Iamb's bundle identifiers; Windows: the file's leaf certificate has the same thumbprint) | yes |
//! | Signed | a member's path, but unsigned, ad hoc, invalid or another signer | **no** |
//! | Unsigned | a member's path, and its file is the member's (macOS: device and inode; Windows: volume serial number and file index) | yes |
//! | either | not a member's path: another folder, a copy of `iamb-desktop` elsewhere, a member that does not ship | no |
//!
//! "A member's path" is the peer's executable with every link resolved, compared with the
//! member's canonical path by components and with ASCII letters folded, as the file systems
//! compare names (Unicode folding would match names the file system tells apart). What ties a
//! process to that path is what the system reports for it: on macOS the executable's vnode,
//! which holds no link, so a process started through a link to a member is reported at the
//! member's path; on Windows the image name. The file identity compares two look-ups of files
//! by path, not the file the process was mapped from, which neither system offers to ask for.
//! An unsigned installation therefore trusts what is at its members' paths; whoever can write
//! there can replace the whole installation anyway.

use std::path::{Component, Path, PathBuf};

/// The macOS bundle identifiers of an installation's code, for the signed requirement: the
/// main app (the UI host) and the helper (the service).
pub const MAC_BUNDLE_IDS: [&str; 2] = ["org.iamb.Iamb", "org.iamb.Iamb.Service"];

/// The helper app's path inside the main app bundle (ADR-0021).
pub const MAC_HELPER: &str = "Contents/Library/LoginItems/Iamb Service.app";

/// The executables an installation holds, by name without `.exe`: what the installer puts there.
pub const MEMBER_NAMES: [&str; 2] = ["iamb-desktop", "iamb-service"];

/// The per-user installation directory under the `LocalAppData` known folder on Windows (the
/// installer's, `apps/desktop/release/tauri.windows.json`).
pub const WINDOWS_INSTALL_DIR: &str = "Iamb";

/// `errSecCSUnsigned`: the code has no signature at all.
pub const MAC_ERR_UNSIGNED: i32 = -67062;

/// `TRUST_E_NOSIGNATURE`: the file has no Authenticode signature.
#[allow(clippy::cast_possible_wrap)]
pub const WIN_TRUST_E_NOSIGNATURE: i32 = 0x800B_0100_u32 as i32;

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
    let helper = root.join(MAC_HELPER);
    Some(vec![
        Member {
            exe: root.join("Contents/MacOS/iamb-desktop"),
            bundle: Some(root.clone()),
        },
        Member {
            exe: helper.join("Contents/MacOS/iamb-service"),
            bundle: Some(helper),
        },
    ])
}

/// The main app bundle around `own`, when `own` is the UI host or the helper of one.
fn macos_root(own: &Path) -> Option<PathBuf> {
    let name = own.file_name()?.to_str()?;
    let dir = own.parent()?;
    let root = match name {
        "iamb-desktop" => strip_suffix_path(dir, "Contents/MacOS")?,
        "iamb-service" => strip_suffix_path(strip_suffix_path(dir, "Contents/MacOS")?, MAC_HELPER)?,
        _ => return None,
    };
    root.extension()
        .is_some_and(|e| e == "app")
        .then(|| root.to_path_buf())
}

/// `path` without a trailing run of components.
fn strip_suffix_path<'p>(path: &'p Path, suffix: &str) -> Option<&'p Path> {
    let mut path = path;
    for part in Path::new(suffix).components().rev() {
        if path.file_name()? != part.as_os_str() {
            return None;
        }
        path = path.parent()?;
    }
    Some(path)
}

/// The members of the Windows installation that `own` (this process's canonical executable)
/// belongs to, given `install_dir`, the canonical per-user installation directory (the Local
/// `AppData` known folder and [`WINDOWS_INSTALL_DIR`], with every link resolved); `None` when
/// `own` is not one of them in that directory.
#[must_use]
pub fn windows_members(own: &Path, install_dir: Option<&Path>) -> Option<Vec<Member>> {
    let own_text = windows_key(own);
    let (dir, name) = own_text.rsplit_once('\\')?;
    if !MEMBER_NAMES
        .iter()
        .any(|m| name == format!("{m}.exe").as_str())
    {
        return None;
    }
    if dir != windows_key(install_dir?) {
        return None;
    }
    // Built from the text, not with `Path::parent`, so that a Windows path is read the same
    // way on every system (the tests run everywhere).
    let original = own.to_string_lossy().replace('/', "\\");
    let (dir, _) = original.trim_end_matches('\\').rsplit_once('\\')?;
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
/// dropped (`\\?\UNC\` back to `\\`), no empty components and no trailing separator, ASCII
/// letters folded (NTFS compares names ordinally, without regard to case; Unicode lower case
/// would also match names it tells apart, such as `k` and the Kelvin sign).
#[must_use]
pub fn windows_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let (prefix, rest) = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        (r"\\", rest)
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        ("", rest)
    } else if let Some(rest) = text.strip_prefix(r"\\") {
        (r"\\", rest)
    } else {
        ("", text.as_str())
    };
    let parts: Vec<&str> = rest.split('\\').filter(|p| !p.is_empty()).collect();
    format!("{prefix}{}", parts.join("\\")).to_ascii_lowercase()
}

/// A macOS path as compared here: by components (no trailing or repeated separator, no `.`),
/// ASCII letters folded, since the default volume format compares names without regard to case
/// (the file identity check then tells two files apart on a volume that does not).
fn macos_key(path: &Path) -> String {
    let mut out = String::new();
    for part in path.components() {
        match part {
            Component::RootDir => out.push('/'),
            Component::CurDir => {}
            other => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&other.as_os_str().to_string_lossy());
            }
        }
    }
    out.to_ascii_lowercase()
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

/// The member whose canonical path `canonical` is (the peer's executable with every link
/// resolved; `None` when it could not be resolved, which is no member).
#[must_use]
pub fn member_for<'m>(
    members: &'m [Member],
    canonical: Option<&Path>,
    rules: PathRules,
) -> Option<&'m Member> {
    let key = rules.key(canonical?);
    members.iter().find(|m| rules.key(&m.exe) == key)
}

/// The member whose code the code-signing API names with `code_path` (macOS: the bundle for an
/// app's main executable, the executable otherwise), compared by components.
#[must_use]
pub fn member_for_code_path<'m>(members: &'m [Member], code_path: &Path) -> Option<&'m Member> {
    let key = macos_key(code_path);
    members.iter().find(|m| {
        macos_key(&m.exe) == key || m.bundle.as_deref().is_some_and(|b| macos_key(b) == key)
    })
}

/// Whether `team` is a team identifier as Apple issues them: ten ASCII letters and digits.
#[must_use]
pub fn valid_team(team: &str) -> bool {
    team.len() == 10 && team.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The macOS code requirement a peer of a signed installation must satisfy: Apple's anchor,
/// this installation's team identifier and one of [`MAC_BUNDLE_IDS`]. `None` for a team
/// identifier that is not valid ([`valid_team`]; it could not be quoted).
#[must_use]
pub fn macos_requirement(team: &str) -> Option<String> {
    if !valid_team(team) {
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

/// The requirement this process's own running code must satisfy to count as signed by `team`.
#[must_use]
pub fn macos_own_requirement(team: &str) -> Option<String> {
    valid_team(team)
        .then(|| format!("anchor apple generic and certificate leaf[subject.OU] = \"{team}\""))
}

/// A Windows certificate thumbprint as compared here: the 40 hexadecimal digits of the SHA-1
/// hash of the certificate, in upper case, spaces dropped. `None` for anything else.
#[must_use]
pub fn windows_thumbprint(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    (digits.len() == 40 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| digits.to_ascii_uppercase())
}

/// How this process is signed (module docs, "This process's own signing state").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnSigning {
    /// Signed: macOS with a team identifier, Windows with a valid Authenticode signature.
    Signed,
    /// Ad hoc or not signed.
    Unsigned,
    /// It could not be told. No peer is trusted.
    Unknown,
}

/// This process's own signing state, with its signer (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnState {
    /// Signed by this signer: the team identifier (macOS) or the certificate thumbprint
    /// (Windows, [`windows_thumbprint`]).
    Signed(String),
    /// Ad hoc or not signed.
    Unsigned,
    /// It could not be told, and why.
    Unknown(&'static str),
}

impl OwnState {
    /// The state without the signer.
    #[must_use]
    pub const fn signing(&self) -> OwnSigning {
        match self {
            Self::Signed(_) => OwnSigning::Signed,
            Self::Unsigned => OwnSigning::Unsigned,
            Self::Unknown(_) => OwnSigning::Unknown,
        }
    }

    /// The signer, when signed.
    #[must_use]
    pub fn signer(&self) -> Option<&str> {
        match self {
            Self::Signed(signer) => Some(signer),
            Self::Unsigned | Self::Unknown(_) => None,
        }
    }

    /// One line for the log.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Signed(signer) => format!("signed by {signer}: peers must be signed by it too"),
            Self::Unsigned => "unsigned: peers are trusted by their installation path".to_owned(),
            Self::Unknown(why) => {
                format!("signing state unknown ({why}): no peer is trusted")
            }
        }
    }
}

/// This process's signing state given `pin`, the signer compiled in by the release pipeline
/// (`IAMB_SIGNER_TEAM` on macOS, `IAMB_SIGNER_THUMBPRINT` on Windows; empty or absent for an
/// unsigned build), normalized by `normalize` ([`valid_team`] as a filter, or
/// [`windows_thumbprint`]); otherwise what `read` finds in the running code. A malformed pin
/// trusts no one.
pub fn pinned(
    pin: Option<&str>,
    normalize: impl Fn(&str) -> Option<String>,
    read: impl FnOnce() -> OwnState,
) -> OwnState {
    match pin.map(str::trim).filter(|p| !p.is_empty()) {
        Some(pin) => normalize(pin).map_or(
            OwnState::Unknown("the compiled-in signer is malformed"),
            OwnState::Signed,
        ),
        None => read(),
    }
}

/// The macOS state from what the running code says: `validity`, the status of checking its own
/// signature with no requirement; `team`, its team identifier; `anchored`, whether it satisfies
/// [`macos_own_requirement`] for that team.
#[must_use]
pub fn classify_macos(validity: i32, team: Option<String>, anchored: bool) -> OwnState {
    match (validity, team) {
        (MAC_ERR_UNSIGNED, _) | (0, None) => OwnState::Unsigned,
        (0, Some(team)) if valid_team(&team) && anchored => OwnState::Signed(team),
        (0, Some(_)) => OwnState::Unknown("signed, but not by a valid Developer ID team"),
        _ => OwnState::Unknown("the running code's signature could not be checked"),
    }
}

/// The Windows state from verifying this process's own image: `status`, what `WinVerifyTrust`
/// answered; `thumbprint`, its signer's certificate thumbprint when it verified.
#[must_use]
pub fn classify_windows(status: i32, thumbprint: Option<&str>) -> OwnState {
    match (status, thumbprint.and_then(windows_thumbprint)) {
        (0, Some(thumbprint)) => OwnState::Signed(thumbprint),
        (0, None) => OwnState::Unknown("the image verifies but its signer could not be read"),
        (WIN_TRUST_E_NOSIGNATURE, _) => OwnState::Unsigned,
        _ => OwnState::Unknown("the image's signature does not verify"),
    }
}

/// The peer's signature, compared with this process's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSignature {
    /// Valid, and by the same signer (macOS: satisfies [`macos_requirement`]; Windows: the
    /// same certificate thumbprint).
    SameSigner,
    /// Anything else: unsigned, ad hoc, invalid, or another signer.
    Other,
}

/// Whether the peer is trusted under the installed-binary policy (the table in the module
/// docs). `member`: [`member_for`] found it (and, on macOS, the code its audit token names is
/// that member). `same_file`: the file at its canonical path is the member's file.
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
        OwnSigning::Unknown => false,
    }
}

/// Whether a process created at `created` can be the one that connected at `connected`, both
/// in the same clock (Windows: `FILETIME`, 100 ns since 1601): a process created after the
/// connection is one that took the process id of the client that connected, after that client
/// ended (the strict review's finding 4).
#[must_use]
pub const fn created_before(created: u64, connected: u64) -> bool {
    created <= connected
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
        assert!(
            macos_members(Path::new(
                "/Users/ada/Applications/Iamb 2.app/Contents/MacOS/iamb-desktop"
            ))
            .is_some()
        );
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
    fn the_windows_installation_is_the_canonical_per_user_directory() {
        let dir = Path::new(r"\\?\C:\Users\Ada\AppData\Local\Iamb");
        let members = windows_members(
            Path::new(r"\\?\C:\Users\Ada\AppData\Local\Iamb\iamb-service.exe"),
            Some(dir),
        )
        .expect("installed");
        assert_eq!(members.len(), 2);
        assert_eq!(
            windows_key(&members[0].exe),
            r"c:\users\ada\appdata\local\iamb\iamb-desktop.exe"
        );
        // Canonical against canonical: any ASCII case, either separator, a trailing one.
        assert!(
            windows_members(
                Path::new("C:/users/ada/appdata/local/IAMB/IAMB-DESKTOP.EXE"),
                Some(Path::new(r"C:\Users\Ada\AppData\Local\Iamb\"))
            )
            .is_some()
        );
        for own in [
            r"C:\Program Files\Iamb\iamb-service.exe",
            r"C:\Users\Ada\Downloads\iamb-service.exe",
            r"C:\Users\Ada\AppData\Local\Iamb\sub\iamb-service.exe",
            r"C:\Users\Ada\AppData\Local\Iamb\other.exe",
            r"C:\Users\Ada\AppData\Local\Iamb\iamb-sync.exe",
        ] {
            assert_eq!(windows_members(Path::new(own), Some(dir)), None, "{own}");
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

    #[test]
    fn a_peer_is_a_member_only_at_a_members_canonical_path() {
        let members = macos_members(&helper_exe(APP)).expect("installed");
        let ui = Path::new("/Applications/Iamb.app/Contents/MacOS/iamb-desktop");
        let rules = PathRules::MacOs;
        assert_eq!(member_for(&members, Some(ui), rules), Some(&members[0]));
        let upper = Path::new("/Applications/IAMB.app/Contents/MacOS/iamb-desktop");
        assert_eq!(member_for(&members, Some(upper), rules), Some(&members[0]));
        let copy = Path::new("/tmp/Iamb.app/Contents/MacOS/iamb-desktop");
        assert_eq!(member_for(&members, Some(copy), rules), None);
        let other = Path::new("/Applications/Iamb.app/Contents/MacOS/Other");
        assert_eq!(member_for(&members, Some(other), rules), None);
        assert_eq!(member_for(&members, None, rules), None);
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

        let dir = Path::new(r"C:\Users\Ada\AppData\Local\Iamb");
        let win = windows_members(&Path::new(dir).join("iamb-service.exe"), Some(dir))
            .expect("installed");
        let canonical = Path::new(r"\\?\C:\Users\Ada\AppData\Local\Iamb\IAMB-DESKTOP.EXE");
        assert_eq!(
            member_for(&win, Some(canonical), PathRules::Windows),
            Some(&win[0])
        );
        let elsewhere = Path::new(r"C:\Users\Ada\Downloads\iamb-desktop.exe");
        assert_eq!(member_for(&win, Some(elsewhere), PathRules::Windows), None);
    }

    #[test]
    fn an_unsigned_installation_trusts_its_own_members_and_a_signed_one_its_signer_only() {
        use OwnSigning::{Signed, Unknown, Unsigned};
        use PeerSignature::{Other, SameSigner};
        assert!(trusted(true, Unsigned, Other, true, true));
        assert!(!trusted(true, Unsigned, Other, true, false));
        assert!(trusted(true, Signed, SameSigner, true, true));
        assert!(!trusted(true, Signed, Other, true, true));
        for own in [Signed, Unsigned, Unknown] {
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
        assert_eq!(
            macos_own_requirement("ABCDE12345").as_deref(),
            Some("anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\"")
        );
        for team in ["", "ABC", "ABCDE1234\"", "ABCDE 2345", "ABCDE123456"] {
            assert_eq!(macos_requirement(team), None, "{team}");
            assert_eq!(macos_own_requirement(team), None, "{team}");
        }
    }

    /// The strict review's finding 1: an error reading this process's own signature is never
    /// "unsigned", and a pinned signer is never asked about.
    #[test]
    fn an_error_reading_the_own_signature_trusts_no_peer() {
        // macOS.
        assert_eq!(
            classify_macos(MAC_ERR_UNSIGNED, None, false),
            OwnState::Unsigned
        );
        assert_eq!(classify_macos(0, None, false), OwnState::Unsigned, "ad hoc");
        assert_eq!(
            classify_macos(0, Some("ABCDE12345".into()), true),
            OwnState::Signed("ABCDE12345".into())
        );
        for state in [
            classify_macos(-67061, None, false),
            classify_macos(-67068, Some("ABCDE12345".into()), true),
            classify_macos(0, Some("ABCDE12345".into()), false),
            classify_macos(0, Some("bad team".into()), true),
        ] {
            assert_eq!(state.signing(), OwnSigning::Unknown, "{state:?}");
        }
        // Windows.
        let thumb = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
        assert_eq!(
            classify_windows(0, Some(thumb)),
            OwnState::Signed(thumb.to_ascii_uppercase())
        );
        assert_eq!(
            classify_windows(WIN_TRUST_E_NOSIGNATURE, None),
            OwnState::Unsigned
        );
        #[allow(clippy::cast_possible_wrap)]
        let untrusted = 0x800B_0111_u32 as i32; // TRUST_E_EXPLICIT_DISTRUST
        #[allow(clippy::cast_possible_wrap)]
        let expired = 0x800B_0101_u32 as i32; // CERT_E_EXPIRED
        for state in [
            classify_windows(untrusted, None),
            classify_windows(expired, Some(thumb)),
            classify_windows(0, None),
            classify_windows(-1, None),
        ] {
            assert_eq!(state.signing(), OwnSigning::Unknown, "{state:?}");
        }
        // Could not tell: no peer at all, whatever it is.
        for signature in [PeerSignature::SameSigner, PeerSignature::Other] {
            for same_file in [false, true] {
                assert!(!trusted(
                    true,
                    OwnSigning::Unknown,
                    signature,
                    true,
                    same_file
                ));
            }
        }
        // A pin wins over what the running code says, and is never read from it.
        let team = |t: &str| valid_team(t).then(|| t.to_owned());
        assert_eq!(
            pinned(Some("ABCDE12345"), team, || panic!("a pinned build asked")),
            OwnState::Signed("ABCDE12345".into())
        );
        assert_eq!(
            pinned(Some("nonsense"), team, || OwnState::Unsigned).signing(),
            OwnSigning::Unknown
        );
        assert_eq!(
            pinned(Some(" "), team, || OwnState::Unsigned),
            OwnState::Unsigned
        );
        assert_eq!(
            pinned(None, team, || OwnState::Unsigned),
            OwnState::Unsigned
        );
        assert_eq!(
            pinned(
                Some(&thumb.replace('a', "A")),
                windows_thumbprint,
                || panic!("asked")
            ),
            OwnState::Signed(thumb.to_ascii_uppercase())
        );
        assert!(
            OwnState::Unknown("x")
                .describe()
                .contains("no peer is trusted")
        );
    }

    #[test]
    fn a_process_created_after_the_connection_is_not_the_client() {
        assert!(created_before(100, 100));
        assert!(created_before(99, 100));
        assert!(!created_before(101, 100));
    }

    /// The strict review's findings 5, 7 and 9, with functions whose signatures did not change.
    mod review {
        use super::super::*;

        #[test]
        fn programs_that_do_not_ship_are_no_members() {
            let members = macos_members(Path::new(
                "/Applications/Iamb.app/Contents/MacOS/iamb-desktop",
            ))
            .expect("installed");
            for name in ["iamb", "iamb-sync"] {
                let path = Path::new("/Applications/Iamb.app/Contents/MacOS").join(name);
                assert_eq!(member_for_code_path(&members, &path), None, "{name}");
            }
            assert_eq!(members.len(), 2);
        }

        #[test]
        fn only_ascii_letters_are_folded() {
            // U+212A KELVIN SIGN lower-cases to `k` in Unicode; NTFS and APFS tell them apart.
            assert_ne!(
                windows_key(Path::new("C:\\Users\\Mi\u{212A}e\\AppData\\Local\\Iamb")),
                windows_key(Path::new(r"C:\Users\Mike\AppData\Local\Iamb"))
            );
            assert_eq!(
                windows_key(Path::new(r"C:\USERS\Ada")),
                windows_key(Path::new(r"c:\users\ada"))
            );
            let members = macos_members(Path::new(
                "/Users/mike/Applications/Iamb.app/Contents/MacOS/iamb-desktop",
            ))
            .expect("installed");
            assert_eq!(
                member_for_code_path(&members, Path::new("/Users/MIKE/Applications/Iamb.app")),
                Some(&members[0])
            );
            assert_eq!(
                member_for_code_path(
                    &members,
                    Path::new("/Users/mi\u{212A}e/Applications/Iamb.app")
                ),
                None
            );
        }

        #[test]
        fn trailing_and_repeated_separators_do_not_matter() {
            let members = macos_members(Path::new(
                "/Applications/Iamb.app/Contents/MacOS/iamb-desktop",
            ))
            .expect("installed");
            for path in [
                "/Applications/Iamb.app/",
                "/Applications//Iamb.app",
                "/Applications/./Iamb.app",
            ] {
                assert_eq!(
                    member_for_code_path(&members, Path::new(path)),
                    Some(&members[0]),
                    "{path}"
                );
            }
            assert_eq!(
                windows_key(Path::new(r"C:\Users\\Ada\Iamb\\")),
                windows_key(Path::new(r"C:\Users\Ada\Iamb"))
            );
        }
    }
}
