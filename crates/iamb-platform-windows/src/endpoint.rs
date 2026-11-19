//! The local endpoint: a named pipe (architecture §3.3, §6.1; ADR-0006).
//!
//! - The pipe is `\\.\pipe\<stem>-<user SID>`, the stem from [`EndpointName::pipe_stem`]. When
//!   the data root is not the default one (`%APPDATA%\Iamb`), a suffix derived from
//!   [`EndpointName::run_dir`] is added, so tests never meet the installed service
//!   ([`pipe_name`]).
//! - `bind` creates the pipe with `FILE_FLAG_FIRST_PIPE_INSTANCE` (a second server gets
//!   [`EndpointInUse`](PlatformError::EndpointInUse) and must not start), with a security
//!   descriptor whose DACL admits the current user's SID only (`D:P(A;;GA;;;<SID>)`), and with
//!   `PIPE_REJECT_REMOTE_CLIENTS`. One instance always waits for the next client, so the name
//!   stays held while the listener lives.
//! - `accept` identifies the client with `GetNamedPipeClientProcessId`, the user SID of that
//!   process's token and its image path; `connect` identifies the server the same way with
//!   `GetNamedPipeServerProcessId`, before anything is written. Both decide with
//!   [`PeerPolicy::check`]. A rejected client is disconnected before `accept` reports it.
//!   Clients open the pipe with `SECURITY_IDENTIFICATION`, so a server can never act as them.
//! - Under [`PeerPolicy::InstalledBinary`] the peer is trusted when its executable lives in the
//!   installation directory of the running process (the path check) and its Authenticode
//!   signature verifies with `WinVerifyTrust` (revocation checks off and URL retrieval from the
//!   cache only, so nothing touches the network). Iamb is signed from Phase 5: until then a
//!   release build trusts no peer, and debug builds use [`PeerPolicy::SameUser`].
//! - Streams use overlapped I/O, so a read blocked on one thread does not hold up a write on a
//!   clone; a read timeout cancels the blocked read, and `shutdown` cancels every pending
//!   operation on the handle (and disconnects, on the server side).

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy,
    PeerRejection, PlatformError, Result,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};

use crate::Platform;

/// The endpoint's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

/// The pipe's buffer sizes; frames larger than this simply take several reads.
const BUFFER: u32 = 64 * 1024;

fn last_error() -> u32 {
    // SAFETY: `GetLastError` only reads the calling thread's last-error value.
    unsafe { GetLastError() }
}

fn os(context: &'static str) -> PlatformError {
    PlatformError::os(i64::from(last_error()), context)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------

/// The `run` directory of the default data root, `%APPDATA%\Iamb\run`.
fn default_run_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(|a| PathBuf::from(a).join("Iamb").join("run"))
}

