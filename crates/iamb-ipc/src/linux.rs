//! The local endpoint on Linux: a Unix domain socket, std only.
//!
//! # Why it lives here
//!
//! Linux has no OS crate: it is a development platform (the service runs there against the
//! mock platform, and the command-line tool is used there). Peer credentials (`SO_PEERCRED`)
//! need either `unsafe` or a nightly-only std API, and `unsafe` is forbidden outside the two
//! OS crates. So this implementation verifies the peer through the file system instead, and is
//! documented as weaker than the macOS and Windows ones:
//!
//! - **Server side.** The socket lives in the data root's `run` directory, which must be owned
//!   by the current user with mode `0700` (created so, tightened if it is looser, refused if
//!   another user owns it), and the socket file is `0600`. Another user can therefore not
//!   reach the socket at all. The peer's identity is reported as the current user with
//!   `pid: 0` and no executable (unknown).
//! - **Client side.** Before connecting, the client checks that the `run` directory and the
//!   socket file are owned by the current user (only the user, or root, can create a file in
//!   a `0700` directory the user owns), so it never talks to another user's socket.
//! - **Signatures.** Linux builds are never signed, so [`PeerPolicy::InstalledBinary`] is
//!   treated as [`PeerPolicy::SameUser`].
//!
//! # Exclusive bind
//!
//! `bind` first takes an exclusive lock on `<socket>.lock` (held for the listener's life), so
//! two servers can never both believe they own the name. With the lock held, an existing socket
//! file is removed only after a connection attempt shows that nothing answers on it (a stale
//! socket left by a killed server); if something does answer, the result is `EndpointInUse`.
//!
//! # Closing
//!
//! A thread blocked in `accept` is woken by [`LocalListener::close`], which sets a flag and
//! connects to the socket once; `accept` sees the flag and returns `Closed`. No polling.

use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy,
    PeerRejection, PlatformError, Result,
};

fn os(e: &io::Error, context: &'static str) -> PlatformError {
    PlatformError::os(i64::from(e.raw_os_error().unwrap_or(-1)), context)
}

/// The effective user id of this process: the owner of `/proc/self`.
fn own_uid() -> Result<u32> {
    fs::metadata("/proc/self")
        .map(|m| m.uid())
        .map_err(|e| os(&e, "stat /proc/self"))
}

/// Makes sure `dir` exists, is a directory owned by `uid` and has mode `0700`.
pub(crate) fn private_dir(dir: &Path, uid: u32) -> Result<()> {
    if !dir.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| os(&e, "mkdir run directory"))?;
    }
    let meta = fs::symlink_metadata(dir).map_err(|e| os(&e, "stat run directory"))?;
    if !meta.is_dir() {
        return Err(PlatformError::Invalid {
            context: "the run directory is not a directory",
        });
    }
    if meta.uid() != uid {
        return Err(PlatformError::PeerRejected(PeerRejection::OtherUser));
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| os(&e, "chmod run directory"))?;
    }
    Ok(())
}

/// The Linux endpoint. See the [module docs](self).
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxEndpoint;

impl LinuxEndpoint {
    /// The endpoint.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn identity(uid: u32) -> PeerIdentity {
    PeerIdentity {
        pid: 0,
        user: uid.to_string(),
        executable: None,
        trusted_binary: false,
    }
}

fn lock_path(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

impl LocalEndpoint for LinuxEndpoint {
    fn bind(&self, name: &EndpointName, _policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        let uid = own_uid()?;
        private_dir(name.run_dir(), uid)?;
        let path = name.socket_path();
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(&path))
            .map_err(|e| os(&e, "open socket lock"))?;
        let _ = fs::set_permissions(lock_path(&path), fs::Permissions::from_mode(0o600));
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(PlatformError::EndpointInUse),
            Err(TryLockError::Error(e)) => return Err(os(&e, "lock socket lock")),
        }
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                // A socket file exists. Replace it only if nothing answers on it.
                if UnixStream::connect(&path).is_ok() {
                    return Err(PlatformError::EndpointInUse);
                }
                fs::remove_file(&path).map_err(|e| os(&e, "remove stale socket"))?;
                UnixListener::bind(&path).map_err(|e| os(&e, "bind"))?
            }
            Err(e) => return Err(os(&e, "bind")),
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| os(&e, "chmod socket"))?;
        Ok(Box::new(Listener {
            socket: listener,
            path,
            uid,
            closed: AtomicBool::new(false),
            lock: Mutex::new(Some(lock)),
        }))
    }

    fn connect(
        &self,
        name: &EndpointName,
        _policy: PeerPolicy,
        _timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        let uid = own_uid()?;
        let path = name.socket_path();
        let Ok(dir) = fs::symlink_metadata(name.run_dir()) else {
            return Err(PlatformError::EndpointNotFound);
        };
        let Ok(sock) = fs::symlink_metadata(&path) else {
            return Err(PlatformError::EndpointNotFound);
        };
        if dir.uid() != uid || sock.uid() != uid {
            return Err(PlatformError::PeerRejected(PeerRejection::OtherUser));
        }
        // A Unix socket connect completes at once (the backlog takes it) or fails at once, so
        // there is nothing to wait for.
        let stream = UnixStream::connect(&path).map_err(|e| match e.kind() {
            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound => {
                PlatformError::EndpointNotFound
            }
            _ => os(&e, "connect"),
        })?;
        Ok(Box::new(Stream::new(stream, identity(uid))))
    }
}

struct Listener {
    socket: UnixListener,
    path: PathBuf,
    uid: u32,
    closed: AtomicBool,
    /// The exclusive lock on `<socket>.lock`, released on close.
    lock: Mutex<Option<File>>,
}

impl LocalListener for Listener {
    fn accept(&self) -> Result<Box<dyn LocalStream>> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(PlatformError::Closed);
            }
            match self.socket.accept() {
                Ok((stream, _)) => {
                    if self.closed.load(Ordering::SeqCst) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return Err(PlatformError::Closed);
                    }
                    return Ok(Box::new(Stream::new(stream, identity(self.uid))));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(os(&e, "accept")),
            }
        }
    }

    fn close(&self) -> Result<()> {
        if !self.closed.swap(true, Ordering::SeqCst) {
            // Wake a blocked accept.
            let _ = UnixStream::connect(&self.path);
            let _ = fs::remove_file(&self.path);
            self.lock
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
        }
        Ok(())
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct Stream {
    stream: UnixStream,
    peer: Arc<PeerIdentity>,
}

impl Stream {
    fn new(stream: UnixStream, peer: PeerIdentity) -> Self {
        Self {
            stream,
            peer: Arc::new(peer),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl LocalStream for Stream {
    fn peer(&self) -> &PeerIdentity {
        &self.peer
    }

    fn try_clone(&self) -> Result<Box<dyn LocalStream>> {
        let stream = self.stream.try_clone().map_err(|e| os(&e, "dup"))?;
        Ok(Box::new(Self {
            stream,
            peer: Arc::clone(&self.peer),
        }))
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        // A zero timeout is an error for the socket; the shortest real wait stands in for it.
        let timeout = timeout.map(|t| t.max(Duration::from_millis(1)));
        self.stream
            .set_read_timeout(timeout)
            .map_err(|e| os(&e, "set read timeout"))
    }

    fn shutdown(&self) -> Result<()> {
        match self.stream.shutdown(std::net::Shutdown::Both) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotConnected => Ok(()),
            Err(e) => Err(os(&e, "shutdown")),
        }
    }
}
