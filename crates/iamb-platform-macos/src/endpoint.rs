//! The local endpoint: a Unix domain socket (architecture §3.3, §6.1; ADR-0006).
//!
//! - The socket is [`EndpointName::socket_path`], in the data root's `run` directory. `bind`
//!   makes sure that directory is owned by the current user with mode `0700` (creating it so,
//!   tightening a looser one, refusing one another user owns), and binds exclusively: it first
//!   takes an exclusive lock on `<socket>.lock`, held for the listener's life, and then removes
//!   an existing socket file only if nothing answers on it (a stale socket of a killed server);
//!   if something answers, the result is [`EndpointInUse`](PlatformError::EndpointInUse). The
//!   socket file is `0600`.
//! - `accept` and `connect` identify the peer from the socket itself: its effective user id
//!   (`getpeereid`), its process id (`LOCAL_PEERPID`) and the path of its executable
//!   (`proc_pidpath`), and decide with [`PeerPolicy::check`]. A rejected client is closed before
//!   `accept` reports it; a rejected server is never written to.
//! - Under [`PeerPolicy::InstalledBinary`] the peer is trusted only when its executable lives in
//!   the same `Iamb.app` bundle as this process and its code signature is valid and satisfies
//!   this process's own designated requirement (`SecCodeCopyGuestWithAttributes` with the pid,
//!   then `SecCodeCheckValidity`; no network access). An unsigned development build therefore
//!   trusts no one under that policy; debug builds use [`PeerPolicy::SameUser`].
//! - Streams are the socket itself: clones are duplicated descriptors, the read timeout is the
//!   socket's, `shutdown` shuts both directions down. A blocked `accept` waits in `poll` on the
//!   socket and on a private socket pair; `close` sets a flag and writes one byte to the pair,
//!   which wakes it. The wake-up does not go through the socket's path, so a data folder or
//!   `run/` removed while the service runs cannot leave `accept` blocked and the worker's stop
//!   waiting for it for ever (the 2026-11 beta review's finding 10). The listening socket is
//!   non-blocking; an accepted stream is made blocking again.

use std::ffi::c_void;
use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy,
    PeerRejection, PlatformError, Result,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_security::{SecCSFlags, SecCode, SecRequirement, SecStaticCode, kSecGuestAttributePid};

use crate::Platform;

/// The endpoint's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

fn os(e: &io::Error, context: &'static str) -> PlatformError {
    PlatformError::os(i64::from(e.raw_os_error().unwrap_or(-1)), context)
}

