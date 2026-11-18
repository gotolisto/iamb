//! Starting Iamb at login (architecture §3.2, §5, ADR-0021).

use crate::error::Result;

/// Whether the supervisor starts at login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoginItemStatus {
    /// Registered: the supervisor starts at the next login.
    Enabled,
    /// Not registered.
    Disabled,
    /// Registered, but the user must approve it in the system settings first (macOS).
    RequiresApproval,
    /// The helper to register cannot be found: the installation is damaged.
    NotFound,
}

/// Registers the supervisor to start at login: the per-user Run key on Windows, the login-item
/// registration of the helper app on macOS. Called by the UI host; needs no administrator
/// rights.
pub trait LoginItem: Send + Sync {
    /// Registers the login item. Enabling an enabled item is not an error.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the registration is refused.
    fn enable(&self) -> Result<()>;

    /// Removes the registration. Disabling a disabled item is not an error.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn disable(&self) -> Result<()>;

    /// The current state.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the state cannot be read.
    fn login_item_status(&self) -> Result<LoginItemStatus>;
}
