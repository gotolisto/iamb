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
//! - Under [`PeerPolicy::InstalledBinary`] the peer is trusted only when it is a member of the
//!   same installation as this process, as `iamb_platform::installation` decides (the 2.8
//!   review's finding 11). The facts: the peer's code object from its audit token
//!   (`LOCAL_PEERTOKEN`, `SecCodeCopyGuestWithAttributes` with `kSecGuestAttributeAudit`, so a
//!   process id reused since cannot stand in), the path that code object names, the executable
//!   `proc_pidpath` reports, that path with its links resolved, and its device and inode. A
//!   signed installation also requires the peer to satisfy `anchor apple generic`, the same
//!   team identifier and Iamb's bundle identifiers (`SecCodeCheckValidity`, no network access);
//!   an ad hoc or unsigned one requires the member's own file instead. Whether this process is
//!   signed is read once, from its running code ([`read_own_signing`], first thing in `main`),
//!   or compiled in (`IAMB_SIGNER_TEAM`); when it cannot be told, no peer is trusted. Debug
//!   builds use [`PeerPolicy::SameUser`].
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

use iamb_platform::installation::{self, OwnState, PathRules, PeerSignature};
use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy,
    PeerRejection, PlatformError, Result,
};
use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFString, CFType, CFURL};
use objc2_security::{
    SecCSFlags, SecCode, SecRequirement, SecStaticCode, kSecCSSigningInformation,
    kSecCodeInfoTeamIdentifier, kSecGuestAttributeAudit,
};

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

/// The audit token of the socket's peer (`LOCAL_PEERTOKEN`): the process, including its
/// generation, at the moment it connected.
fn audit_token(stream: &UnixStream) -> Option<[u32; 8]> {
    let mut token = [0_u32; 8];
    let mut len = libc::socklen_t::try_from(size_of_val(&token)).ok()?;
    // SAFETY: `stream` is an open socket; `token` and `len` are valid out-pointers of the size
    // given (an `audit_token_t` is eight 32-bit words).
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast::<c_void>(),
            &raw mut len,
        )
    };
    (rc == 0 && usize::try_from(len).ok() == Some(size_of_val(&token))).then_some(token)
}

/// The code object of the process `token` names.
fn guest(token: &[u32; 8]) -> Option<CFRetained<SecCode>> {
    let bytes: Vec<u8> = token.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let data = CFData::from_bytes(&bytes);
    // SAFETY: `kSecGuestAttributeAudit` is a constant string of the Security framework.
    let key: &CFString = unsafe { kSecGuestAttributeAudit };
    let attributes = CFDictionary::<CFString, CFData>::from_slices(&[key], &[&data]);
    let mut guest: *mut SecCode = std::ptr::null_mut();
    // SAFETY: the attributes map the audit key to the token's bytes, as the API requires;
    // `guest` is a valid out-pointer.
    let rc = unsafe {
        SecCode::copy_guest_with_attributes(
            None,
            Some(attributes.as_opaque()),
            SecCSFlags::DefaultFlags,
            NonNull::from(&mut guest),
        )
    };
    if rc == 0 { adopt(guest) } else { None }
}

/// The static code (the code on disk) of `code`.
fn static_of(code: &SecCode) -> Option<CFRetained<SecStaticCode>> {
    let mut out: *const SecStaticCode = std::ptr::null();
    // SAFETY: `code` is a valid code object; `out` is a valid out-pointer.
    let rc = unsafe { code.copy_static_code(SecCSFlags::DefaultFlags, NonNull::from(&mut out)) };
    if rc == 0 { adopt(out.cast_mut()) } else { None }
}

/// The path the code-signing API names for `code`: the bundle of an app's main executable, the
/// executable otherwise.
fn code_path(code: &SecStaticCode) -> Option<PathBuf> {
    let mut url: *const CFURL = std::ptr::null();
    // SAFETY: `code` is a valid static code object; `url` is a valid out-pointer.
    let rc = unsafe { SecCode::copy_path(code, SecCSFlags::DefaultFlags, NonNull::from(&mut url)) };
    if rc != 0 {
        return None;
    }
    adopt(url.cast_mut())?.to_file_path()
}

/// The team identifier `code` is signed with; `None` for ad hoc and unsigned code.
fn team_of(code: &SecStaticCode) -> Option<String> {
    let mut info: *const CFDictionary = std::ptr::null();
    // SAFETY: `code` is a valid static code object; `info` is a valid out-pointer.
    let rc = unsafe {
        SecCode::copy_signing_information(
            code,
            SecCSFlags(kSecCSSigningInformation),
            NonNull::from(&mut info),
        )
    };
    if rc != 0 {
        return None;
    }
    let info = adopt(info.cast_mut())?;
    // SAFETY: the signing information dictionary has string keys and object values.
    let info: CFRetained<CFDictionary<CFString, CFType>> =
        unsafe { CFRetained::cast_unchecked(info) };
    // SAFETY: `kSecCodeInfoTeamIdentifier` is a constant string of the Security framework.
    let key: &CFString = unsafe { kSecCodeInfoTeamIdentifier };
    let team = info.get(key)?.downcast::<CFString>().ok()?.to_string();
    (!team.is_empty()).then_some(team)
}

/// The team identifier the release pipeline compiled in for a Developer ID build
/// (`IAMB_SIGNER_TEAM`, `.github/workflows/installers.yml`); absent or empty for every other
/// build.
const PINNED_TEAM: Option<&str> = option_env!("IAMB_SIGNER_TEAM");

/// This process: its canonical executable and its signing state
/// (`iamb_platform::installation`, "This process's own signing state").
struct Own {
    exe: Option<PathBuf>,
    state: OwnState,
}