fn own_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Makes sure `dir` exists, is a directory owned by `uid` and has mode `0700`.
fn private_dir(dir: &Path, uid: u32) -> Result<()> {
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

fn lock_path(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// The outermost `.app` bundle that contains `exe`, if any.
pub(crate) fn outer_bundle(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .last()
}

/// True when `peer` is in the same `Iamb.app` bundle as `own` (the path half of the
/// installed-binary check).
pub(crate) fn same_installation(own: &Path, peer: &Path) -> bool {
    match (outer_bundle(own), outer_bundle(peer)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The user id, process id and executable of the socket's peer.
fn peer_of(stream: &UnixStream) -> Result<(u32, u32, Option<PathBuf>)> {
    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `fd` is an open socket owned by `stream`; both out-pointers are valid.
    if unsafe { libc::getpeereid(fd, &raw mut uid, &raw mut gid) } != 0 {
        return Err(PlatformError::PeerRejected(PeerRejection::Unidentified));
    }
    let mut pid: libc::pid_t = 0;
    let mut len = libc::socklen_t::try_from(size_of::<libc::pid_t>()).unwrap_or(4);
    // SAFETY: `fd` is an open socket; `pid` and `len` are valid out-pointers of the size given.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast::<c_void>(),
            &raw mut len,
        )
    };
    if rc != 0 {
        return Err(PlatformError::PeerRejected(PeerRejection::Unidentified));
    }
    let mut buf = vec![0_u8; usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE).unwrap_or(4096)];
    let cap = u32::try_from(buf.len()).unwrap_or(4096);
    // SAFETY: `buf` is a writable buffer of `cap` bytes; `proc_pidpath` writes at most that.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast::<c_void>(), cap) };
    let exe = usize::try_from(n).ok().filter(|n| *n > 0).map(|n| {
        buf.truncate(n);
        PathBuf::from(String::from_utf8_lossy(&buf).into_owned())
    });
    Ok((uid, u32::try_from(pid).unwrap_or(0), exe))
}

/// Adopts a +1 reference returned through an out-pointer.
fn adopt<T: objc2_core_foundation::Type>(ptr: *mut T) -> Option<CFRetained<T>> {
    // SAFETY: the Security framework returned `ptr` from a "Copy" function, so we own one
    // reference to it, which `CFRetained` releases.
    NonNull::new(ptr).map(|p| unsafe { CFRetained::from_raw(p) })
}

/// True when the running process `pid` has a valid code signature that satisfies this
/// process's own designated requirement.
fn signed_like_us(pid: u32) -> bool {
    let flags = SecCSFlags::DefaultFlags;
    let mut me: *mut SecCode = std::ptr::null_mut();
    // SAFETY: `me` is a valid out-pointer.
    if unsafe { SecCode::copy_self(flags, NonNull::from(&mut me)) } != 0 {
        return false;
    }
    let Some(me) = adopt(me) else { return false };
    let mut me_static: *const SecStaticCode = std::ptr::null();
    // SAFETY: `me` is a valid code object; `me_static` is a valid out-pointer.
    if unsafe { me.copy_static_code(flags, NonNull::from(&mut me_static)) } != 0 {
        return false;
    }
    let Some(me_static) = adopt(me_static.cast_mut()) else {
        return false;
    };
    let mut requirement: *mut SecRequirement = std::ptr::null_mut();
    // SAFETY: `me_static` is a valid static code object; `requirement` a valid out-pointer.
    if unsafe {
        SecCode::copy_designated_requirement(&me_static, flags, NonNull::from(&mut requirement))
    } != 0
    {
        return false;
    }
    let Some(requirement) = adopt(requirement) else {
        return false;
    };
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    let number = CFNumber::new_i32(pid);
    // SAFETY: `kSecGuestAttributePid` is a constant string of the Security framework.
    let key: &CFString = unsafe { kSecGuestAttributePid };
    let attributes = CFDictionary::<CFString, CFNumber>::from_slices(&[key], &[&number]);
    let mut guest: *mut SecCode = std::ptr::null_mut();
    // SAFETY: the attributes dictionary maps the pid key to a CFNumber, as the API requires;
    // `guest` is a valid out-pointer.
    if unsafe {
        SecCode::copy_guest_with_attributes(
            None,
            Some(attributes.as_opaque()),
            flags,
            NonNull::from(&mut guest),
        )
    } != 0
    {
        return false;
    }
    let Some(guest) = adopt(guest) else {
        return false;
    };
    // SAFETY: both objects are valid; no network access is requested by these flags.
    unsafe { guest.check_validity(flags, Some(&requirement)) == 0 }
}

/// Identifies the peer of `stream` and checks it against `policy`.
fn verify(stream: &UnixStream, policy: PeerPolicy) -> Result<PeerIdentity> {
    let (uid, pid, executable) = peer_of(stream)?;
    let trusted_binary = policy == PeerPolicy::InstalledBinary
        && executable.as_deref().is_some_and(|peer| {
            std::env::current_exe().is_ok_and(|own| same_installation(&own, peer))
        })
        && signed_like_us(pid);
    let peer = PeerIdentity {
        pid,
        user: uid.to_string(),
        executable,
        trusted_binary,
    };
    policy
        .check(&own_uid().to_string(), &peer)
        .map_err(PlatformError::PeerRejected)?;
    Ok(peer)
}

impl LocalEndpoint for Platform {
    fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        let State {} = &self.endpoint;
        private_dir(name.run_dir(), own_uid())?;
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
        let socket = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
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
        socket
            .set_nonblocking(true)
            .map_err(|e| os(&e, "non-blocking socket"))?;
        let (wake_rx, wake_tx) = UnixStream::pair().map_err(|e| os(&e, "wake pair"))?;
        Ok(Box::new(Listener {
            socket,
            path,
            policy,
            closed: AtomicBool::new(false),
            lock: Mutex::new(Some(lock)),
            wake_rx,
            wake_tx,
        }))
    }

    fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        _timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        // A Unix socket connect completes at once (the backlog takes it) or fails at once.
        let stream = UnixStream::connect(name.socket_path()).map_err(|e| match e.kind() {
            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound => {
                PlatformError::EndpointNotFound
            }
            _ => os(&e, "connect"),
        })?;
        match verify(&stream, policy) {
            Ok(peer) => Ok(Box::new(Stream::new(stream, peer))),
            Err(e) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                Err(e)
            }
        }
    }
}

