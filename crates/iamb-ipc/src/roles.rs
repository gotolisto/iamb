//! Which role a client may take, from the executable the operating system names for it (the
//! 2.8 review's finding 10).
//!
//! The `hello` states a role; on its own that is whatever the client says. One verified peer
//! could then open connections as `sync`, `cli` and `ui` alike, and ADR-0006's promise that a
//! sync agent "cannot read diagnostics or pause the engine" would hold only for a bug, not for a
//! sync agent taken over from the network. So the server also looks at the peer's executable,
//! which the endpoint reads from the operating system before the first byte
//! ([`PeerIdentity::executable`]), never from the client:
//!
//! | Executable (file name, any case, `.exe` dropped) | May say |
//! | --- | --- |
//! | `iamb-desktop`, the UI host | `ui` |
//! | `iamb`, the command-line tool | `cli` |
//! | `iamb-sync`, the sync agent | `sync` |
//! | any other name | nothing under [`PeerPolicy::InstalledBinary`]; anything under [`PeerPolicy::SameUser`] |
//! | not known (the Linux endpoint names no executable) | anything |
//!
//! - **An Iamb binary is held to its role in every build.** Debug builds and tests use
//!   [`PeerPolicy::SameUser`], but the names above still bind: a debug `iamb-sync` cannot say
//!   `ui` either. Test programs, the soak and compatibility harnesses and the mock's peers
//!   have other names and keep every role there.
//! - **Release builds accept only the three names.** Under
//!   [`PeerPolicy::InstalledBinary`] the endpoint has already checked that the peer is one of
//!   the executables of this process's own installation, at its own path, and, when the
//!   installation is signed, signed by the same signer (`iamb_platform::installation`, the 2.8
//!   review's finding 11), so a renamed or unknown binary there is refused rather than trusted
//!   with every role.
//!
//! What this does not stop: in an unsigned installation the check is the path, so a process
//! of the user that replaces `iamb-desktop` inside the installation can choose `ui`; it could
//! replace the whole installation as well. A signed installation requires the same signer. On
//! Linux (development only) the endpoint names no executable, so the role is not checked
//! there.

use std::fmt;

use iamb_platform::{PeerIdentity, PeerPolicy};

use crate::protocol::{ErrorBody, ErrorCode, Role};

/// The executable name of each client, as the table in the [module docs](self) has it.
pub const CLIENT_BINARIES: [(&str, Role); 3] = [
    ("iamb-desktop", Role::Ui),
    ("iamb", Role::Cli),
    ("iamb-sync", Role::Sync),
];

/// Why a role was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleRefusal {
    /// The executable is the Iamb client that holds another role.
    WrongBinary {
        /// The role asked for.
        asked: Role,
        /// The role of that executable.
        allowed: Role,
    },
    /// Release builds: the executable is not one of Iamb's clients.
    UnknownBinary {
        /// The role asked for.
        asked: Role,
    },
}

impl fmt::Display for RoleRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongBinary { asked, allowed } => write!(
                f,
                "this program may connect as {allowed} only, not as {asked}"
            ),
            Self::UnknownBinary { asked } => write!(
                f,
                "this program is not an Iamb client and may not connect as {asked}"
            ),
        }
    }
}

impl From<RoleRefusal> for ErrorBody {
    fn from(why: RoleRefusal) -> Self {
        Self::new(ErrorCode::NotPermitted, why.to_string())
    }
}

/// The file name of `path` without its directories, whatever the separator (a Windows path
/// is read on every system in tests), and without a trailing `.exe` in any case.
fn stem(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.len().checked_sub(4) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".exe") => {
            &name[..cut]
        }
        _ => name,
    }
}

/// The role the Iamb client at `executable` holds, if it is one of them.
#[must_use]
pub fn role_of_binary(executable: &str) -> Option<Role> {
    let stem = stem(executable);
    CLIENT_BINARIES
        .iter()
        .find(|(name, _)| stem.eq_ignore_ascii_case(name))
        .map(|(_, role)| *role)
}

