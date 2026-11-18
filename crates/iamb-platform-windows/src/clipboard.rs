//! The clipboard. **Task 2.2** fills this file (with `injector.rs`).
//!
//! What it must do (architecture §5, §8 rules 2 and 8; the trait docs of
//! [`ClipboardAccess`]):
//!
//! - `read_text` returns the Unicode text on the clipboard, if any.
//! - `snapshot` copies every format that can be copied (not the ones rendered on demand, and
//!   not above a size limit; the snapshot then says it is incomplete); `restore` puts them
//!   back.
//! - `write` puts plain text, HTML (in the clipboard's HTML format, with its header) and a
//!   file list on the clipboard. With `transient`, it also sets the formats that keep the
//!   content out of the clipboard history and out of cloud clipboard sync; `restore` sets
//!   them too.
//! - Another program can hold the clipboard open: retry briefly, then report
//!   [`Os`](iamb_platform::PlatformError::Os). Never log clipboard content.

use iamb_platform::core_types::Sensitive;
use iamb_platform::{ClipboardAccess, ClipboardContent, ClipboardSnapshot, PlatformError, Result};

use crate::Platform;

/// The clipboard's state: the registered format identifiers.
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
