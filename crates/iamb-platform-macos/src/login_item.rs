//! Start at login through the system's login-item service, `SMAppService` (architecture §3.2,
//! §5; ADR-0011, ADR-0021). Used by the UI host from task 3.2.
//!
//! - `enable` registers the signed helper app inside `Iamb.app` (the supervisor, bundle
//!   identifier [`HELPER_BUNDLE_ID`], at `Iamb.app/Contents/Library/LoginItems/`) as a login
//!   item; `disable` unregisters it; `login_item_status` maps the service's status: enabled, not
//!   registered, requires approval in System Settings, or not found.
//! - It is the main app that registers the helper, so that the grants belong to the helper and
//!   survive updates.
//!
//! # Only inside the signed bundle
//!
//! `SMAppService` works only for code running from a signed app bundle that contains the
//! helper. Called from anything else (a development binary, a test run by `cargo test`) every
//! method returns [`PlatformError::Invalid`] with a context that says so, without calling the
//! service, rather than an opaque error code. Packaging and signing arrive in Phase 5; the
//! bundle identifier is fixed there and must match [`HELPER_BUNDLE_ID`].

use std::path::Path;

use iamb_platform::{LoginItem, LoginItemStatus, PlatformError, Result};
use objc2_foundation::NSString;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use crate::Platform;

/// The bundle identifier of the helper app (the supervisor) inside `Iamb.app`.
pub(crate) const HELPER_BUNDLE_ID: &str = "org.iamb.Iamb.Service";

const NOT_IN_BUNDLE: PlatformError = PlatformError::Invalid {
    context: "login item: only works when run from the signed Iamb.app bundle",
};

/// The login item's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

/// True when `exe` runs from inside an app bundle (`….app/Contents/MacOS/…`).
pub(crate) fn inside_app_bundle(exe: &Path) -> bool {
    exe.parent()
        .filter(|p| p.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::extension)
        .is_some_and(|ext| ext == "app")
}

/// Maps the service's status.
pub(crate) fn map_status(status: SMAppServiceStatus) -> LoginItemStatus {
    match status {
        SMAppServiceStatus::Enabled => LoginItemStatus::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginItemStatus::RequiresApproval,
        SMAppServiceStatus::NotFound => LoginItemStatus::NotFound,
        _ => LoginItemStatus::Disabled,
    }
}

fn service() -> Result<objc2::rc::Retained<SMAppService>> {
    let exe = std::env::current_exe().map_err(|_| NOT_IN_BUNDLE)?;
    if !inside_app_bundle(&exe) {
        return Err(NOT_IN_BUNDLE);
    }
    let id = NSString::from_str(HELPER_BUNDLE_ID);
    // SAFETY: `loginItemServiceWithIdentifier:` takes any string and returns a new service
    // object; it does not register anything.
    Ok(unsafe { SMAppService::loginItemServiceWithIdentifier(&id) })
}

/// The error of a refused registration: the `SMAppService` error code, which says why
/// (`kSMErrorInvalidSignature` = 3 for an unsigned bundle, `kSMErrorLaunchDeniedByUser` = 11).
fn refused(error: &objc2_foundation::NSError, context: &'static str) -> PlatformError {
    PlatformError::os(i64::try_from(error.code()).unwrap_or(-1), context)
}

/// `kSMErrorAlreadyRegistered`.
const ALREADY_REGISTERED: isize = 12;
/// `kSMErrorJobNotFound`.
const NOT_REGISTERED: isize = 6;

impl LoginItem for Platform {
    fn enable(&self) -> Result<()> {
        let State {} = &self.login_item;
        let service = service()?;
        // SAFETY: registering takes no arguments besides the receiver, a valid service object.
        match unsafe { service.registerAndReturnError() } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ALREADY_REGISTERED => Ok(()),
            Err(e) => Err(refused(&e, "SMAppService register")),
        }
    }

    fn disable(&self) -> Result<()> {
        let service = service()?;
        // SAFETY: as for `register`.
        match unsafe { service.unregisterAndReturnError() } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == NOT_REGISTERED => Ok(()),
            Err(e) => Err(refused(&e, "SMAppService unregister")),
        }
    }

    fn login_item_status(&self) -> Result<LoginItemStatus> {
        let service = service()?;
        // SAFETY: `status` is a read-only property of a valid service object.
        Ok(map_status(unsafe { service.status() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_are_recognised() {
        assert!(inside_app_bundle(Path::new(
            "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service"
        )));
        assert!(inside_app_bundle(Path::new(
            "/Applications/Iamb.app/Contents/MacOS/Iamb"
        )));
        assert!(!inside_app_bundle(Path::new("/usr/local/bin/iamb-service")));
        assert!(!inside_app_bundle(Path::new(
            "/Users/ada/iamb/target/debug/deps/iamb_platform_macos-1234"
        )));
    }

    #[test]
    fn statuses_map() {
        assert_eq!(
            map_status(SMAppServiceStatus::Enabled),
            LoginItemStatus::Enabled
        );
        assert_eq!(
            map_status(SMAppServiceStatus::NotRegistered),
            LoginItemStatus::Disabled
        );
        assert_eq!(
            map_status(SMAppServiceStatus::RequiresApproval),
            LoginItemStatus::RequiresApproval
        );
        assert_eq!(
            map_status(SMAppServiceStatus::NotFound),
            LoginItemStatus::NotFound
        );
    }

    #[test]
    fn outside_the_bundle_every_call_says_why() {
        // `cargo test` runs from target/, never from an app bundle.
        let platform = Platform::new();
        assert_eq!(platform.enable(), Err(NOT_IN_BUNDLE));
        assert_eq!(platform.disable(), Err(NOT_IN_BUNDLE));
        assert_eq!(platform.login_item_status(), Err(NOT_IN_BUNDLE));
    }
}
