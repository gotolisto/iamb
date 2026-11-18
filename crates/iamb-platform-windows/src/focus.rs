//! The focused application, window and field. **Task 2.5** fills this file.
//!
//! What it must do (architecture §3.1, §4.1, §4.5, §5; ADR-0012; the trait docs of
//! [`FocusInspector`]):
//!
//! - `focused_app`: the foreground window's process; the application name is the
//!   executable's file name, never the window title. `own` is true when the executable lives
//!   in the installation directory of the running one.
//! - `watch`: report every change of the foreground window, and every change of the focused
//!   element inside it, from a thread this module owns.
//! - `secure_state`: is the focused element of the target window a password field? The
//!   query goes through UI Automation on a dedicated thread, so a hung application cannot
//!   hold the caller: after `timeout` the answer is `Unknown(Timeout)`. A window running at
//!   a higher integrity level than Iamb is `Unknown(Elevated)`. When the target is not the
//!   foreground window before or after the query, the answer is `Unknown(FocusMoved)`. Fail
//!   closed on every error.
//! - `focus_token`: the process id and the window handle. `restore_focus`: bring that window
//!   to the foreground and return only once it is the foreground window
//!   ([`RESTORE_FOCUS_TIMEOUT`](iamb_platform::RESTORE_FOCUS_TIMEOUT)).
//! - `allow_foreground`: let the UI host's process take the foreground (§4.5 step 3).

use std::time::Duration;

use iamb_platform::{
    AppIdentity, FieldSecurity, FocusInspector, FocusSink, FocusToken, PlatformError, Result,
    UnknownReason,
};

use crate::Platform;

/// The focus state: the watcher and the inspection thread.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl FocusInspector for Platform {
    fn focused_app(&self) -> Result<AppIdentity> {
        let State {} = &self.focus;
        Err(PlatformError::Unsupported)
    }

    fn secure_state(&self, _target: &FocusToken, _timeout: Duration) -> FieldSecurity {
        // Not implemented: fail closed.
        FieldSecurity::Unknown(UnknownReason::NotInspectable)
    }

    fn focus_token(&self) -> Result<FocusToken> {
        Err(PlatformError::Unsupported)
    }

    fn restore_focus(&self, _token: &FocusToken) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn allow_foreground(&self, _pid: u32) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn watch(&self, _sink: Box<dyn FocusSink>) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn unwatch(&self) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet_and_fails_closed() {
        let platform = Platform::new();
        assert_eq!(platform.focused_app(), Err(PlatformError::Unsupported));
        assert_eq!(
            platform.secure_state(&FocusToken::from_raw(1, 1), Duration::from_millis(100)),
            FieldSecurity::Unknown(UnknownReason::NotInspectable)
        );
        assert_eq!(platform.focus_token(), Err(PlatformError::Unsupported));
        assert_eq!(
            platform.restore_focus(&FocusToken::from_raw(1, 1)),
            Err(PlatformError::Unsupported)
        );
        assert_eq!(
            platform.allow_foreground(1),
            Err(PlatformError::Unsupported)
        );
        assert_eq!(
            platform.watch(Box::new(|_: iamb_platform::FocusChange| {})),
            Err(PlatformError::Unsupported)
        );
        assert_eq!(platform.unwatch(), Err(PlatformError::Unsupported));
    }
}
