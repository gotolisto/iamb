//! Start at login through the per-user Run key (architecture §3.2, §5; ADR-0011, ADR-0021).
//! Used by the UI host from task 3.2.
//!
//! - `enable` writes the value `Iamb` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`:
//!   the full, quoted path of `iamb-service.exe` next to the running executable, then
//!   `--supervise`. `disable` removes the value; `login_item_status` reads it and reports
//!   `NotFound` when the executable it names is missing.
//! - No administrator rights and nothing machine-wide: only the current user's hive.

use std::path::{Path, PathBuf};

use iamb_platform::{LoginItem, LoginItemStatus, PlatformError, Result};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use crate::Platform;

/// The per-user Run key.
pub(crate) const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value's name under it.
pub(crate) const VALUE_NAME: &str = "Iamb";

/// The supervisor's file name.
const SERVICE_EXE: &str = "iamb-service.exe";

/// The login item's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The command line stored in the Run key.
pub(crate) fn command_line(exe: &Path) -> String {
    format!("\"{}\" --supervise", exe.display())
}

/// The executable a stored command line names: the quoted first part, or the text up to the
/// first space.
pub(crate) fn executable_of(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let path = if let Some(rest) = command.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        command.split(' ').next()?
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn os(code: WIN32_ERROR, context: &'static str) -> PlatformError {
    PlatformError::os(i64::from(code), context)
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `RegCreateKeyExW` or `RegOpenKeyExW` and is closed
        // once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn create(subkey: &str) -> Result<Key> {
    let name = wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `name` is a NUL-terminated wide string; `key` is a valid out-pointer; the other
    // pointers may be null.
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            name.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            std::ptr::null(),
            &raw mut key,
            std::ptr::null_mut(),
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(Key(key))
    } else {
        Err(os(rc, "RegCreateKeyExW"))
    }
}

fn open(subkey: &str) -> Result<Option<Key>> {
    let name = wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: as for `create`.
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            name.as_ptr(),
            0,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            &raw mut key,
        )
    };
    match rc {
        ERROR_SUCCESS => Ok(Some(Key(key))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        _ => Err(os(rc, "RegOpenKeyExW")),
    }
}

/// Writes the string value `name` under `HKCU\subkey`.
pub(crate) fn write_value(subkey: &str, name: &str, data: &str) -> Result<()> {
    let key = create(subkey)?;
    let name = wide(name);
    let data = wide(data);
    let bytes = u32::try_from(data.len() * 2).map_err(|_| PlatformError::Invalid {
        context: "the login item command line is too long",
    })?;
    // SAFETY: `key` is open for setting values; `name` is NUL-terminated; `data` holds `bytes`
    // bytes of a NUL-terminated wide string, as `REG_SZ` requires.
    let rc = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast::<u8>(),
            bytes,
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(os(rc, "RegSetValueExW"))
    }
}

/// Reads the string value `name` under `HKCU\subkey`, `None` when absent.
pub(crate) fn read_value(subkey: &str, name: &str) -> Result<Option<String>> {
    let Some(key) = open(subkey)? else {
        return Ok(None);
    };
    let name = wide(name);
    let mut buf = vec![0_u16; 2048];
    let mut bytes = u32::try_from(buf.len() * 2).unwrap_or(4096);
    let mut kind = 0;
    // SAFETY: `key` is open for queries; `name` is NUL-terminated; `buf` has `bytes` bytes of
    // room and `bytes` is updated with the length written.
    let rc = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &raw mut kind,
            buf.as_mut_ptr().cast::<u8>(),
            &raw mut bytes,
        )
    };
    match rc {
        ERROR_SUCCESS => {
            let units = usize::try_from(bytes).unwrap_or(0) / 2;
            buf.truncate(units.min(buf.len()));
            while buf.last() == Some(&0) {
                buf.pop();
            }
            Ok(Some(String::from_utf16_lossy(&buf)))
        }
        ERROR_FILE_NOT_FOUND => Ok(None),
        _ => Err(os(rc, "RegQueryValueExW")),
    }
}

