//! The clipboard: the general pasteboard. **Task 2.4** fills this file (with `injector.rs`).
//!
//! What it must do (architecture §5, §8 rules 2 and 8; the trait docs of
//! [`ClipboardAccess`]):
//!
//! - `read_text` returns the plain text on the pasteboard, if any.
//! - `snapshot` copies every type of every pasteboard item that can be copied (not above a
//!   size limit; the snapshot then says it is incomplete); `restore` puts them back.
//! - `write` puts plain text, HTML and file URLs on the pasteboard. With `transient`, it
//!   also adds the transient and concealed marker types that clipboard managers and the
//!   system's cross-device clipboard honour; `restore` adds them too.
//! - Never log pasteboard content.

use iamb_platform::core_types::Sensitive;
use iamb_platform::{ClipboardAccess, ClipboardContent, ClipboardSnapshot, PlatformError, Result};

use crate::Platform;

/// The clipboard's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl ClipboardAccess for Platform {
    fn read_text(&self) -> Result<Option<Sensitive<String>>> {
        let State {} = &self.clipboard;
        Err(PlatformError::Unsupported)
    }

    fn snapshot(&self) -> Result<ClipboardSnapshot> {
        Err(PlatformError::Unsupported)
    }

    fn write(&self, _content: &ClipboardContent, _transient: bool) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn restore(&self, _snapshot: ClipboardSnapshot) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        assert!(matches!(
            platform.read_text(),
            Err(PlatformError::Unsupported)
        ));
        assert_eq!(platform.snapshot(), Err(PlatformError::Unsupported));
        assert_eq!(
            platform.write(&ClipboardContent::text("x"), true),
            Err(PlatformError::Unsupported)
        );
        assert_eq!(
            platform.restore(ClipboardSnapshot::default()),
            Err(PlatformError::Unsupported)
        );
    }
}