struct Listener {
    socket: UnixListener,
    path: PathBuf,
    policy: PeerPolicy,
    closed: AtomicBool,
    lock: Mutex<Option<File>>,
    /// Polled beside the socket; `close` writes to `wake_tx` (module docs).
    wake_rx: UnixStream,
    wake_tx: UnixStream,
}

impl Listener {
    /// Waits until the socket has a connection to accept (`true`) or `close` was called
    /// (`false`).
    fn wait_readable(&self) -> Result<bool> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Ok(false);
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.socket.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.wake_rx.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: `fds` is an array of two initialised `pollfd`s whose descriptors are
            // owned by `self` and open; the length passed is the array's.
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if rc < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(os(&e, "poll"));
            }
            if fds[1].revents != 0 || self.closed.load(Ordering::SeqCst) {
                return Ok(false);
            }
            if fds[0].revents != 0 {
                return Ok(true);
            }
        }
    }
}

impl LocalListener for Listener {
    fn accept(&self) -> Result<Box<dyn LocalStream>> {
        loop {
            if !self.wait_readable()? {
                return Err(PlatformError::Closed);
            }
            match self.socket.accept() {
                Ok((stream, _)) => {
                    if self.closed.load(Ordering::SeqCst) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return Err(PlatformError::Closed);
                    }
                    // Accepted from a non-blocking socket, it inherited the flag.
                    if let Err(e) = stream.set_nonblocking(false) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return Err(os(&e, "blocking stream"));
                    }
                    return match verify(&stream, self.policy) {
                        Ok(peer) => Ok(Box::new(Stream::new(stream, peer))),
                        Err(e) => {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            Err(e)
                        }
                    };
                }
                // Another look found the connection gone, or a signal: wait again.
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(e) => return Err(os(&e, "accept")),
            }
        }
    }

    fn close(&self) -> Result<()> {
        if !self.closed.swap(true, Ordering::SeqCst) {
            // Wakes a blocked accept whatever became of the socket's path (module docs).
            let _ = (&self.wake_tx).write(&[1]);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(user: &str, trusted: bool) -> PeerIdentity {
        PeerIdentity {
            pid: 42,
            user: user.to_owned(),
            executable: None,
            trusted_binary: trusted,
        }
    }

    #[test]
    fn another_uid_is_rejected_through_the_shared_check() {
        // A second user cannot be simulated on CI; the decision is the shared pure function
        // fed with the uid strings this module builds.
        let own = own_uid().to_string();
        let other = (own_uid() + 1).to_string();
        assert_eq!(PeerPolicy::SameUser.check(&own, &peer(&own, false)), Ok(()));
        assert_eq!(
            PeerPolicy::SameUser.check(&own, &peer(&other, true)),
            Err(PeerRejection::OtherUser)
        );
        assert_eq!(
            PeerPolicy::InstalledBinary.check(&own, &peer(&own, false)),
            Err(PeerRejection::UntrustedBinary)
        );
    }

    #[test]
    fn the_installation_is_the_outer_app_bundle() {
        let own = Path::new(
            "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service",
        );
        assert_eq!(outer_bundle(own), Some(Path::new("/Applications/Iamb.app")));
        assert!(same_installation(
            own,
            Path::new("/Applications/Iamb.app/Contents/MacOS/Iamb")
        ));
        assert!(!same_installation(
            own,
            Path::new("/tmp/Iamb.app/Contents/MacOS/Iamb")
        ));
        assert!(!same_installation(own, Path::new("/usr/local/bin/iamb")));
        assert!(!same_installation(
            Path::new("/usr/local/bin/iamb-service"),
            Path::new("/usr/local/bin/iamb")
        ));
    }

    #[test]
    fn an_unbundled_test_binary_is_not_signed_like_an_installation() {
        // `cargo test` binaries are ad-hoc signed at most; checking ourselves must not crash
        // and the policy must not trust a test binary.
        let _ = signed_like_us(std::process::id());
    }
}
