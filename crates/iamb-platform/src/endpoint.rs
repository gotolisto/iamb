//! The local endpoint under the IPC framing (architecture §3.3, §6.1; ADR-0006).
//!
//! A Unix domain socket on macOS, a named pipe on Windows. This module fixes what both must
//! guarantee and yields a blocking byte stream plus the verified identity of the process at
//! the other end; `iamb-ipc` puts its framing, the `Hello` message and the roles on top and
//! never sees a socket or a pipe handle.
//!
//! | | Server ([`LocalEndpoint::bind`]) | Client ([`LocalEndpoint::connect`]) |
//! | --- | --- | --- |
//! | Exclusive | Creates the endpoint as the first of its name and holds a lock beside it, or fails with `EndpointInUse` | — |
//! | User-only | Socket in a directory the user owns with mode `0700`; pipe restricted to the user's SID, remote clients rejected | — |
//! | Verified | Each accepted client is checked against the [`PeerPolicy`] before `accept` returns it | The server is checked before `connect` returns, so nothing is ever sent to an impostor |

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Result;

/// Which of Iamb's two endpoints (architecture §6.2, §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointKind {
    /// The service endpoint, served by the worker.
    Service,
    /// The account endpoint, served by the sync agent.
    Sync,
}

impl EndpointKind {
    /// The short name: `service` or `sync`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Sync => "sync",
        }
    }
}

/// Names one endpoint of one data root.
///
/// The OS crate derives the real address from it (architecture §3.3): on macOS the socket
/// [`socket_path`](Self::socket_path) inside the `run` directory; on Windows the pipe
/// `\\.\pipe\` + [`pipe_stem`](Self::pipe_stem) + `-<user SID>-<secret>`, the secret drawn
/// for every bind and kept in the `run` directory, so each data root (tests always set
/// `IAMB_DATA_DIR`) has a pipe of its own that no other user can name in advance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointName {
    kind: EndpointKind,
    run_dir: PathBuf,
}

impl EndpointName {
    /// The endpoint `kind` of the data root whose `run` directory is `run_dir`.
    #[must_use]
    pub fn new(kind: EndpointKind, run_dir: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            run_dir: run_dir.into(),
        }
    }

    /// Which endpoint.
    #[must_use]
    pub const fn kind(&self) -> EndpointKind {
        self.kind
    }

    /// The `run` directory of the data root.
    #[must_use]
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// The socket file: `<run_dir>/service.sock` or `<run_dir>/sync.sock`.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.run_dir.join(format!("{}.sock", self.kind.as_str()))
    }

    /// The first part of the pipe name: `iamb-service` or `iamb-sync`.
    #[must_use]
    pub const fn pipe_stem(&self) -> &'static str {
        match self.kind {
            EndpointKind::Service => "iamb-service",
            EndpointKind::Sync => "iamb-sync",
        }
    }
}

/// The process at the other end of a stream, as the OS reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerIdentity {
    /// Its process id.
    pub pid: u32,
    /// The user it runs as: the numeric user id on macOS, the SID string on Windows.
    pub user: String,
    /// The path of its executable, when the OS reveals it.
    pub executable: Option<PathBuf>,
    /// True when the executable is the installed Iamb binary with a valid signature. The OS
    /// crate checks this only under [`PeerPolicy::InstalledBinary`]; otherwise it is false.
    pub trusted_binary: bool,
}

/// Why a peer failed verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerRejection {
    /// It runs as another user.
    OtherUser,
    /// Its executable is not the installed, signed Iamb binary.
    UntrustedBinary,
    /// It connected from another machine (a remote pipe client).
    Remote,
    /// The OS would not say who it is.
    Unidentified,
}

impl fmt::Display for PeerRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OtherUser => "the peer runs as another user",
            Self::UntrustedBinary => "the peer is not the installed Iamb binary",
            Self::Remote => "the peer is on another machine",
            Self::Unidentified => "the peer could not be identified",
        })
    }
}

/// What a peer must be (architecture §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerPolicy {
    /// The same user. Development and test builds.
    SameUser,
    /// The same user, and the installed Iamb binary with a valid signature. Release builds.
    InstalledBinary,
}

impl PeerPolicy {
    /// The policy for the build being compiled: [`InstalledBinary`](Self::InstalledBinary) in
    /// release builds, [`SameUser`](Self::SameUser) with debug assertions on.
    #[must_use]
    pub const fn for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::SameUser
        } else {
            Self::InstalledBinary
        }
    }

    /// Checks `peer` for a process that runs as `own_user`. The OS crates and the mock all
    /// decide with this one function.
    ///
    /// # Errors
    ///
    /// Why the peer is not acceptable.
    pub fn check(
        self,
        own_user: &str,
        peer: &PeerIdentity,
    ) -> std::result::Result<(), PeerRejection> {
        if peer.user != own_user {
            return Err(PeerRejection::OtherUser);
        }
        if self == Self::InstalledBinary && !peer.trusted_binary {
            return Err(PeerRejection::UntrustedBinary);
        }
        Ok(())
    }
}

