//! The local endpoint: a named pipe (architecture §3.3, §6.1; ADR-0006).
//!
//! - The pipe is `\\.\pipe\<stem>-<user SID>-<secret>`, the stem from
//!   [`EndpointName::pipe_stem`] and the secret 128 random bits drawn for every bind and kept
//!   in `<run>/<kind>.pipe`, where clients read it (`pipe_names` has the rules: the 2.8
//!   review's finding 12, another local user taking the name first). The `run` directory is
//!   restricted to the current user before the secret is written there
//!   ([`restrict_to_current_user`](crate::restrict_to_current_user)).
//! - `bind` first takes an exclusive lock on `<run>/<kind>.pipe.lock`, held for the
//!   listener's life: a second server of the same data root gets
//!   [`EndpointInUse`](PlatformError::EndpointInUse). It then creates the pipe with
//!   `FILE_FLAG_FIRST_PIPE_INSTANCE`, with a security descriptor whose DACL admits the
//!   current user's SID only (`D:P(A;;GA;;;<SID>)`), and with `PIPE_REJECT_REMOTE_CLIENTS`; a
//!   name that exists already makes it draw another secret, a few times. One instance always
//!   waits for the next client, so the name stays held while the listener lives. `close`
//!   removes the name file and releases the lock.
//! - `accept` identifies the client with `GetNamedPipeClientProcessId`, the user SID of that
//!   process's token and its image path; `connect` identifies the server the same way with
//!   `GetNamedPipeServerProcessId`, before anything is written. Both decide with
//!   [`PeerPolicy::check`]. A rejected client is disconnected before `accept` reports it.
//!   Clients open the pipe with `SECURITY_IDENTIFICATION`, so a server can never act as them.
//! - Under [`PeerPolicy::InstalledBinary`] the peer is trusted only when it is a member of the
//!   same installation as this process, as `iamb_platform::installation` decides (the 2.8
//!   review's finding 11, and its strict review): the process is opened and kept open while it
//!   is examined, must have been created before it connected (a process id reused since is
//!   refused), and its image path (`QueryFullProcessImageNameW`), with every link resolved,
//!   must be one of Iamb's executables in the canonical per-user installation directory
//!   (`SHGetKnownFolderPath(FOLDERID_LocalAppData)\Iamb`) this process runs from. When this
//!   process is signed (read once at the start, [`read_own_signing`], or compiled in,
//!   `IAMB_SIGNER_THUMBPRINT`), the peer's file must verify with the same certificate
//!   thumbprint (`WinVerifyTrust` with revocation checks off and URL retrieval from the cache
//!   only, so nothing touches the network); an unsigned installation requires the member's own
//!   file (volume serial number and file index); when it cannot be told, no peer is trusted.
//!   What is verified is the file, not the image the process was mapped from, which Windows
//!   does not let another process ask for. Debug builds use [`PeerPolicy::SameUser`].
//! - Streams use overlapped I/O, so a read blocked on one thread does not hold up a write on a
//!   clone; a read timeout cancels the blocked read, and `shutdown` cancels every pending
//!   operation on the handle (and disconnects, on the server side).

use std::ffi::c_void;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iamb_platform::installation::{self, OwnState, PathRules, PeerSignature};
use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy,
    PeerRejection, PlatformError, Result,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    FILETIME, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::Cryptography::{
    CERT_SHA1_HASH_PROP_ID, CertGetCertificateContextProperty,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetProcessTimes, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};

use crate::Platform;
use crate::pipe_names::{
    BIND_ATTEMPTS, lock_file, name_file, parse_secret, pipe_name, secret_from,
};

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

/// A new secret for the pipe's name: 16 bytes from the system's preferred generator.
fn new_secret() -> Result<String> {
    let mut bytes = [0_u8; 16];
    // SAFETY: a null algorithm handle with `BCRYPT_USE_SYSTEM_PREFERRED_RNG` is allowed; the
    // buffer is writable for the length passed.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(secret_from(bytes))
    } else {
        Err(PlatformError::os(i64::from(status), "BCryptGenRandom"))
    }
}

fn io_error(e: &io::Error, context: &'static str) -> PlatformError {
    PlatformError::os(e.raw_os_error().map_or(0, i64::from), context)
}