/// [`Own`], read once: by [`read_own_signing`] at the start of `main`, or at the first peer
/// check of a process that did not call it.
fn own() -> &'static Own {
    static OWN: std::sync::OnceLock<Own> = std::sync::OnceLock::new();
    OWN.get_or_init(|| Own {
        exe: std::env::current_exe()
            .ok()
            .and_then(|e| fs::canonicalize(e).ok()),
        state: installation::pinned(
            PINNED_TEAM,
            |t| installation::valid_team(t).then(|| t.to_owned()),
            running_code_state,
        ),
    })
}

/// Reads this process's signing state now, from its running code, and keeps it for the life of
/// the process; returns the line for the log. Call it first in `main` (the strict review's
/// finding 1): read later, the file it would be judged by may have been replaced.
#[must_use]
pub fn read_own_signing() -> String {
    own().state.describe()
}

/// What this process's running code (its dynamic code object, not the file on disk) says about
/// its signature.
fn running_code_state() -> OwnState {
    let mut me: *mut SecCode = std::ptr::null_mut();
    // SAFETY: `me` is a valid out-pointer.
    if unsafe { SecCode::copy_self(SecCSFlags::DefaultFlags, NonNull::from(&mut me)) } != 0 {
        return OwnState::Unknown("this process's code object could not be had");
    }
    let Some(me) = adopt(me) else {
        return OwnState::Unknown("this process's code object could not be had");
    };
    // SAFETY: `me` is a valid code object; no requirement, no network access.
    let validity = unsafe { me.check_validity(SecCSFlags::NoNetworkAccess, None) };
    // SAFETY: the signing-information call accepts a dynamic code object as well as a static
    // one (both are code objects of the Security framework); it then describes the running
    // code.
    let as_static: &SecStaticCode = unsafe { &*std::ptr::from_ref::<SecCode>(&me).cast() };
    let team = if validity == 0 {
        team_of(as_static)
    } else {
        None
    };
    let anchored = team
        .as_deref()
        .and_then(installation::macos_own_requirement)
        .is_some_and(|text| satisfies(&me, &text));
    installation::classify_macos(validity, team, anchored)
}

/// Whether `code` satisfies the requirement `text`, without network access.
fn satisfies(code: &SecCode, text: &str) -> bool {
    let text = CFString::from_str(text);
    let mut requirement: *mut SecRequirement = std::ptr::null_mut();
    // SAFETY: `text` is a valid string; `requirement` is a valid out-pointer.
    if unsafe {
        SecRequirement::create_with_string(
            &text,
            SecCSFlags::DefaultFlags,
            NonNull::from(&mut requirement),
        )
    } != 0
    {
        return false;
    }
    let Some(requirement) = adopt(requirement) else {
        return false;
    };
    // SAFETY: both objects are valid; the flag forbids network access.
    unsafe { code.check_validity(SecCSFlags::NoNetworkAccess, Some(&requirement)) == 0 }
}

/// Whether `code` satisfies the requirement of a signed installation of `team`
/// (`installation::macos_requirement`), without network access.
fn signed_by(code: &SecCode, team: &str) -> bool {
    installation::macos_requirement(team).is_some_and(|text| satisfies(code, &text))
}

/// True when `a` and `b` are the same regular file (device and inode), `a` not being a link.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::symlink_metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.file_type().is_file() && a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Whether the peer of `stream`, whose executable the system reports as `reported`, is a
/// member of this process's installation (module docs; `iamb_platform::installation`).
fn installed_peer(stream: &UnixStream, reported: Option<&Path>) -> bool {
    let own = own();
    let Some(members) = own.exe.as_deref().and_then(installation::macos_members) else {
        return false;
    };
    // `proc_pidpath` names the executable's vnode, so it holds no link; resolving it is what
    // makes it comparable with the member's canonical path.
    let canonical = reported.and_then(|r| fs::canonicalize(r).ok());
    let Some(member) = installation::member_for(&members, canonical.as_deref(), PathRules::MacOs)
    else {
        return false;
    };
    // The code the audit token names is that member too: not a process that has run another
    // program since, nor one whose pid was reused.
    let Some(code) = audit_token(stream).and_then(|t| guest(&t)) else {
        return false;
    };
    let agrees = static_of(&code)
        .and_then(|s| code_path(&s))
        .is_some_and(|p| installation::member_for_code_path(&members, &p) == Some(member));
    let signature = match own.state.signer() {
        Some(team) if signed_by(&code, team) => PeerSignature::SameSigner,
        _ => PeerSignature::Other,
    };
    let same = canonical
        .as_deref()
        .is_some_and(|c| same_file(c, &member.exe));
    installation::trusted(true, own.state.signing(), signature, agrees, same)
}

/// Identifies the peer of `stream` and checks it against `policy`.
fn verify(stream: &UnixStream, policy: PeerPolicy) -> Result<PeerIdentity> {
    let (uid, pid, executable) = peer_of(stream)?;
    let trusted_binary =
        policy == PeerPolicy::InstalledBinary && installed_peer(stream, executable.as_deref());
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
    fn a_test_binary_is_in_no_installation_and_reading_its_own_signature_does_not_crash() {
        // `cargo test` binaries are ad hoc signed at most and live in `target/`: the policy
        // trusts no peer for them (the decision itself is tested in iamb_platform::installation).
        let own = std::env::current_exe().expect("own path");
        assert_eq!(installation::macos_members(&own), None);
        // Ad hoc at most: unsigned, never "signed" from an error, and the line says so.
        assert!(
            !read_own_signing().starts_with("signed"),
            "{}",
            read_own_signing()
        );
    }
}