/// One verified connection: a blocking byte stream in both directions.
///
/// Reads block until at least one byte, the end of the stream (`Ok(0)`) or the read timeout
/// (an error of kind `TimedOut` or `WouldBlock`). A stream and its
/// [clones](Self::try_clone) may be used from different threads at once: one thread blocked
/// in a read must not hold up another thread's write (the server pushes events while its
/// reader waits for the next request).
pub trait LocalStream: Read + Write + Send {
    /// The process at the other end. It passed the [`PeerPolicy`] before this stream was
    /// handed out.
    fn peer(&self) -> &PeerIdentity;

    /// Another handle to the same connection.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the handle cannot be duplicated.
    fn try_clone(&self) -> Result<Box<dyn LocalStream>>;

    /// Sets how long a read waits; `None` waits for ever. Applies to every clone.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()>;

    /// Closes the connection in both directions: blocked reads on either side return, and the
    /// peer reads the end of the stream.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn shutdown(&self) -> Result<()>;
}

/// A bound endpoint that accepts connections. Dropping it releases the name.
pub trait LocalListener: Send + Sync {
    /// Blocks until a client that passes the policy connects.
    ///
    /// # Errors
    ///
    /// [`PeerRejected`](crate::PlatformError::PeerRejected) when a client failed verification
    /// (its connection is already closed; the server counts it and accepts again);
    /// [`Closed`](crate::PlatformError::Closed) after [`close`](Self::close);
    /// [`Os`](crate::PlatformError::Os) otherwise.
    fn accept(&self) -> Result<Box<dyn LocalStream>>;

    /// Stops accepting: an `accept` blocked on another thread returns
    /// [`Closed`](crate::PlatformError::Closed), and the name is released.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn close(&self) -> Result<()>;
}

/// Creates and connects to local endpoints.
pub trait LocalEndpoint: Send + Sync {
    /// Creates the endpoint, exclusively and for the current user only.
    ///
    /// # Errors
    ///
    /// [`EndpointInUse`](crate::PlatformError::EndpointInUse) when the name is taken (the
    /// service's worker then runs without its endpoint and tries again, §6.1);
    /// [`Os`](crate::PlatformError::Os) otherwise.
    fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>>;

    /// Connects to the endpoint and verifies the server before returning.
    ///
    /// # Errors
    ///
    /// [`EndpointNotFound`](crate::PlatformError::EndpointNotFound) when nothing is listening
    /// (the command-line tool then opens the store itself, §6.3);
    /// [`PeerRejected`](crate::PlatformError::PeerRejected) when the server failed
    /// verification; [`Timeout`](crate::PlatformError::Timeout) when the server did not take
    /// the connection in `timeout`; [`Os`](crate::PlatformError::Os) otherwise.
    fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        timeout: Duration,
    ) -> Result<Box<dyn LocalStream>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(user: &str, trusted: bool) -> PeerIdentity {
        PeerIdentity {
            pid: 10,
            user: user.to_owned(),
            executable: Some(PathBuf::from("/Applications/Iamb.app/iamb-service")),
            trusted_binary: trusted,
        }
    }

    #[test]
    fn names_follow_the_architecture() {
        let service = EndpointName::new(EndpointKind::Service, "/data/run");
        assert_eq!(service.kind(), EndpointKind::Service);
        assert_eq!(service.run_dir(), Path::new("/data/run"));
        assert_eq!(service.socket_path(), Path::new("/data/run/service.sock"));
        assert_eq!(service.pipe_stem(), "iamb-service");

        let sync = EndpointName::new(EndpointKind::Sync, PathBuf::from("/data/run"));
        assert_eq!(sync.socket_path(), Path::new("/data/run/sync.sock"));
        assert_eq!(sync.pipe_stem(), "iamb-sync");
        assert_ne!(service, sync);
        assert_ne!(
            service,
            EndpointName::new(EndpointKind::Service, "/other/run")
        );
    }

    #[test]
    fn same_user_policy_checks_the_user_only() {
        assert_eq!(
            PeerPolicy::SameUser.check("501", &peer("501", false)),
            Ok(())
        );
        assert_eq!(
            PeerPolicy::SameUser.check("501", &peer("502", true)),
            Err(PeerRejection::OtherUser)
        );
    }

    #[test]
    fn installed_binary_policy_also_checks_the_signature() {
        let policy = PeerPolicy::InstalledBinary;
        assert_eq!(
            policy.check("S-1-5-21-1", &peer("S-1-5-21-1", true)),
            Ok(())
        );
        assert_eq!(
            policy.check("S-1-5-21-1", &peer("S-1-5-21-1", false)),
            Err(PeerRejection::UntrustedBinary)
        );
        assert_eq!(
            policy.check("S-1-5-21-1", &peer("S-1-5-21-2", true)),
            Err(PeerRejection::OtherUser)
        );
    }

    #[test]
    fn the_build_policy_matches_the_profile() {
        let expected = if cfg!(debug_assertions) {
            PeerPolicy::SameUser
        } else {
            PeerPolicy::InstalledBinary
        };
        assert_eq!(PeerPolicy::for_build(), expected);
    }

    #[test]
    fn rejections_have_messages() {
        for why in [
            PeerRejection::OtherUser,
            PeerRejection::UntrustedBinary,
            PeerRejection::Remote,
            PeerRejection::Unidentified,
        ] {
            assert!(why.to_string().starts_with("the peer"));
        }
    }
}