/// Writes `secret` to the name file, whole or not at all: a client reads either the old file
/// or the new one.
fn publish(name: &EndpointName, secret: &str) -> Result<()> {
    let file = name_file(name);
    let mut temporary = file.clone().into_os_string();
    temporary.push(".new");
    std::fs::write(&temporary, format!("{secret}\n"))
        .map_err(|e| io_error(&e, "writing the pipe's name file"))?;
    std::fs::rename(&temporary, &file).map_err(|e| io_error(&e, "renaming the pipe's name file"))
}

/// The pipe's full name as the name file gives it, for a client.
fn published_name(name: &EndpointName, sid: &str) -> Result<String> {
    let text = match std::fs::read_to_string(name_file(name)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(PlatformError::EndpointNotFound);
        }
        Err(e) => return Err(io_error(&e, "reading the pipe's name file")),
    };
    let secret = parse_secret(&text).ok_or(PlatformError::EndpointNotFound)?;
    Ok(pipe_name(name.pipe_stem(), sid, secret))
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

/// The string SID of the user this process runs as.
pub(crate) fn own_sid() -> Result<String> {
    // SAFETY: the pseudo handle of the current process is always valid and needs no closing.
    token_user(unsafe { GetCurrentProcess() })
}

/// Process `pid`, opened, with its user and image path. The handle is kept while the process
/// is examined, so that its id cannot be reused meanwhile (the strict review's finding 4).
fn process_identity(pid: u32) -> Result<(Handle, String, Option<PathBuf>)> {
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
    Ok((process, user, exe))
}