/// FNV-1a over the case-folded path: stable across runs and builds.
fn path_hash(path: &Path) -> u64 {
    let text = path.to_string_lossy().to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.trim_end_matches(['\\', '/']).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The pipe's full name for `name`, the user `sid`, and the default `run` directory.
pub(crate) fn pipe_name(name: &EndpointName, sid: &str, default_run: Option<&Path>) -> String {
    let base = format!(r"\\.\pipe\{}-{sid}", name.pipe_stem());
    let is_default = default_run.is_some_and(|d| path_hash(d) == path_hash(name.run_dir()));
    if is_default {
        base
    } else {
        format!("{base}-{:016x}", path_hash(name.run_dir()))
    }
}

// ---------------------------------------------------------------------------------------------
// Handles, processes, users
// ---------------------------------------------------------------------------------------------

/// An owned kernel handle.
struct Handle(HANDLE);

// SAFETY: a kernel handle is a process-wide value that any thread may use; the operations on it
// used here are thread-safe.
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

impl Handle {
    fn new(h: HANDLE) -> Option<Self> {
        (!h.is_null() && h != INVALID_HANDLE_VALUE).then_some(Self(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A manual-reset event, not signalled.
fn event() -> Result<Handle> {
    // SAFETY: null attributes and name are allowed; the event is manual-reset and unsignalled.
    Handle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
        .ok_or_else(|| os("CreateEventW"))
}

/// Memory returned by an API that must be freed with `LocalFree`.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer was allocated by the system with `LocalAlloc` and is freed once.
            unsafe { LocalFree(self.0) };
        }
    }
}

// SAFETY: the security descriptor is immutable after creation and only read by the system.
unsafe impl Send for Local {}
// SAFETY: as above.
unsafe impl Sync for Local {}

/// The string SID of the user a process token belongs to.
fn token_user(process: HANDLE) -> Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` is a valid process handle with query rights; `token` is an out-pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(os("OpenProcessToken"));
    }
    let token = Handle::new(token).ok_or_else(|| os("OpenProcessToken"))?;
    // TOKEN_USER is followed by the SID itself; 256 bytes hold any SID. `u64` aligns it.
    let mut buf = [0_u64; 64];
    let mut len = 0_u32;
    // SAFETY: `buf` is writable for its full size and suitably aligned for TOKEN_USER.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            u32::try_from(size_of_val(&buf)).unwrap_or(512),
            &raw mut len,
        )
    };
    if ok == 0 {
        return Err(os("GetTokenInformation"));
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER whose SID points inside it.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` is a valid SID; `text` receives a LocalAlloc'd, NUL-terminated string.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return Err(os("ConvertSidToStringSidW"));
    }
    let owned = Local(text.cast::<c_void>());
    let mut n = 0;
    // SAFETY: the string is NUL-terminated; we read up to the terminator.
    while unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `text` points to `n` valid UTF-16 units.
    let units = unsafe { std::slice::from_raw_parts(text, n) };
    let sid = String::from_utf16_lossy(units);
    drop(owned);
    Ok(sid)
}

fn own_sid() -> Result<String> {
    // SAFETY: the pseudo handle of the current process is always valid and needs no closing.
    token_user(unsafe { GetCurrentProcess() })
}

/// The user and image path of process `pid`.
fn process_identity(pid: u32) -> Result<(String, Option<PathBuf>)> {
    // SAFETY: `OpenProcess` with limited query rights has no other preconditions.
    let process = Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
        .ok_or(PlatformError::PeerRejected(PeerRejection::Unidentified))?;
    let user = token_user(process.0)
        .map_err(|_| PlatformError::PeerRejected(PeerRejection::Unidentified))?;
    let mut buf = vec![0_u16; 32 * 1024];
    let mut len = u32::try_from(buf.len()).unwrap_or(32 * 1024);
    // SAFETY: `buf` holds `len` units; `len` is updated with the length written.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            buf.as_mut_ptr(),
            &raw mut len,
        )
    };
    let exe = (ok != 0).then(|| {
        buf.truncate(usize::try_from(len).unwrap_or(0));
        PathBuf::from(String::from_utf16_lossy(&buf))
    });
    Ok((user, exe))
}

/// True when `peer` lives in the directory of `own` (the path half of the installed-binary
/// check). Compared case-insensitively, as NTFS does.
pub(crate) fn same_installation(own: &Path, peer: &Path) -> bool {
    match (own.parent(), peer.parent()) {
        (Some(a), Some(b)) => {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        }
        _ => false,
    }
}

