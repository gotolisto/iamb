//! The account's home directory from the user database (the beta-gap review's re-verification,
//! finding 1): the released helper builds the default data root from it, not from `HOME`, which
//! whoever starts the helper chooses (`open --env HOME=…`).

use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// The home directory `getpwuid_r` gives for this process's effective user, if it is an absolute
/// path.
#[must_use]
pub fn account_home() -> Option<PathBuf> {
    let mut size = 1024_usize;
    loop {
        let mut buf: Vec<libc::c_char> = vec![0; size];
        // SAFETY: `passwd` is plain data; an all-zero value is valid for it to be written into.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `entry`, `buf` (of `buf.len()` bytes) and `result` are valid for writes for the
        // duration of the call; `geteuid` has no preconditions.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                &raw mut entry,
                buf.as_mut_ptr(),
                buf.len(),
                &raw mut result,
            )
        };
        if rc == libc::ERANGE && size < 1 << 16 {
            size *= 4;
            continue;
        }
        if rc != 0 || result.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        // SAFETY: on success `pw_dir` points to a NUL-terminated string inside `buf`, which is
        // alive here.
        let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
        let path = PathBuf::from(OsStr::from_bytes(dir.to_bytes()));
        return path.has_root().then_some(path);
    }
}
