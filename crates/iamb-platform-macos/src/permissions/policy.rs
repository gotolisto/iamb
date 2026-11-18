//! What Iamb does with the two macOS permissions, free of any system call.
//!
//! The system calls sit behind [`Probe`]; everything decided about them is decided here and
//! tested on every system with a fake probe:
//!
//! - **Asking is not requesting.** [`status`] only reads. It never makes the system show a
//!   prompt and never adds Iamb to a list in System Settings, so it can be polled.
//! - **Requesting is explicit.** [`open_settings`] is the one place that requests: it is what
//!   runs when the user presses the button in onboarding. For a permission that is still
//!   missing it first makes the system list Iamb on the page (the system may show its own
//!   prompt, once), then opens the page.
//! - **The listener needs both.** [`missing_for_listener`] names the permission whose absence
//!   keeps the listener from starting: Input Monitoring, without which the tap sees no key,
//!   then Accessibility, without which the self-test's event (and every expansion) is
//!   dropped by the system.

use iamb_platform::{Grant, PermissionKind, PermissionStatus, PlatformError, Result};

/// The system calls about permissions. Each is about the calling process.
pub(crate) trait Probe {
    /// Whether this process is a trusted Accessibility client. Shows nothing.
    fn accessibility(&self) -> bool;
    /// Whether this process may listen to input events. Shows nothing.
    fn input_monitoring(&self) -> bool;
    /// Makes the system list this process under Accessibility; it may show its prompt.
    fn request_accessibility(&self);
    /// Makes the system list this process under Input Monitoring; it may show its prompt.
    fn request_input_monitoring(&self);
    /// Opens a URL with the system; `false` when nothing would open it.
    fn open_url(&self, url: &str) -> bool;
}

/// The Privacy & Security page of System Settings where `kind` is granted.
pub(crate) const fn settings_url(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionKind::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    }
}

const fn grant(granted: bool) -> Grant {
    if granted {
        Grant::Granted
    } else {
        Grant::Denied
    }
}

/// The state of both permissions, read now. Nothing is cached, so a grant or a revocation is
/// seen by the next call, as far as the system itself reports it to a running process.
pub(crate) fn status(probe: &dyn Probe) -> PermissionStatus {
    PermissionStatus {
        accessibility: grant(probe.accessibility()),
        input_monitoring: grant(probe.input_monitoring()),
    }
}

/// Whether `kind` is granted now.
fn is_granted(probe: &dyn Probe, kind: PermissionKind) -> bool {
    match kind {
        PermissionKind::Accessibility => probe.accessibility(),
        PermissionKind::InputMonitoring => probe.input_monitoring(),
    }
}

/// Makes the system list Iamb on the page of `kind` if the permission is missing, then opens
/// the page.
///
/// # Errors
///
/// [`Os`](PlatformError::Os) when the system would not open the page.
pub(crate) fn open_settings(probe: &dyn Probe, kind: PermissionKind) -> Result<()> {
    if !is_granted(probe, kind) {
        match kind {
            PermissionKind::Accessibility => probe.request_accessibility(),
            PermissionKind::InputMonitoring => probe.request_input_monitoring(),
        }
    }
    if probe.open_url(settings_url(kind)) {
        Ok(())
    } else {
        Err(PlatformError::os(0, "NSWorkspace openURL"))
    }
}

