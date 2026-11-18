//! Permissions. Complete: Windows asks the user for none (architecture §5, ADR-0021).
//!
//! Windows that run at a higher privilege level than Iamb are not a permission: `focus.rs`
//! reports them as not expandable.

use iamb_platform::{PermissionKind, PermissionStatus, Permissions, Result};

use crate::Platform;

impl Permissions for Platform {
    fn permission_status(&self) -> PermissionStatus {
        PermissionStatus::NOT_REQUIRED
    }

    fn open_settings(&self, _kind: PermissionKind) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_grant() {
        let platform = Platform::new();
        assert_eq!(platform.permission_status(), PermissionStatus::NOT_REQUIRED);
        assert!(platform.permission_status().missing().is_empty());
        for kind in PermissionKind::ALL {
            assert_eq!(platform.open_settings(kind), Ok(()));
        }
    }
}
