//! The focused application, window and field. **Task 2.5** fills this file.
//!
//! What it must do (architecture §3.1, §4.1, §4.5, §5; ADR-0012, ADR-0021; the trait docs of
//! [`FocusInspector`]):
//!
//! - `focused_app`: the frontmost application; the application name is its bundle
//!   identifier (the executable name when it has none), never a window title. `own` is true
//!   for Iamb's own bundle identifiers.
//! - `watch`: report every activation of another application, and every change of the
//!   focused window or focused element inside it (Accessibility notifications), from a
//!   thread this module owns.
//! - `secure_state`: `Secure` when secure keyboard entry is on, or when the focused element
//!   is a secure text field. The Accessibility query runs on a dedicated thread with a
//!   messaging timeout, so a hung application cannot hold the caller: after `timeout` the
//!   answer is `Unknown(Timeout)`. Without Accessibility, or when the element cannot be
//!   read, the answer is `Unknown(NotInspectable)`. When the target window is not the
//!   focused one before or after the query, the answer is `Unknown(FocusMoved)`. Fail closed
//!   on every error.
//! - `focus_token`: the process id and the window id. `restore_focus`: activate that
//!   application, raise the window, and return only once it is focused
//!   ([`RESTORE_FOCUS_TIMEOUT`](iamb_platform::RESTORE_FOCUS_TIMEOUT)).
//! - `allow_foreground`: nothing to do. The form and the palette are non-activating panels
//!   (§4.5 step 3); return `Ok`.

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