/// The time now, as a `FILETIME` value (100 ns since 1601).
fn now_filetime() -> u64 {
    let mut t = FILETIME::default();
    // SAFETY: `t` is a valid out-pointer.
    unsafe { GetSystemTimeAsFileTime(&raw mut t) };
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// When `process` was created, as a `FILETIME` value.
fn created_at(process: &Handle) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: `process` is open with query rights; the four out-pointers are valid.
    let ok = unsafe {
        GetProcessTimes(
            process.0,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    (ok != 0).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// Verifies the Authenticode signature of `path`: what `WinVerifyTrust` answered, and the
/// thumbprint of the signer's leaf certificate when it verified. No revocation checks and no
/// URL retrieval beyond the local cache: this never touches the network.
fn signer_of(path: &Path) -> (i32, Option<String>) {
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
    let signer = if status == 0 {
        leaf_thumbprint(data.hWVTStateData)
    } else {
        None
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
    (status, signer)
}

/// The thumbprint (SHA-1, upper-case hexadecimal) of the leaf certificate of the first signer
/// in a verification's open `state`.
fn leaf_thumbprint(state: HANDLE) -> Option<String> {
    // SAFETY: `state` is the state a successful `WinVerifyTrust` left open; the caller closes
    // it after this returns, and nothing read here outlives it but the copied bytes.
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return None;
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut hash = [0_u8; 20];
        let mut len = u32::try_from(hash.len()).ok()?;
        if CertGetCertificateContextProperty(
            cert,
            CERT_SHA1_HASH_PROP_ID,
            hash.as_mut_ptr().cast::<c_void>(),
            &raw mut len,
        ) == 0
            || usize::try_from(len).ok()? != hash.len()
        {
            return None;
        }
        let mut text = String::with_capacity(40);
        for b in hash {
            use std::fmt::Write as _;
            let _ = write!(text, "{b:02X}");
        }
        Some(text)
    }
}

/// The certificate thumbprint the release pipeline compiled in for a signed build
/// (`IAMB_SIGNER_THUMBPRINT`, `.github/workflows/installers.yml`); absent or empty for every
/// other build.
const PINNED_THUMBPRINT: Option<&str> = option_env!("IAMB_SIGNER_THUMBPRINT");

/// This process: its canonical image, the canonical per-user installation directory and its
/// signing state (`iamb_platform::installation`).
struct Own {
    exe: Option<PathBuf>,
    install_dir: Option<PathBuf>,
    state: OwnState,
}

/// [`Own`], read once: by [`read_own_signing`] at the start of `main`, or at the first peer
/// check of a process that did not call it.
fn own() -> &'static Own {
    static OWN: std::sync::OnceLock<Own> = std::sync::OnceLock::new();
    OWN.get_or_init(|| {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|e| std::fs::canonicalize(e).ok());
        let state = installation::pinned(
            PINNED_THUMBPRINT,
            installation::windows_thumbprint,
            || match &exe {
                Some(exe) => {
                    let (status, thumbprint) = signer_of(exe);
                    installation::classify_windows(status, thumbprint.as_deref())
                }
                None => OwnState::Unknown("this process's image could not be found"),
            },
        );
        Own {
            exe,
            install_dir: install_dir(),
            state,
        }
    })
}

/// Reads this process's signing state now and keeps it for the life of the process; returns
/// the line for the log. Call it first in `main` (the strict review's finding 1): read later,
/// the file it is judged by may have been replaced.
#[must_use]
pub fn read_own_signing() -> String {
    own().state.describe()
}

/// The canonical per-user installation directory: the `LocalAppData` known folder as the system
/// gives it (not `%LOCALAPPDATA%`, which whoever starts a process chooses), `Iamb` under it,
/// with every junction, 8.3 name and redirection resolved; `None` when it does not exist.
fn install_dir() -> Option<PathBuf> {
    let folder_id = FOLDERID_LocalAppData;
    let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: the folder id is a constant; a null token means the current user; `text`
    // receives a CoTaskMemAlloc'd, NUL-terminated string, freed below.
    let hr = unsafe {
        SHGetKnownFolderPath(&raw const folder_id, 0, std::ptr::null_mut(), &raw mut text)
    };
    let folder = if hr == 0 && !text.is_null() {
        let mut n = 0;
        // SAFETY: the string is NUL-terminated; we read up to the terminator.
        while unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: `text` points to `n` valid UTF-16 units.
        let units = unsafe { std::slice::from_raw_parts(text, n) };
        Some(PathBuf::from(String::from_utf16_lossy(units)))
    } else {
        None
    };
    // SAFETY: freeing what the call allocated (null is allowed).
    unsafe { CoTaskMemFree(text.cast::<c_void>()) };
    std::fs::canonicalize(folder?.join(installation::WINDOWS_INSTALL_DIR)).ok()
}

/// The volume serial number and file index of the file at `path` (its link resolved).
fn file_id(path: &Path) -> Option<(u32, u32, u32)> {
    let name = wide(&path.to_string_lossy());
    // SAFETY: `name` is NUL-terminated; no access is asked for, only the file's information;
    // backup semantics let a directory be opened too.
    let file = Handle::new(unsafe {
        CreateFileW(
            name.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    })?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` is open; `info` is a valid out-pointer.
    if unsafe { GetFileInformationByHandle(file.0, &raw mut info) } == 0 {
        return None;
    }
    Some((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

/// Whether the peer whose image the system reports as `reported` is a member of this
/// process's installation (module docs; `iamb_platform::installation`).
fn installed_peer(reported: &Path) -> bool {
    let own = own();
    let Some(members) = own
        .exe
        .as_deref()
        .and_then(|exe| installation::windows_members(exe, own.install_dir.as_deref()))
    else {
        return false;
    };
    let canonical = std::fs::canonicalize(reported).ok();
    let Some(member) = installation::member_for(&members, canonical.as_deref(), PathRules::Windows)
    else {
        return false;
    };
    let Some(canonical) = canonical else {
        return false;
    };
    let signature = match own.state.signer() {
        Some(signer) if signer_of(&canonical).1.as_deref() == Some(signer) => {
            PeerSignature::SameSigner
        }
        _ => PeerSignature::Other,
    };
    let same = file_id(&canonical).is_some_and(|id| file_id(&member.exe) == Some(id));
    installation::trusted(true, own.state.signing(), signature, true, same)
}

/// Identifies process `pid` at the other end, which connected at `connected` (a `FILETIME`
/// value), and checks it against `policy` for a process of `own_user`. A process created after
/// the connection took the id of the one that connected (`installation::created_before`).
fn verify(pid: u32, own_user: &str, policy: PeerPolicy, connected: u64) -> Result<PeerIdentity> {
    let (process, user, executable) = process_identity(pid)?;
    if !created_at(&process).is_some_and(|c| installation::created_before(c, connected)) {
        return Err(PlatformError::PeerRejected(PeerRejection::Unidentified));
    }
    let trusted_binary =
        policy == PeerPolicy::InstalledBinary && executable.as_deref().is_some_and(installed_peer);
    // The process stays open until it has been examined.
    drop(process);
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

/// Creates an instance of the pipe `name` (NUL-terminated) with the descriptor `sd`; with
/// `first`, only as the first instance of that name, [`EndpointInUse`] when the name exists.
///
/// [`EndpointInUse`]: PlatformError::EndpointInUse
fn instance(name: &[u16], sd: &Local, first: bool) -> Result<Handle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: sd.0,
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
            name.as_ptr(),
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

struct Listener {
    /// The endpoint, for the name file.
    endpoint: EndpointName,
    name: Vec<u16>,
    sd: Local,
    own_user: String,
    policy: PeerPolicy,
    /// The instance that waits for the next client.
    pending: Mutex<Option<Handle>>,
    close_event: Handle,
    closed: AtomicBool,
    /// The exclusive lock on `<run>/<kind>.pipe.lock`, released by `close`.
    lock: Mutex<Option<File>>,
}

impl Listener {
    fn instance(&self, first: bool) -> Result<Handle> {
        instance(&self.name, &self.sd, first)
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
            let connected = now_filetime();
            let mut pid = 0;
            // SAFETY: `pipe` is a connected server instance; `pid` is an out-pointer.
            let known = unsafe { GetNamedPipeClientProcessId(pipe.0, &raw mut pid) } != 0;
            let verdict = if known {
                verify(pid, &self.own_user, self.policy, connected)
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
            // The name is no longer served; the next server writes its own. Removed before the
            // lock is released, so it never removes a newer server's file.
            let _ = std::fs::remove_file(name_file(&self.endpoint));
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

/// Takes the exclusive lock on the endpoint's lock file, or `EndpointInUse` when another
/// server holds it.
fn take_lock(name: &EndpointName) -> Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_file(name))
        .map_err(|e| io_error(&e, "opening the pipe's lock file"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(PlatformError::EndpointInUse),
        Err(TryLockError::Error(e)) => Err(io_error(&e, "locking the pipe's lock file")),
    }
}

/// `bind`, with the secrets drawn from `secrets` (the system's generator; a test's list).
pub(crate) fn bind_with(
    name: &EndpointName,
    policy: PeerPolicy,
    mut secrets: impl FnMut() -> Result<String>,
) -> Result<Box<dyn LocalListener>> {
    let own_user = own_sid()?;
    std::fs::create_dir_all(name.run_dir())
        .map_err(|e| io_error(&e, "creating the run directory"))?;
    crate::restrict_to_current_user(name.run_dir())
        .map_err(|e| io_error(&e, "restricting the run directory to the user"))?;
    let lock = take_lock(name)?;
    let sd = security_descriptor(&own_user)?;
    for _ in 0..BIND_ATTEMPTS {
        let secret = secrets()?;
        let full = wide(&pipe_name(name.pipe_stem(), &own_user, &secret));
        let first = match instance(&full, &sd, true) {
            Ok(first) => first,
            // Someone holds this very name: draw another.
            Err(PlatformError::EndpointInUse) => continue,
            Err(e) => return Err(e),
        };
        publish(name, &secret)?;
        return Ok(Box::new(Listener {
            endpoint: name.clone(),
            name: full,
            sd,
            own_user,
            policy,
            pending: Mutex::new(Some(first)),
            close_event: event()?,
            closed: AtomicBool::new(false),
            lock: Mutex::new(Some(lock)),
        }));
    }
    Err(PlatformError::EndpointInUse)
}

impl LocalEndpoint for Platform {
    fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        let State {} = &self.endpoint;
        bind_with(name, policy, new_secret)
    }

    fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        let own_user = own_sid()?;
        let full = wide(&published_name(name, &own_user)?);
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
        let connected = now_filetime();
        let mut pid = 0;
        // SAFETY: `handle` is a client end of a pipe; `pid` is an out-pointer.
        if unsafe { GetNamedPipeServerProcessId(handle.0, &raw mut pid) } == 0 {
            return Err(PlatformError::PeerRejected(PeerRejection::Unidentified));
        }
        let peer = verify(pid, &own_user, policy, connected)?;
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

    fn temp_name(tag: &str) -> EndpointName {
        let dir = std::env::temp_dir().join(format!("iamb-pipe-unit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        EndpointName::new(EndpointKind::Service, dir.join("run"))
    }

    /// Finding 12: a name somebody else created first does not keep the server from serving;
    /// it draws another secret, and clients find that one through the name file.
    #[test]
    fn a_taken_name_makes_bind_draw_another_secret() {
        let n = temp_name("taken");
        let sid = own_sid().expect("own SID");
        let taken = "1".repeat(32);
        let free = "2".repeat(32);
        // The squatter: the first instance of the name the first secret gives.
        let sd = security_descriptor(&sid).expect("descriptor");
        let squatter = instance(&wide(&pipe_name(n.pipe_stem(), &sid, &taken)), &sd, true)
            .expect("the squatter creates the name");
        let mut secrets = vec![free.clone(), taken.clone()];
        let listener = bind_with(&n, PeerPolicy::SameUser, || {
            Ok(secrets.pop().expect("a secret"))
        })
        .expect("binds under another name");
        assert_eq!(
            std::fs::read_to_string(name_file(&n)).expect("name file"),
            format!("{free}\n")
        );
        assert_eq!(
            published_name(&n, &sid).expect("published"),
            pipe_name(n.pipe_stem(), &sid, &free)
        );
        // A client reaches the server, not the squatter.
        let accepted = {
            let listener: Arc<dyn LocalListener> = Arc::from(listener);
            let server = Arc::clone(&listener);
            let t = std::thread::spawn(move || server.accept().map(|s| s.peer().pid));
            let client = Platform::new()
                .connect(&n, PeerPolicy::SameUser, Duration::from_secs(5))
                .expect("connects");
            assert_eq!(client.peer().pid, std::process::id());
            let pid = t.join().expect("thread").expect("accepted");
            listener.close().expect("closes");
            pid
        };
        assert_eq!(accepted, std::process::id());
        // Closed: the name file is gone, so a client finds nothing.
        assert!(!name_file(&n).exists());
        assert!(matches!(
            Platform::new().connect(&n, PeerPolicy::SameUser, Duration::from_secs(1)),
            Err(PlatformError::EndpointNotFound)
        ));
        drop(squatter);
        let _ = std::fs::remove_dir_all(n.run_dir().parent().expect("root"));
    }

    #[test]
    fn a_name_that_stays_taken_is_reported_after_a_few_secrets() {
        let n = temp_name("always");
        let sid = own_sid().expect("own SID");
        let sd = security_descriptor(&sid).expect("descriptor");
        let taken = "3".repeat(32);
        let _squatter = instance(&wide(&pipe_name(n.pipe_stem(), &sid, &taken)), &sd, true)
            .expect("the squatter creates the name");
        let mut drawn = 0;
        let result = bind_with(&n, PeerPolicy::SameUser, || {
            drawn += 1;
            Ok(taken.clone())
        });
        assert!(matches!(result, Err(PlatformError::EndpointInUse)));
        assert_eq!(drawn, BIND_ATTEMPTS);
        assert!(!name_file(&n).exists(), "nothing published");
        let _ = std::fs::remove_dir_all(n.run_dir().parent().expect("root"));
    }

    #[test]
    fn the_run_directory_is_the_users_alone_and_secrets_are_random() {
        let n = temp_name("private");
        let sid = own_sid().expect("own SID");
        let first = Platform::new()
            .bind(&n, PeerPolicy::SameUser)
            .expect("binds");
        let sddl = crate::files::dacl_sddl(n.run_dir()).expect("readable");
        assert!(crate::files::is_users_only(&sddl, &sid), "{sddl}");
        let one = std::fs::read_to_string(name_file(&n)).expect("name file");
        assert!(parse_secret(&one).is_some(), "{one:?}");
        first.close().expect("closes");
        drop(first);
        let second = Platform::new()
            .bind(&n, PeerPolicy::SameUser)
            .expect("binds again");
        let two = std::fs::read_to_string(name_file(&n)).expect("name file");
        assert_ne!(one, two, "every bind draws a new secret");
        second.close().expect("closes");
        assert_ne!(new_secret().expect("random"), new_secret().expect("random"));
        let _ = std::fs::remove_dir_all(n.run_dir().parent().expect("root"));
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
        let me = verify(
            std::process::id(),
            &own,
            PeerPolicy::SameUser,
            now_filetime(),
        )
        .expect("self");
        assert_eq!(me.user, own);
        assert!(me.executable.is_some());
    }

    #[test]
    fn a_test_binary_is_in_no_installation() {
        // `target\` is not `%LOCALAPPDATA%\Iamb`: the policy trusts no peer for a test binary
        // (the decision itself is tested in iamb_platform::installation).
        let own = std::env::current_exe().expect("test binary");
        assert_eq!(
            installation::windows_members(&own, install_dir().as_deref()),
            None
        );
        assert!(!installed_peer(&own));
        // A test binary is not signed: "unsigned", never "signed" from an error.
        assert!(
            !read_own_signing().starts_with("signed"),
            "{}",
            read_own_signing()
        );
    }

    #[test]
    fn an_unsigned_test_binary_does_not_verify() {
        let exe = std::env::current_exe().expect("test binary");
        assert_eq!(signer_of(&exe).1, None);
    }
}
