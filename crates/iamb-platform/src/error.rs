//! The error type every platform trait returns.

use std::fmt;

use crate::endpoint::PeerRejection;
use crate::permissions::PermissionKind;

/// Why a platform call failed.
///
/// An error never carries user data: the only free text is `context`, a `&'static str` written
/// in the source (the name of the OS call that failed), so an error can always be logged
/// (architecture §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// This operating system, or this build, does not provide the operation.
    Unsupported,
    /// The user has not granted a permission the operation needs.
    PermissionDenied(PermissionKind),
    /// An operating-system call failed.
    Os {
        /// The OS error code (`GetLastError`, an `HRESULT`, an `OSStatus`, `errno`, …).
        code: i64,
        /// The call that failed, e.g. `"SetWindowsHookExW"`. Never user data.
        context: &'static str,
    },
    /// The operation did not finish in the time allowed.
    Timeout,
    /// The listener (or the focus watcher) is already running.
    AlreadyRunning,
    /// The operation needs a running listener (or focus watcher).
    NotRunning,
    /// No window has the keyboard focus.
    NoFocus,
    /// The window a focus token names is gone, or would not take the focus back.
    FocusLost,
    /// The target window refuses synthetic input (it runs at a higher privilege level).
    InjectionRefused,
    /// Another application holds the hotkey chord.
    HotkeyUnavailable,
    /// The endpoint name is taken: another process created it first (architecture §6.1).
    EndpointInUse,
    /// Nothing is listening on the endpoint.
    EndpointNotFound,
    /// The process at the other end of an endpoint failed verification.
    PeerRejected(PeerRejection),
    /// The listener or stream was closed.
    Closed,
    /// An argument was not acceptable to the OS (an empty memory region, a chord the OS cannot
    /// express, …). `context` says which.
    Invalid {
        /// What was wrong. Never user data.
        context: &'static str,
    },
}

/// The result type of the platform traits.
pub type Result<T> = std::result::Result<T, PlatformError>;

impl PlatformError {
    /// An [`Os`](Self::Os) error.
    #[must_use]
    pub const fn os(code: i64, context: &'static str) -> Self {
        Self::Os { code, context }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("not supported on this system"),
            Self::PermissionDenied(kind) => write!(f, "permission not granted: {kind}"),
            Self::Os { code, context } => write!(f, "{context} failed with code {code}"),
            Self::Timeout => f.write_str("timed out"),
            Self::AlreadyRunning => f.write_str("already running"),
            Self::NotRunning => f.write_str("not running"),
            Self::NoFocus => f.write_str("no window has the focus"),
            Self::FocusLost => f.write_str("the window could not be focused"),
            Self::InjectionRefused => f.write_str("the window refuses synthetic input"),
            Self::HotkeyUnavailable => f.write_str("the hotkey is taken by another application"),
            Self::EndpointInUse => f.write_str("the endpoint name is taken"),
            Self::EndpointNotFound => f.write_str("nothing is listening on the endpoint"),
            Self::PeerRejected(why) => write!(f, "peer rejected: {why}"),
            Self::Closed => f.write_str("closed"),
            Self::Invalid { context } => write!(f, "invalid argument: {context}"),
        }
    }
}

impl std::error::Error for PlatformError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_failure_without_user_data() {
        assert_eq!(
            PlatformError::os(5, "SetWindowsHookExW").to_string(),
            "SetWindowsHookExW failed with code 5"
        );
        assert_eq!(
            PlatformError::PermissionDenied(PermissionKind::Accessibility).to_string(),
            "permission not granted: accessibility"
        );
        assert_eq!(
            PlatformError::PeerRejected(PeerRejection::OtherUser).to_string(),
            "peer rejected: the peer runs as another user"
        );
        assert_eq!(
            PlatformError::Invalid {
                context: "empty region"
            }
            .to_string(),
            "invalid argument: empty region"
        );
    }

    #[test]
    fn every_variant_has_a_message() {
        let all = [
            PlatformError::Unsupported,
            PlatformError::PermissionDenied(PermissionKind::InputMonitoring),
            PlatformError::os(-1, "x"),
            PlatformError::Timeout,
            PlatformError::AlreadyRunning,
            PlatformError::NotRunning,
            PlatformError::NoFocus,
            PlatformError::FocusLost,
            PlatformError::InjectionRefused,
            PlatformError::HotkeyUnavailable,
            PlatformError::EndpointInUse,
            PlatformError::EndpointNotFound,
            PlatformError::PeerRejected(PeerRejection::UntrustedBinary),
            PlatformError::Closed,
            PlatformError::Invalid { context: "x" },
        ];
        for e in &all {
            assert!(!e.to_string().is_empty());
        }
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(PlatformError::Timeout);
        assert_eq!(boxed.to_string(), "timed out");
    }
}