/// Removes the value `name` under `HKCU\subkey`; absent is not an error.
pub(crate) fn delete_value(subkey: &str, name: &str) -> Result<()> {
    let Some(key) = open(subkey)? else {
        return Ok(());
    };
    let name = wide(name);
    // SAFETY: `key` is open for setting values; `name` is NUL-terminated.
    let rc = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
    match rc {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        _ => Err(os(rc, "RegDeleteValueW")),
    }
}

/// The status a stored command line means.
pub(crate) fn status_of(value: Option<&str>) -> LoginItemStatus {
    match value.map(executable_of) {
        None => LoginItemStatus::Disabled,
        Some(Some(exe)) if exe.is_file() => LoginItemStatus::Enabled,
        Some(_) => LoginItemStatus::NotFound,
    }
}

/// The supervisor next to the running executable.
fn service_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|e| {
        PlatformError::os(
            i64::from(e.raw_os_error().unwrap_or(-1)),
            "GetModuleFileNameW",
        )
    })?;
    let service = exe
        .parent()
        .map(|dir| dir.join(SERVICE_EXE))
        .ok_or(PlatformError::Invalid {
            context: "the running executable has no directory",
        })?;
    if service.is_file() {
        Ok(service)
    } else {
        Err(PlatformError::Invalid {
            context: "iamb-service.exe is not installed next to this executable",
        })
    }
}

impl LoginItem for Platform {
    fn enable(&self) -> Result<()> {
        let State {} = &self.login_item;
        write_value(RUN_KEY, VALUE_NAME, &command_line(&service_exe()?))
    }

    fn disable(&self) -> Result<()> {
        delete_value(RUN_KEY, VALUE_NAME)
    }

    fn login_item_status(&self) -> Result<LoginItemStatus> {
        Ok(status_of(read_value(RUN_KEY, VALUE_NAME)?.as_deref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_round_trip() {
        let exe = Path::new(r"C:\Users\Ada\AppData\Local\Programs\Iamb\iamb-service.exe");
        let line = command_line(exe);
        assert_eq!(
            line,
            r#""C:\Users\Ada\AppData\Local\Programs\Iamb\iamb-service.exe" --supervise"#
        );
        assert_eq!(executable_of(&line).as_deref(), Some(exe));
        assert_eq!(
            executable_of(r"C:\x\iamb-service.exe --supervise").as_deref(),
            Some(Path::new(r"C:\x\iamb-service.exe"))
        );
        assert_eq!(executable_of(""), None);
        assert_eq!(status_of(None), LoginItemStatus::Disabled);
        assert_eq!(
            status_of(Some(r#""C:\nowhere\iamb-service.exe" --supervise"#)),
            LoginItemStatus::NotFound
        );
    }

    #[test]
    fn values_are_written_read_and_removed_in_the_user_hive() {
        // A scratch key of this test run, never the real Run key.
        let key = format!(r"Software\IambTest\login-item-{}", std::process::id());
        let exe = std::env::current_exe().expect("test binary");
        write_value(&key, VALUE_NAME, &command_line(&exe)).expect("written");
        let stored = read_value(&key, VALUE_NAME).expect("read");
        assert_eq!(status_of(stored.as_deref()), LoginItemStatus::Enabled);
        delete_value(&key, VALUE_NAME).expect("deleted");
        delete_value(&key, VALUE_NAME).expect("deleting twice is fine");
        assert_eq!(read_value(&key, VALUE_NAME).expect("read"), None);
        assert_eq!(
            read_value(r"Software\IambTest\absent", VALUE_NAME).expect("read"),
            None
        );
    }

    #[test]
    fn enable_needs_the_installed_supervisor() {
        // Test binaries run from target\, where no iamb-service.exe sits next to them.
        if service_exe().is_err() {
            assert!(matches!(
                Platform::new().enable(),
                Err(PlatformError::Invalid { .. })
            ));
        }
    }
}
