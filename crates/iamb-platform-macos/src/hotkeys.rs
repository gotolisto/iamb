//! Global hotkeys. **Task 2.3** fills this file (with `listener.rs`).
//!
//! What it must do (architecture §4.5, §5; the trait docs of [`Hotkeys`]):
//!
//! - Register each [`Chord`] with the OS hotkey API (a Carbon call, declared by hand here:
//!   see the crate docs), with its handler on the listener thread's run loop, which delivers
//!   [`ListenerEvent::Hotkey`](iamb_platform::ListenerEvent::Hotkey) through the listener's
//!   sink, with the next sequence number.
//! - Registrations made while the listener is stopped are kept and applied when it starts;
//!   they are applied again after a restart.
//! - A chord the system refuses is
//!   [`HotkeyUnavailable`](iamb_platform::PlatformError::HotkeyUnavailable). `Meta` is the
//!   Command key and `Alt` the Option key.
//! - The event tap is never used for hotkeys: the listener stays observe-only (ADR-0010).

use iamb_platform::{Chord, HotkeyId, Hotkeys, PlatformError, Result};

use crate::Platform;

/// The registered chords, shared with the listener thread.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl Hotkeys for Platform {
    fn register(&self, _id: HotkeyId, _chord: Chord) -> Result<()> {
        let State {} = &self.hotkeys;
        Err(PlatformError::Unsupported)
    }

    fn unregister(&self, _id: HotkeyId) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        let chord = Chord::parse("Ctrl+Shift+Space").expect("valid chord");
        assert_eq!(
            platform.register(HotkeyId(1), chord),
            Err(PlatformError::Unsupported)
        );
        assert_eq!(
            platform.unregister(HotkeyId(1)),
            Err(PlatformError::Unsupported)
        );
    }
}
