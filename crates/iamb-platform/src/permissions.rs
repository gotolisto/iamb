//! The permissions the operating system makes the user grant (architecture §5, ADR-0021).

use std::fmt;

use crate::error::Result;

/// A permission the user grants in the system settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionKind {
    /// macOS Accessibility: needed to post synthetic keys and to inspect the focused field.
    Accessibility,
    /// macOS Input Monitoring: needed to observe the keyboard.
    InputMonitoring,
}

impl PermissionKind {
    /// Both kinds.
    pub const ALL: [PermissionKind; 2] = [Self::Accessibility, Self::InputMonitoring];

    /// A stable lower-case name, for messages and IPC.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::InputMonitoring => "input_monitoring",
        }
    }
}

impl fmt::Display for PermissionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Accessibility => "accessibility",
            Self::InputMonitoring => "input monitoring",
        })
    }
}

/// The state of one permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grant {
    /// The user granted it.
    Granted,
    /// Not granted: never asked, or refused. macOS does not tell the two apart reliably.
    Denied,
    /// This operating system has no such permission (Windows).
    NotRequired,
}

impl Grant {
    /// True unless the permission is [`Denied`](Self::Denied).
    #[must_use]
    pub const fn is_satisfied(self) -> bool {
        !matches!(self, Self::Denied)
    }
}

/// The state of every permission, as seen by the process that asks (which must be the worker:
/// the grants belong to it, ADR-0021).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionStatus {
    /// Accessibility.
    pub accessibility: Grant,
    /// Input Monitoring.
    pub input_monitoring: Grant,
}

impl PermissionStatus {
    /// The status on a system without permissions.
    pub const NOT_REQUIRED: Self = Self {
        accessibility: Grant::NotRequired,
        input_monitoring: Grant::NotRequired,
    };

    /// The state of `kind`.
    #[must_use]
    pub const fn grant(self, kind: PermissionKind) -> Grant {
        match kind {
            PermissionKind::Accessibility => self.accessibility,
            PermissionKind::InputMonitoring => self.input_monitoring,
        }
    }

    /// True when nothing is missing.
    #[must_use]
    pub const fn all_satisfied(self) -> bool {
        self.accessibility.is_satisfied() && self.input_monitoring.is_satisfied()
    }

    /// The permissions still missing, in the order onboarding asks for them.
    #[must_use]
    pub fn missing(self) -> Vec<PermissionKind> {
        PermissionKind::ALL
            .into_iter()
            .filter(|&kind| !self.grant(kind).is_satisfied())
            .collect()
    }
}

/// Reads the permission state and sends the user to the right settings page.
pub trait Permissions: Send + Sync {
    /// The current state. Cheap enough to poll once a second while onboarding is open.
    fn permission_status(&self) -> PermissionStatus;

    /// Opens the system settings page where the user grants `kind` (and, the first time,
    /// makes the OS list Iamb there).
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the page cannot be opened. On a system without
    /// permissions this does nothing and succeeds.
    fn open_settings(&self, kind: PermissionKind) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(PermissionKind::Accessibility.as_str(), "accessibility");
        assert_eq!(PermissionKind::InputMonitoring.as_str(), "input_monitoring");
        assert_eq!(
            PermissionKind::InputMonitoring.to_string(),
            "input monitoring"
        );
    }

    #[test]
    fn missing_lists_only_denied() {
        assert!(PermissionStatus::NOT_REQUIRED.all_satisfied());
        assert!(PermissionStatus::NOT_REQUIRED.missing().is_empty());
        let status = PermissionStatus {
            accessibility: Grant::Granted,
            input_monitoring: Grant::Denied,
        };
        assert!(!status.all_satisfied());
        assert_eq!(status.missing(), [PermissionKind::InputMonitoring]);
        assert_eq!(status.grant(PermissionKind::Accessibility), Grant::Granted);
        assert_eq!(status.grant(PermissionKind::InputMonitoring), Grant::Denied);
        let none = PermissionStatus {
            accessibility: Grant::Denied,
            input_monitoring: Grant::Denied,
        };
        assert_eq!(none.missing(), PermissionKind::ALL);
        assert!(Grant::Granted.is_satisfied() && Grant::NotRequired.is_satisfied());
    }
}
