//! Start at login. **Task 2.6** fills this file (used by the UI host from task 3.2).
//!
//! What it must do (architecture §3.2, §5; ADR-0011, ADR-0021; the trait docs of
//! [`LoginItem`]):
//!
//! - `enable` writes a value under the per-user Run key that starts the supervisor
//!   (`iamb-service --supervise`) with the full, quoted path of the installed executable;
//!   `disable` removes it; `login_item_status` reads it and reports `NotFound` when the
//!   executable it names is missing.
//! - No administrator rights and nothing machine-wide.

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