/// The permission the listener cannot start without, if one is missing.
pub(crate) fn missing_for_listener(probe: &dyn Probe) -> Option<PermissionKind> {
    [
        PermissionKind::InputMonitoring,
        PermissionKind::Accessibility,
    ]
    .into_iter()
    .find(|&kind| !is_granted(probe, kind))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::cell::{Cell, RefCell};

    use super::Probe;

    /// A probe a test sets by hand, which records what was asked of it.
    #[derive(Default)]
    pub(crate) struct FakeProbe {
        pub(crate) accessibility: Cell<bool>,
        pub(crate) input_monitoring: Cell<bool>,
        pub(crate) opens: Cell<bool>,
        pub(crate) calls: RefCell<Vec<String>>,
    }

    impl FakeProbe {
        pub(crate) fn new(accessibility: bool, input_monitoring: bool) -> Self {
            let probe = Self::default();
            probe.accessibility.set(accessibility);
            probe.input_monitoring.set(input_monitoring);
            probe.opens.set(true);
            probe
        }

        /// The requests and opened URLs so far, in order. Status reads are not recorded.
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Probe for FakeProbe {
        fn accessibility(&self) -> bool {
            self.accessibility.get()
        }

        fn input_monitoring(&self) -> bool {
            self.input_monitoring.get()
        }

        fn request_accessibility(&self) {
            self.calls
                .borrow_mut()
                .push("request accessibility".to_owned());
        }

        fn request_input_monitoring(&self) {
            self.calls
                .borrow_mut()
                .push("request input monitoring".to_owned());
        }

        fn open_url(&self, url: &str) -> bool {
            self.calls.borrow_mut().push(format!("open {url}"));
            self.opens.get()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeProbe;
    use super::*;

    #[test]
    fn status_reports_each_permission_on_its_own() {
        for (accessibility, input_monitoring) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let probe = FakeProbe::new(accessibility, input_monitoring);
            let status = status(&probe);
            assert_eq!(status.accessibility, grant(accessibility));
            assert_eq!(status.input_monitoring, grant(input_monitoring));
            assert_eq!(status.all_satisfied(), accessibility && input_monitoring);
            // A Mac always requires them.
            assert_ne!(status.accessibility, Grant::NotRequired);
            assert_ne!(status.input_monitoring, Grant::NotRequired);
        }
    }

    #[test]
    fn a_status_check_never_requests_and_never_opens_anything() {
        let probe = FakeProbe::new(false, false);
        for _ in 0..100 {
            let _ = status(&probe);
            let _ = missing_for_listener(&probe);
        }
        assert!(probe.calls().is_empty());
    }

    #[test]
    fn a_revoked_permission_shows_on_the_next_check() {
        let probe = FakeProbe::new(true, true);
        assert!(status(&probe).all_satisfied());
        assert_eq!(missing_for_listener(&probe), None);

        probe.input_monitoring.set(false);
        assert_eq!(status(&probe).missing(), [PermissionKind::InputMonitoring]);
        assert_eq!(
            missing_for_listener(&probe),
            Some(PermissionKind::InputMonitoring)
        );

        probe.input_monitoring.set(true);
        probe.accessibility.set(false);
        assert_eq!(status(&probe).missing(), [PermissionKind::Accessibility]);
        assert_eq!(
            missing_for_listener(&probe),
            Some(PermissionKind::Accessibility)
        );

        probe.accessibility.set(true);
        assert!(status(&probe).all_satisfied());
    }

    #[test]
    fn the_listener_names_input_monitoring_first() {
        assert_eq!(
            missing_for_listener(&FakeProbe::new(false, false)),
            Some(PermissionKind::InputMonitoring)
        );
        assert_eq!(
            missing_for_listener(&FakeProbe::new(true, false)),
            Some(PermissionKind::InputMonitoring)
        );
        assert_eq!(
            missing_for_listener(&FakeProbe::new(false, true)),
            Some(PermissionKind::Accessibility)
        );
        assert_eq!(missing_for_listener(&FakeProbe::new(true, true)), None);
    }

    #[test]
    fn opening_settings_requests_the_missing_permission_then_opens_its_page() {
        let probe = FakeProbe::new(false, false);
        assert_eq!(open_settings(&probe, PermissionKind::Accessibility), Ok(()));
        assert_eq!(
            probe.calls(),
            [
                "request accessibility",
                "open x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            ]
        );

        let probe = FakeProbe::new(false, false);
        assert_eq!(
            open_settings(&probe, PermissionKind::InputMonitoring),
            Ok(())
        );
        assert_eq!(
            probe.calls(),
            [
                "request input monitoring",
                "open x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
            ]
        );
    }

    #[test]
    fn a_granted_permission_is_not_requested_again() {
        let probe = FakeProbe::new(true, false);
        assert_eq!(open_settings(&probe, PermissionKind::Accessibility), Ok(()));
        assert_eq!(probe.calls().len(), 1);
        assert!(probe.calls()[0].starts_with("open "));
        // The other permission is not touched either way.
        assert!(!probe.calls().iter().any(|call| call.contains("monitoring")));
    }

    #[test]
    fn a_page_that_does_not_open_is_an_error() {
        let probe = FakeProbe::new(true, true);
        probe.opens.set(false);
        assert_eq!(
            open_settings(&probe, PermissionKind::InputMonitoring),
            Err(PlatformError::os(0, "NSWorkspace openURL"))
        );
    }

    #[test]
    fn the_pages_are_the_two_privacy_panes() {
        for kind in PermissionKind::ALL {
            let url = settings_url(kind);
            assert!(url.starts_with("x-apple.systempreferences:com.apple.preference.security?"));
        }
        assert_ne!(
            settings_url(PermissionKind::Accessibility),
            settings_url(PermissionKind::InputMonitoring)
        );
    }
}
