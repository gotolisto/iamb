//! Keeping the process's memory out of Windows Error Reporting (architecture §8 rule 9).
//!
//! [`harden_process`] takes two steps, both for the calling process only and inherited by the
//! processes it starts:
//!
//! 1. `SetErrorMode` with `SEM_NOGPFAULTERRORBOX` (and `SEM_FAILCRITICALERRORS`) added to the
//!    current mode. Per the API documentation, with `SEM_NOGPFAULTERRORBOX` "the system does
//!    not invoke Windows Error Reporting", so an unhandled exception ends the process without a
//!    report or a dump, including the local dumps an administrator can configure for WER.
//!    `SEM_FAILCRITICALERRORS` keeps critical-error message boxes from blocking a background
//!    process (the documentation's recommendation for every application).
//! 2. `WerSetFlags(WER_FAULT_REPORTING_FLAG_NOHEAP)`: "do not collect heap information in the
//!    event of an application crash or non-response". A second layer: if WER still makes a
//!    report for the process (a hang report, or a termination that does not pass through the
//!    error mode, such as a fail-fast exit), the report holds no heap.
//!
//! What it does not change: a dump requested from outside the process (a debugger, Task
//! Manager's "Create dump file", a full-memory dump policy for all processes) still holds the
//! process's memory; architecture §8 rule 9 names that limit. The service's own panics never
//! reach WER: its panic hook exits with a status instead of aborting (`iamb_service::crash`).

use iamb_platform::{PlatformError, Result};
use windows_sys::Win32::Foundation::S_OK;
use windows_sys::Win32::System::Diagnostics::Debug::{
    GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::ErrorReporting::{WER_FAULT_REPORTING_FLAG_NOHEAP, WerSetFlags};

/// Opts the process out of error reporting and its heap collection (module docs). Returns
/// each step with its result; call it before any other thread starts (the error mode is
/// process-wide).
#[must_use]
pub fn harden_process() -> Vec<(&'static str, Result<()>)> {
    // SAFETY: `GetErrorMode` only reads the process's error mode.
    let mode = unsafe { GetErrorMode() };
    // SAFETY: `SetErrorMode` only sets the process's error mode; it keeps every bit already
    // set and adds two.
    unsafe { SetErrorMode(mode | SEM_NOGPFAULTERRORBOX | SEM_FAILCRITICALERRORS) };
    // SAFETY: as for the first call.
    let now = unsafe { GetErrorMode() };
    let error_mode = if now & SEM_NOGPFAULTERRORBOX != 0 {
        Ok(())
    } else {
        Err(PlatformError::os(i64::from(now), "SetErrorMode"))
    };
    // SAFETY: `WerSetFlags` only changes the error-reporting settings of the current process.
    let hr = unsafe { WerSetFlags(WER_FAULT_REPORTING_FLAG_NOHEAP) };
    let no_heap = if hr == S_OK {
        Ok(())
    } else {
        Err(PlatformError::os(i64::from(hr), "WerSetFlags"))
    };
    vec![
        ("error-reporting-off", error_mode),
        ("error-report-without-heap", no_heap),
    ]
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::ErrorReporting::WerGetFlags;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use super::*;

    #[test]
    fn error_reporting_is_off_and_without_heap_afterwards() {
        let steps = harden_process();
        assert_eq!(steps.len(), 2);
        assert!(steps.iter().all(|(_, r)| r.is_ok()), "{steps:?}");
        // SAFETY: as in `harden_process`.
        let mode = unsafe { GetErrorMode() };
        assert_ne!(mode & SEM_NOGPFAULTERRORBOX, 0);
        assert_ne!(mode & SEM_FAILCRITICALERRORS, 0);
        let mut flags = 0;
        // SAFETY: the pseudo handle of the current process is always valid; the out-pointer
        // refers to a local that outlives the call.
        let hr = unsafe { WerGetFlags(GetCurrentProcess(), &raw mut flags) };
        assert_eq!(hr, S_OK);
        assert_ne!(flags & WER_FAULT_REPORTING_FLAG_NOHEAP, 0);
    }
}
