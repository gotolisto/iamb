//! Start at login. **Task 2.6** fills this file (used by the UI host from task 3.2).
//!
//! What it must do (architecture §3.2, §5; ADR-0011, ADR-0021; the trait docs of
//! [`LoginItem`]):
//!
//! - `enable` registers the signed helper app inside `Iamb.app` (the supervisor) as a login
//!   item through the system's login-item service; `disable` unregisters it;
//!   `login_item_status` maps the service's status: enabled, not registered, requires
//!   approval in System Settings, or not found.
//! - It is the main app that registers the helper, so that the grants belong to the helper
//!   and survive updates.

use iamb_platform::{LoginItem, LoginItemStatus, PlatformError, Result};

use crate::Platform;

/// The login item's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl LoginItem for Platform {
    fn enable(&self) -> Result<()> {
        let State {} = &self.login_item;
        Err(PlatformError::Unsupported)
    }

    fn disable(&self) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn login_item_status(&self) -> Result<LoginItemStatus> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        assert_eq!(platform.enable(), Err(PlatformError::Unsupported));
        assert_eq!(platform.disable(), Err(PlatformError::Unsupported));
        assert_eq!(
            platform.login_item_status(),
            Err(PlatformError::Unsupported)
        );
    }
}
