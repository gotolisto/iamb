//! Directories for the current user only (the 2.8 review's finding 20; architecture §3.3).
//!
//! The default data root, `%APPDATA%\Iamb`, inherits the user profile's permissions, which
//! admit the user, the system and administrators only. A data root chosen with `--data-dir`
//! or `IAMB_DATA_DIR` inherits whatever its parent has: under `C:\Iamb` or on a shared drive
//! every user of the machine could read the library, the logs and the pipe's secret. So the
//! service gives the data root a protected DACL with one entry, full control for the current
//! user's SID, inherited by everything inside (`D:P(A;OICI;FA;;;<SID>)`), the same rule as the
//! pipe's own security descriptor. "Protected" stops the parent's entries from being inherited.
//! The endpoint does the same for the `run` directory it keeps the pipe's secret in.
//!
//! Setting a DACL on a directory that holds files makes the system walk them all to pass the
//! inherited entry on, so a directory that already has exactly this DACL is left alone.

use std::ffi::c_void;
use std::io;
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};

use crate::endpoint::own_sid;

/// Memory the system allocated with `LocalAlloc`, freed when dropped.
struct LocalMemory(*mut c_void);

impl Drop for LocalMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a call that allocates with `LocalAlloc`, and is
            // freed once.
            unsafe { LocalFree(self.0) };
        }
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn win32(code: u32) -> io::Error {
    io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(-1))
}

/// The DACL for `sid` alone, as SDDL.
fn wanted(sid: &str) -> String {
    format!("D:P(A;OICI;FA;;;{sid})")
}

/// True when `sddl`, the DACL of a directory as the system writes it, is the one [`wanted`]
/// for `sid`: protected (`P`, with `AI` when the system marked it auto-inherited), with that
/// one entry. The system writes some well-known accounts as a two-letter alias instead of the
/// SID (the built-in Administrator, which CI runners use, is `LA`), so the entry's trustee is
/// resolved before it is compared.
pub(crate) fn is_users_only(sddl: &str, sid: &str) -> bool {
    is_users_only_by(sddl, |trustee| {
        trustee == sid || trustee_sid(trustee).is_some_and(|t| t == sid)
    })
}

/// [`is_users_only`] with the comparison of the entry's trustee given.
fn is_users_only_by(sddl: &str, is_user: impl Fn(&str) -> bool) -> bool {
    sddl.strip_prefix("D:P")
        .map(|rest| rest.strip_prefix("AI").unwrap_or(rest))
        .and_then(|rest| rest.strip_prefix("(A;OICI;FA;;;"))
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|trustee| !trustee.contains(['(', ')', ';']) && is_user(trustee))
}

