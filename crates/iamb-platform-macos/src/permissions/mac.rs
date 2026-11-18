//! The system calls behind [`Probe`], and the `Permissions` trait for
//! [`Platform`]. See the parent module for what each call does.

use iamb_platform::{PermissionKind, PermissionStatus, Permissions, Result};
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightListenEventAccess, CGRequestListenEventAccess};
use objc2_foundation::{NSString, NSURL};

use super::policy::{self, Probe};
use crate::Platform;

/// The permission checker's state: none. Every answer is read from the system when asked.
#[derive(Debug, Default)]
pub(crate) struct State {}

/// The system's own answers about this process.
pub(crate) struct SystemProbe;

impl Probe for SystemProbe {
    fn accessibility(&self) -> bool {
        // SAFETY: the function takes no arguments and only reads this process's trust state.
        unsafe { AXIsProcessTrusted() }
    }

    fn input_monitoring(&self) -> bool {
        CGPreflightListenEventAccess()
    }

    fn request_accessibility(&self) {
        // SAFETY: a constant the framework initialises when it is loaded; it never changes.
        let prompt = unsafe { kAXTrustedCheckOptionPrompt };
        let options =
            CFDictionary::<CFString, CFBoolean>::from_slices(&[prompt], &[CFBoolean::new(true)]);
        // SAFETY: `options` is a valid dictionary with the one key the function documents,
        // holding a `CFBoolean`, and it lives for the duration of the call. The result (the
        // trust state before any prompt) is read again by the next status check.
        let _ = unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
    }

    fn request_input_monitoring(&self) {
        // The result is the state before any prompt; the next status check reads it again.
        let _ = CGRequestListenEventAccess();
    }

    fn open_url(&self, url: &str) -> bool {
        autoreleasepool(|_| {
            NSURL::URLWithString(&NSString::from_str(url))
                .is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url))
        })
    }
}

impl Permissions for Platform {
    fn permission_status(&self) -> PermissionStatus {
        let State {} = &self.permissions;
        policy::status(&SystemProbe)
    }

    fn open_settings(&self, kind: PermissionKind) -> Result<()> {
        policy::open_settings(&SystemProbe, kind)
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::Grant;

    use super::*;

    // `open_settings` is not called in tests: it would open System Settings on the machine
    // that runs them, and the request it makes shows a system prompt. Its logic is tested in
    // `policy` with a fake probe.

    #[test]
    fn the_status_is_the_systems_answer_for_this_process() {
        let platform = Platform::new();
        let status = platform.permission_status();
        assert_eq!(
            status.accessibility == Grant::Granted,
            SystemProbe.accessibility()
        );
        assert_eq!(
            status.input_monitoring == Grant::Granted,
            SystemProbe.input_monitoring()
        );
        // macOS always requires both.
        assert_ne!(status.accessibility, Grant::NotRequired);
        assert_ne!(status.input_monitoring, Grant::NotRequired);
        assert_eq!(
            status.all_satisfied(),
            SystemProbe.accessibility() && SystemProbe.input_monitoring()
        );
    }

    #[test]
    fn asking_repeatedly_gives_the_same_answer_and_never_requests() {
        // If a status check requested access, the second answer could differ from the first on
        // a machine where the permission was never asked for, and the test run would show a
        // system prompt. Neither happens.
        let platform = Platform::new();
        let first = platform.permission_status();
        for _ in 0..50 {
            assert_eq!(platform.permission_status(), first);
        }
    }
}
