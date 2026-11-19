//! Choosing the platform at compile time: the Windows or macOS crate on those systems, the
//! in-memory mock on request. Linux has no OS crate (it is a development platform), so there
//! the service runs only with `--platform mock`.

use std::sync::Arc;

use iamb_platform::Platform;
use iamb_platform::mock::{MockPlatform, MockWindow};

/// Which platform the worker drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformChoice {
    /// The OS crate of this system.
    Native,
    /// The in-memory mock: no real keyboard. For development and tests only.
    Mock,
}

impl PlatformChoice {
    /// The name used on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Mock => "mock",
        }
    }

    /// Parses the command-line name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "native" => Some(Self::Native),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }

    /// The default: native where there is an OS crate, else the mock.
    #[must_use]
    pub const fn default_for_build() -> Self {
        if cfg!(any(windows, target_os = "macos")) {
            Self::Native
        } else {
            Self::Mock
        }
    }
}

/// A mock machine for `--platform mock`: one focused window with an ordinary text field, so
/// the engine leaves suspension and `ExpandSnippet` and the self-test have something to work
/// on. Nothing reaches a real application.
#[must_use]
pub fn dev_mock() -> MockPlatform {
    let os = MockPlatform::new();
    let field = os.open_window(MockWindow::new("org.iamb.dev-field", std::process::id()));
    let _ = os.focus(field);
    os
}

/// Makes the directory `dir` readable by the current user only, where the system needs a call
/// of the OS crate for it: on Windows a protected DACL for the user's SID alone
/// (`iamb_platform_windows::restrict_to_current_user`, the 2.8 review's finding 20). Unix
/// systems need none: [`create_private_dir`](crate::paths::create_private_dir) sets mode
/// `0700` there.
///
/// # Errors
///
/// The system's error when the permissions cannot be set.
pub fn restrict_to_user(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        iamb_platform_windows::restrict_to_current_user(dir)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Ok(())
    }
}

/// The platform for `choice`.
///
/// # Errors
///
/// A message when `choice` is not available on this system.
pub fn select(choice: PlatformChoice) -> Result<Arc<dyn Platform>, String> {
    match choice {
        PlatformChoice::Mock => Ok(Arc::new(dev_mock())),
        PlatformChoice::Native => native(),
    }
}

#[cfg(windows)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "one signature for every system; only the fallback branch can fail"
)]
fn native() -> Result<Arc<dyn Platform>, String> {
    Ok(Arc::new(iamb_platform_windows::Platform::new()))
}

#[cfg(target_os = "macos")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "one signature for every system; only the fallback branch can fail"
)]
fn native() -> Result<Arc<dyn Platform>, String> {
    Ok(Arc::new(iamb_platform_macos::Platform::new()))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn native() -> Result<Arc<dyn Platform>, String> {
    Err(
        "this system has no native platform; run with --platform mock (development only)"
            .to_owned(),
    )
}