/// The SID string an SDDL trustee names: an SDDL alias such as `LA` resolved through the
/// system, a SID string as it is.
fn trustee_sid(trustee: &str) -> Option<String> {
    if trustee.starts_with("S-") {
        return Some(trustee.to_owned());
    }
    let wide: Vec<u16> = trustee.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sid = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated; `sid` receives a SID allocated with `LocalAlloc`,
    // freed by `_sid` below.
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &raw mut sid) } == 0 {
        return None;
    }
    let _sid = LocalMemory(sid);
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` is the valid SID made above; `text` receives a NUL-terminated string
    // allocated with `LocalAlloc`, freed by `_text` below.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return None;
    }
    let _text = LocalMemory(text.cast::<c_void>());
    let mut len = 0;
    // SAFETY: the string is NUL-terminated; this reads up to the terminator.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `text` points to `len` valid UTF-16 units.
    Some(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(text, len)
    }))
}

/// The DACL of `path` as SDDL (`D:…`).
///
/// # Errors
///
/// The system's error when the path's security cannot be read.
pub(crate) fn dacl_sddl(path: &Path) -> io::Result<String> {
    let name = wide_path(path);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; the owner, group and ACL out-pointers may be null;
    // `sd` receives a descriptor allocated with `LocalAlloc`, freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut sd,
        )
    };
    if code != ERROR_SUCCESS {
        return Err(win32(code));
    }
    let _sd = LocalMemory(sd);
    let mut text: *mut u16 = std::ptr::null_mut();
    let mut len = 0_u32;
    // SAFETY: `sd` is the valid descriptor read above; `text` receives a NUL-terminated string
    // allocated with `LocalAlloc` and `len` its length in units without the NUL.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &raw mut text,
            &raw mut len,
        )
    };
    if ok == 0 || text.is_null() {
        return Err(io::Error::last_os_error());
    }
    let _text = LocalMemory(text.cast::<c_void>());
    // SAFETY: the call wrote `len` units at `text`.
    let units = unsafe { std::slice::from_raw_parts(text, usize::try_from(len).unwrap_or(0)) };
    Ok(String::from_utf16_lossy(units)
        .trim_end_matches('\0')
        .to_owned())
}

/// Gives the directory `dir` a protected DACL that admits the current user only, inherited by
/// everything inside it (module docs). Leaves a directory that already has it alone.
///
/// # Errors
///
/// The system's error when the user's SID or the directory's security cannot be read or set.
pub fn restrict_to_current_user(dir: &Path) -> io::Result<()> {
    let sid = own_sid().map_err(|e| io::Error::other(e.to_string()))?;
    if dacl_sddl(dir).is_ok_and(|sddl| is_users_only(&sddl, &sid)) {
        return Ok(());
    }
    let sddl: Vec<u16> = wanted(&sid)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` receives a descriptor allocated with
    // `LocalAlloc`, freed below; the size out-pointer may be null.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let _sd = LocalMemory(sd);
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `sd` is the valid descriptor made above; the out-pointers are valid. The DACL
    // points into `sd`, which lives until the end of this function.
    let ok = unsafe {
        GetSecurityDescriptorDacl(sd, &raw mut present, &raw mut dacl, &raw mut defaulted)
    };
    if ok == 0 || present == 0 || dacl.is_null() {
        return Err(io::Error::last_os_error());
    }
    let name = wide_path(dir);
    // SAFETY: `name` is NUL-terminated; only the DACL is set (owner, group and SACL null), and
    // `dacl` is valid for the call.
    let code = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            std::ptr::null(),
        )
    };
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(win32(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wanted_dacl_is_recognised_as_the_system_writes_it() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert!(is_users_only(&wanted(sid), sid));
        assert!(is_users_only(&format!("D:PAI(A;OICI;FA;;;{sid})"), sid));
        assert!(!is_users_only(&format!("D:AI(A;OICI;FA;;;{sid})"), sid));
        assert!(!is_users_only(
            &format!("D:PAI(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)"),
            sid
        ));
        assert!(!is_users_only("D:PAI(A;OICI;FA;;;S-1-5-21-9)", sid));
    }

    /// The system writes the built-in Administrator, the account CI runners use, as `LA`.
    #[test]
    fn an_alias_the_system_writes_for_the_users_sid_is_recognised() {
        let is_admin = |t: &str| t == "LA";
        assert!(is_users_only_by("D:PAI(A;OICI;FA;;;LA)", is_admin));
        assert!(!is_users_only_by("D:PAI(A;OICI;FA;;;BA)", is_admin));
        assert!(!is_users_only_by(
            "D:PAI(A;OICI;FA;;;LA)(A;OICI;FA;;;SY)",
            is_admin
        ));
        // `LA` resolves to a SID of the local accounts domain ending in 500.
        let la = trustee_sid("LA").expect("the system resolves LA");
        assert!(la.starts_with("S-1-5-21-") && la.ends_with("-500"), "{la}");
        assert_eq!(trustee_sid("S-1-5-18").as_deref(), Some("S-1-5-18"));
    }

    #[test]
    fn a_directory_and_what_is_made_in_it_admit_the_user_only() {
        let sid = own_sid().expect("own SID");
        let dir = std::env::temp_dir().join(format!("iamb-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("before")).expect("created");
        std::fs::write(dir.join("before").join("file.txt"), b"x").expect("written");
        // The temporary directory's own DACL is inherited: not the user's alone.
        let inherited = dacl_sddl(&dir).expect("readable");
        assert!(!is_users_only(&inherited, &sid), "{inherited}");

        restrict_to_current_user(&dir).expect("restricted");
        let own = dacl_sddl(&dir).expect("readable");
        assert!(is_users_only(&own, &sid), "{own}");
        // What was there before, and what comes after, inherits the one entry and nothing
        // from above.
        std::fs::create_dir(dir.join("after")).expect("created");
        for inner in [
            dir.join("before"),
            dir.join("before").join("file.txt"),
            dir.join("after"),
        ] {
            let sddl = dacl_sddl(&inner).expect("readable");
            // Every entry names the user (the system may write the SID as an alias such as
            // `LA`), and there is at least one.
            let trustees: Vec<&str> = sddl
                .split(";;;")
                .skip(1)
                .filter_map(|rest| rest.split(')').next())
                .collect();
            assert!(!trustees.is_empty(), "{sddl}");
            for trustee in trustees {
                assert_eq!(
                    trustee_sid(trustee).as_deref(),
                    Some(sid.as_str()),
                    "{sddl}"
                );
            }
        }
        // Again: nothing to do, and no error.
        restrict_to_current_user(&dir).expect("idempotent");
        assert_eq!(dacl_sddl(&dir).expect("readable"), own);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
