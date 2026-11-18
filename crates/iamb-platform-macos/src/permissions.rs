//! Permissions. **Task 2.3** fills this file (with `listener.rs`).
//!
//! What it must do (architecture §5; ADR-0021; the trait docs of [`Permissions`]):
//!
//! - `permission_status` reports Accessibility (is this process a trusted Accessibility
//!   client?) and Input Monitoring (may this process listen to events?), without showing a
//!   prompt. It is always asked in the worker, the process that holds the grants; the UI
//!   host never checks its own.
//! - `open_settings` opens the matching page of System Settings, Privacy & Security. The
//!   first time, it also makes the system list Iamb there (the prompting variants of the
//!   two checks do that).
//! - A grant given while the worker runs must be seen without a restart: do not cache.

use iamb_platform::{Grant, PermissionKind, PermissionStatus, Permissions, PlatformError, Result};

use crate::Platform;

/// The permission checker's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl Permissions for Platform {
    fn permission_status(&self) -> PermissionStatus {
        let State {} = &self.permissions;
        // Not implemented: nothing is known to be granted.
        PermissionStatus {
            accessibility: Grant::Denied,
            input_monitoring: Grant::Denied,
        }
    }

    fn open_settings(&self, _kind: PermissionKind) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet_and_nothing_is_granted() {
        let platform = Platform::new();
        assert_eq!(platform.permission_status().missing(), PermissionKind::ALL);
        for kind in PermissionKind::ALL {
            assert_eq!(
                platform.open_settings(kind),
                Err(PlatformError::Unsupported)
            );
        }
    }
}