/// Whether `peer`, verified under `policy`, may take `role` (see the [module docs](self)).
///
/// # Errors
///
/// Why not.
pub fn check_role(role: Role, peer: &PeerIdentity, policy: PeerPolicy) -> Result<(), RoleRefusal> {
    let Some(executable) = &peer.executable else {
        return Ok(());
    };
    match role_of_binary(&executable.to_string_lossy()) {
        Some(allowed) if allowed == role => Ok(()),
        Some(allowed) => Err(RoleRefusal::WrongBinary {
            asked: role,
            allowed,
        }),
        None if policy == PeerPolicy::InstalledBinary => {
            Err(RoleRefusal::UnknownBinary { asked: role })
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn peer(executable: Option<&str>) -> PeerIdentity {
        PeerIdentity {
            pid: 7,
            user: "501".to_owned(),
            executable: executable.map(PathBuf::from),
            trusted_binary: true,
        }
    }

    #[test]
    fn the_three_clients_are_known_by_name_in_any_case_and_with_exe() {
        for (path, role) in [
            (
                "/Applications/Iamb.app/Contents/MacOS/iamb-desktop",
                Role::Ui,
            ),
            (
                r"C:\Users\Ada\AppData\Local\Programs\Iamb\IAMB-DESKTOP.EXE",
                Role::Ui,
            ),
            ("/usr/local/bin/iamb", Role::Cli),
            (r"C:\Program Files\Iamb\iamb.exe", Role::Cli),
            (
                "/Applications/Iamb.app/Contents/Helpers/iamb-sync",
                Role::Sync,
            ),
            ("iamb-sync.exe", Role::Sync),
        ] {
            assert_eq!(role_of_binary(path), Some(role), "{path}");
        }
        for path in [
            "/mock/iamb-service",
            "/target/debug/deps/end_to_end-0123abcd",
            r"C:\Temp\iamb-desktop-copy.exe",
            "iamb.exe.bak",
            "",
            ".exe",
        ] {
            assert_eq!(role_of_binary(path), None, "{path}");
        }
    }

    #[test]
    fn an_iamb_client_is_held_to_its_role_under_either_policy() {
        let sync = peer(Some("/opt/iamb/iamb-sync"));
        for policy in [PeerPolicy::SameUser, PeerPolicy::InstalledBinary] {
            assert_eq!(check_role(Role::Sync, &sync, policy), Ok(()));
            for asked in [Role::Ui, Role::Cli] {
                assert_eq!(
                    check_role(asked, &sync, policy),
                    Err(RoleRefusal::WrongBinary {
                        asked,
                        allowed: Role::Sync
                    })
                );
            }
            let desktop = peer(Some(r"C:\Iamb\iamb-desktop.exe"));
            assert_eq!(check_role(Role::Ui, &desktop, policy), Ok(()));
            assert!(check_role(Role::Cli, &desktop, policy).is_err());
            assert!(check_role(Role::Sync, &desktop, policy).is_err());
        }
    }

    #[test]
    fn other_programs_keep_every_role_in_development_and_none_in_release() {
        let test = peer(Some("/target/debug/deps/contract-5a5a"));
        for role in Role::ALL {
            assert_eq!(check_role(role, &test, PeerPolicy::SameUser), Ok(()));
            assert_eq!(
                check_role(role, &test, PeerPolicy::InstalledBinary),
                Err(RoleRefusal::UnknownBinary { asked: role })
            );
            // An endpoint that names no executable (Linux) cannot be checked.
            assert_eq!(
                check_role(role, &peer(None), PeerPolicy::InstalledBinary),
                Ok(())
            );
        }
    }

    #[test]
    fn a_refusal_is_answered_as_not_permitted() {
        let body = ErrorBody::from(RoleRefusal::WrongBinary {
            asked: Role::Ui,
            allowed: Role::Sync,
        });
        assert_eq!(body.code, ErrorCode::NotPermitted);
        assert_eq!(
            body.message,
            "this program may connect as sync only, not as ui"
        );
    }
}
