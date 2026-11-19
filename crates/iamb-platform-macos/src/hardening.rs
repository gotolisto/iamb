//! Keeping the process's memory out of core files (architecture §8 rule 9).
//!
//! [`harden_process`] sets the core-file limit (`RLIMIT_CORE`) to 0, soft and hard, so that a
//! crash of this process or of a child it starts never writes its memory to `/cores`, whatever
//! limit the shell or `launchd` passed on. Child processes inherit the limit and cannot raise
//! it again. The service calls it first thing in `main`, in the supervisor and in the worker.
//!
//! What it does not change: the system's crash reporter still writes a report to
//! `~/Library/Logs/DiagnosticReports` when the process crashes. Such a report holds the
//! threads' backtraces and registers and the list of loaded images, not heap memory; a
//! register can hold the character being handled at that moment. The service's own panics
//! never get there: its panic hook exits with a status instead of aborting
//! (`iamb_service::crash`).

use iamb_platform::{PlatformError, Result};

/// Sets the core-file limit to 0 (module docs). Returns each step with its result; call it
/// before any other thread starts.
#[must_use]
pub fn harden_process() -> Vec<(&'static str, Result<()>)> {
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `setrlimit` reads the `rlimit` value the pointer refers to, which lives on this
    // stack frame for the whole call; lowering a limit needs no privilege.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const none) };
    let result = if rc == 0 {
        Ok(())
    } else {
        Err(PlatformError::os(
            i64::from(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)),
            "setrlimit(RLIMIT_CORE)",
        ))
    };
    vec![("core-limit-0", result)]
}

/// The current core-file limit, soft and hard.
#[cfg(test)]
fn core_limit() -> (libc::rlim_t, libc::rlim_t) {
    let mut now = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    // SAFETY: `getrlimit` writes one `rlimit` through the pointer, which refers to a local
    // that outlives the call.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_CORE, &raw mut now) };
    assert_eq!(rc, 0, "getrlimit");
    (now.rlim_cur, now.rlim_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_limit_is_zero_afterwards() {
        let steps = harden_process();
        assert_eq!(steps.len(), 1);
        assert!(steps.iter().all(|(_, r)| r.is_ok()), "{steps:?}");
        assert_eq!(core_limit(), (0, 0));
        // Idempotent: a second call keeps it there.
        assert!(harden_process().iter().all(|(_, r)| r.is_ok()));
        assert_eq!(core_limit(), (0, 0));
    }
}