/// True when `path` carries a valid Authenticode signature. No revocation checks and no URL
/// retrieval beyond the local cache: this never touches the network.
fn signature_valid(path: &Path) -> bool {
    let file = wide(&path.to_string_lossy());
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: u32::try_from(size_of::<WINTRUST_FILE_INFO>()).unwrap_or(0),
        pcwszFilePath: file.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: u32::try_from(size_of::<WINTRUST_DATA>()).unwrap_or(0),
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..WINTRUST_DATA::default()
    };
    data.Anonymous.pFile = &raw mut info;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` and `info` are fully initialised and outlive both calls; the file path is
    // NUL-terminated; a null window means no UI.
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &raw mut action,
            (&raw mut data).cast::<c_void>(),
        )
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closing the state the verification opened, with the same structures.
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &raw mut action,
            (&raw mut data).cast::<c_void>(),
        );
    }
    status == 0
}

/// Identifies process `pid` at the other end and checks it against `policy` for a process of
/// `own_user`.
fn verify(pid: u32, own_user: &str, policy: PeerPolicy) -> Result<PeerIdentity> {
    let (user, executable) = process_identity(pid)?;
    let trusted_binary = policy == PeerPolicy::InstalledBinary
        && executable.as_deref().is_some_and(|peer| {
            std::env::current_exe().is_ok_and(|own| same_installation(&own, peer))
                && signature_valid(peer)
        });
    let peer = PeerIdentity {
        pid,
        user,
        executable,
        trusted_binary,
    };
    policy
        .check(own_user, &peer)
        .map_err(PlatformError::PeerRejected)?;
    Ok(peer)
}

// ---------------------------------------------------------------------------------------------
// Overlapped I/O
// ---------------------------------------------------------------------------------------------

/// How an overlapped operation ended.
enum Done {
    Bytes(u32),
    Failed(u32),
    TimedOut,
}

/// Waits for the operation on `ov` (started on `handle`, signalling `event` when it completes)
/// up to `timeout`; on a timeout it is cancelled and waited for, so `ov` and its buffer are
/// free when this returns.
///
/// `ov` is a raw pointer, not a reference: until the operation completes the system writes
/// the structure's status fields, and a shared reference would promise that the memory does
/// not change (the 2.8 review, finding 20). The event is passed on its own for the same
/// reason, so that nothing reads the structure while the system may write it.
fn finish(
    handle: HANDLE,
    ov: *const OVERLAPPED,
    event: HANDLE,
    started: bool,
    timeout: Option<Duration>,
) -> Done {
    if !started {
        let code = last_error();
        if code != ERROR_IO_PENDING {
            return Done::Failed(code);
        }
        let ms = timeout.map_or(INFINITE, |t| {
            u32::try_from(t.as_millis()).unwrap_or(INFINITE - 1).max(1)
        });
        // SAFETY: `event` is the valid event of `ov`, which the system signals on completion.
        let waited = unsafe { WaitForSingleObject(event, ms) };
        if waited == WAIT_TIMEOUT {
            // SAFETY: cancels only this operation; `ov` stays valid until the wait below.
            unsafe { CancelIoEx(handle, ov) };
            let mut n = 0;
            // SAFETY: waits for the cancelled operation to complete.
            let ok = unsafe { GetOverlappedResult(handle, ov, &raw mut n, 1) };
            // The operation may have completed just before the cancel.
            return if ok != 0 && n > 0 {
                Done::Bytes(n)
            } else {
                Done::TimedOut
            };
        }
    }
    let mut n = 0;
    // SAFETY: the operation has completed (or completed synchronously); this reads its result.
    let ok = unsafe { GetOverlappedResult(handle, ov, &raw mut n, 1) };
    if ok != 0 {
        Done::Bytes(n)
    } else {
        Done::Failed(last_error())
    }
}

struct Pipe {
    handle: Handle,
    server: bool,
    read_timeout: Mutex<Option<Duration>>,
    closed: AtomicBool,
}

impl Pipe {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.closed.load(Ordering::SeqCst) {
            return Ok(0);
        }
        let timeout = *self
            .read_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let ev = event().map_err(|_| io::Error::other("CreateEventW failed"))?;
        let mut ov = OVERLAPPED {
            hEvent: ev.0,
            ..OVERLAPPED::default()
        };
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: `buf` and `ov` stay valid until `finish` has seen the operation complete.
        let started = unsafe {
            ReadFile(
                self.handle.0,
                buf.as_mut_ptr(),
                len,
                std::ptr::null_mut(),
                &raw mut ov,
            )
        } != 0;
        match finish(self.handle.0, &raw const ov, ev.0, started, timeout) {
            Done::Bytes(n) => Ok(usize::try_from(n).unwrap_or(0)),
            Done::TimedOut => Err(io::ErrorKind::TimedOut.into()),
            Done::Failed(
                ERROR_BROKEN_PIPE
                | ERROR_PIPE_NOT_CONNECTED
                | ERROR_OPERATION_ABORTED
                | ERROR_NO_DATA,
            ) => Ok(0),
            Done::Failed(code) => Err(io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(-1),
            )),
        }
    }

    fn write(&self, buf: &[u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let ev = event().map_err(|_| io::Error::other("CreateEventW failed"))?;
        let mut ov = OVERLAPPED {
            hEvent: ev.0,
            ..OVERLAPPED::default()
        };
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: `buf` and `ov` stay valid until `finish` has seen the operation complete.
        let started = unsafe {
            WriteFile(
                self.handle.0,
                buf.as_ptr(),
                len,
                std::ptr::null_mut(),
                &raw mut ov,
            )
        } != 0;
        match finish(self.handle.0, &raw const ov, ev.0, started, None) {
            Done::Bytes(n) => Ok(usize::try_from(n).unwrap_or(0)),
            Done::TimedOut | Done::Failed(_) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    fn shutdown(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            // SAFETY: cancels every pending operation on the handle, from any thread.
            unsafe { CancelIoEx(self.handle.0, std::ptr::null()) };
            if self.server {
                // SAFETY: the handle is a server end of a pipe; the client then reads the end.
                unsafe { DisconnectNamedPipe(self.handle.0) };
            }
        }
    }
}

struct Stream {
    pipe: Arc<Pipe>,
    peer: Arc<PeerIdentity>,
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.pipe.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pipe.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LocalStream for Stream {
    fn peer(&self) -> &PeerIdentity {
        &self.peer
    }

    fn try_clone(&self) -> Result<Box<dyn LocalStream>> {
        Ok(Box::new(Self {
            pipe: Arc::clone(&self.pipe),
            peer: Arc::clone(&self.peer),
        }))
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        *self
            .pipe
            .read_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = timeout;
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        self.pipe.shutdown();
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------------------------

/// The pipe's security descriptor: full access for `sid`, nobody else, inheritance blocked.
fn security_descriptor(sid: &str) -> Result<Local> {
    let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'd descriptor.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(os("ConvertStringSecurityDescriptorToSecurityDescriptorW"));
    }
    Ok(Local(sd))
}

struct Listener {
    name: Vec<u16>,
    sd: Local,
    own_user: String,
    policy: PeerPolicy,
    /// The instance that waits for the next client.
    pending: Mutex<Option<Handle>>,
    close_event: Handle,
    closed: AtomicBool,
}

impl Listener {
    fn instance(&self, first: bool) -> Result<Handle> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: self.sd.0,
            bInheritHandle: 0,
        };
        let open_mode = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        // SAFETY: `name` is NUL-terminated; `attributes` and the descriptor it points to are
        // valid for the call.
        let h = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                &raw const attributes,
            )
        };
        match Handle::new(h) {
            Some(h) => Ok(h),
            None if first => {
                let code = last_error();
                // The name exists: another server created it first (ERROR_ACCESS_DENIED), or it
                // is busy with all instances (ERROR_PIPE_BUSY).
                if code == windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED
                    || code == ERROR_PIPE_BUSY
                {
                    Err(PlatformError::EndpointInUse)
                } else {
                    Err(PlatformError::os(i64::from(code), "CreateNamedPipeW"))
                }
            }
            None => Err(os("CreateNamedPipeW")),
        }
    }
}

impl LocalListener for Listener {
    fn accept(&self) -> Result<Box<dyn LocalStream>> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(PlatformError::Closed);
            }
            let pipe = match self
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                Some(p) => p,
                None => self.instance(false)?,
            };
            let ev = event()?;
            let mut ov = OVERLAPPED {
                hEvent: ev.0,
                ..OVERLAPPED::default()
            };
            // SAFETY: `pipe` is a server instance; `ov` stays valid until the operation ends.
            let connected = unsafe { ConnectNamedPipe(pipe.0, &raw mut ov) } != 0;
            if !connected {
                match last_error() {
                    ERROR_PIPE_CONNECTED => {}
                    ERROR_IO_PENDING => {
                        let handles = [ev.0, self.close_event.0];
                        // SAFETY: both handles are valid events.
                        let woke =
                            unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                        if woke != WAIT_OBJECT_0 {
                            // SAFETY: cancels the pending connect; then wait for it to end.
                            unsafe { CancelIoEx(pipe.0, &raw const ov) };
                            let mut n = 0;
                            // SAFETY: waits for the cancelled operation so `ov` can be freed.
                            unsafe { GetOverlappedResult(pipe.0, &raw const ov, &raw mut n, 1) };
                            return Err(PlatformError::Closed);
                        }
                        let mut n = 0;
                        // SAFETY: the connect completed; this reads its result.
                        if unsafe { GetOverlappedResult(pipe.0, &raw const ov, &raw mut n, 1) } == 0
                        {
                            continue;
                        }
                    }
                    _ => continue,
                }
            }
            // Keep the name held: the next instance waits before this one is handed out.
            if let Ok(next) = self.instance(false) {
                *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(next);
            }
            let mut pid = 0;
            // SAFETY: `pipe` is a connected server instance; `pid` is an out-pointer.
            let known = unsafe { GetNamedPipeClientProcessId(pipe.0, &raw mut pid) } != 0;
            let verdict = if known {
                verify(pid, &self.own_user, self.policy)
            } else {
                Err(PlatformError::PeerRejected(PeerRejection::Unidentified))
            };
            return match verdict {
                Ok(peer) => Ok(Box::new(Stream {
                    pipe: Arc::new(Pipe {
                        handle: pipe,
                        server: true,
                        read_timeout: Mutex::new(None),
                        closed: AtomicBool::new(false),
                    }),
                    peer: Arc::new(peer),
                })),
                Err(e) => {
                    // SAFETY: disconnects the rejected client before its handle is closed.
                    unsafe { DisconnectNamedPipe(pipe.0) };
                    Err(e)
                }
            };
        }
    }

    fn close(&self) -> Result<()> {
        if !self.closed.swap(true, Ordering::SeqCst) {
            // SAFETY: the event is valid; signalling wakes a blocked accept.
            unsafe { SetEvent(self.close_event.0) };
            self.pending
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

impl LocalEndpoint for Platform {
    fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        let State {} = &self.endpoint;
        let own_user = own_sid()?;
        let full = pipe_name(name, &own_user, default_run_dir().as_deref());
        let listener = Listener {
            name: wide(&full),
            sd: security_descriptor(&own_user)?,
            own_user,
            policy,
            pending: Mutex::new(None),
            close_event: event()?,
            closed: AtomicBool::new(false),
        };
        let first = listener.instance(true)?;
        *listener
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(first);
        Ok(Box::new(listener))
    }

    fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        let own_user = own_sid()?;
        let full = wide(&pipe_name(name, &own_user, default_run_dir().as_deref()));
        let deadline = Instant::now() + timeout;
        let handle = loop {
            // SAFETY: `full` is NUL-terminated; null security attributes and template are
            // allowed. SECURITY_IDENTIFICATION keeps the server from acting as this process.
            let h = unsafe {
                CreateFileW(
                    full.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    std::ptr::null_mut(),
                )
            };
            if let Some(h) = Handle::new(h) {
                break h;
            }
            match last_error() {
                ERROR_FILE_NOT_FOUND => return Err(PlatformError::EndpointNotFound),
                ERROR_PIPE_BUSY => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(PlatformError::Timeout);
                    }
                    let ms = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).max(1);
                    // SAFETY: `full` is NUL-terminated.
                    unsafe { WaitNamedPipeW(full.as_ptr(), ms) };
                }
                _ => return Err(os("CreateFileW")),
            }
        };
        let mut pid = 0;
        // SAFETY: `handle` is a client end of a pipe; `pid` is an out-pointer.
        if unsafe { GetNamedPipeServerProcessId(handle.0, &raw mut pid) } == 0 {
            return Err(PlatformError::PeerRejected(PeerRejection::Unidentified));
        }
        let peer = verify(pid, &own_user, policy)?;
        Ok(Box::new(Stream {
            pipe: Arc::new(Pipe {
                handle,
                server: false,
                read_timeout: Mutex::new(None),
                closed: AtomicBool::new(false),
            }),
            peer: Arc::new(peer),
        }))
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::EndpointKind;

    use super::*;

    fn peer(user: &str, trusted: bool) -> PeerIdentity {
        PeerIdentity {
            pid: 7,
            user: user.to_owned(),
            executable: None,
            trusted_binary: trusted,
        }
    }

    #[test]
    fn pipe_names_follow_the_architecture() {
        let default = Path::new(r"C:\Users\Ada\AppData\Roaming\Iamb\run");
        let name = EndpointName::new(EndpointKind::Service, default);
        assert_eq!(
            pipe_name(&name, "S-1-5-21-1", Some(default)),
            r"\\.\pipe\iamb-service-S-1-5-21-1"
        );
        // Case and a trailing separator do not make it another root.
        let same = EndpointName::new(
            EndpointKind::Service,
            r"c:\users\ada\appdata\roaming\iamb\run\",
        );
        assert_eq!(
            pipe_name(&same, "S-1-5-21-1", Some(default)),
            r"\\.\pipe\iamb-service-S-1-5-21-1"
        );
        let test = EndpointName::new(EndpointKind::Sync, r"C:\Temp\x\run");
        let n = pipe_name(&test, "S-1-5-21-1", Some(default));
        assert!(n.starts_with(r"\\.\pipe\iamb-sync-S-1-5-21-1-"), "{n}");
        assert_eq!(n.len(), r"\\.\pipe\iamb-sync-S-1-5-21-1-".len() + 16);
        assert_ne!(
            n,
            pipe_name(
                &EndpointName::new(EndpointKind::Sync, r"C:\Temp\y\run"),
                "S-1-5-21-1",
                Some(default)
            )
        );
    }

    #[test]
    fn another_sid_is_rejected_through_the_shared_check() {
        // A second user cannot be simulated on CI: the decision is the shared pure function,
        // fed with the SID strings this module reads from tokens.
        let own = own_sid().expect("own SID");
        assert!(own.starts_with("S-1-"), "{own}");
        assert_eq!(PeerPolicy::SameUser.check(&own, &peer(&own, false)), Ok(()));
        assert_eq!(
            PeerPolicy::SameUser.check(&own, &peer("S-1-5-18", true)),
            Err(PeerRejection::OtherUser)
        );
        assert_eq!(
            PeerPolicy::InstalledBinary.check(&own, &peer(&own, false)),
            Err(PeerRejection::UntrustedBinary)
        );
        // This process, identified through its pid, is the same user.
        let me = verify(std::process::id(), &own, PeerPolicy::SameUser).expect("self");
        assert_eq!(me.user, own);
        assert!(me.executable.is_some());
    }

    #[test]
    fn the_installation_is_the_executable_directory() {
        let own = Path::new(r"C:\Program Files\Iamb\iamb-service.exe");
        assert!(same_installation(
            own,
            Path::new(r"c:\program files\iamb\iamb.exe")
        ));
        assert!(!same_installation(own, Path::new(r"C:\Temp\iamb.exe")));
    }

    #[test]
    fn an_unsigned_test_binary_does_not_verify() {
        let exe = std::env::current_exe().expect("test binary");
        assert!(!signature_valid(&exe));
    }
}
