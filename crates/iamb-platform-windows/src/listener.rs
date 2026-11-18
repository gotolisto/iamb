//! The key listener. **Task 2.1** fills this file.
//!
//! What it must do (architecture §3.1, §4.1, §5; ADR-0010, ADR-0011; the trait docs of
//! [`KeyListener`]):
//!
//! - `start` creates the listener thread, installs a low-level keyboard hook and a low-level
//!   mouse hook on it, and runs a message loop there. `stop` removes the hooks and ends the
//!   thread.
//! - The hook callback only observes: it always passes the event on, never blocks, and stays
//!   under 1 ms at the 99th percentile. Per event: decode the marker in the event's
//!   extra-info field ([`Marker::decode`](iamb_platform::Marker::decode)); drop Iamb's own
//!   output; report the injection markers; complete a pending self-test; translate a real
//!   key; take a sequence number ([`SeqSource`](iamb_platform::SeqSource)); deliver.
//! - Translation is stateless with respect to the system: it uses the foreground thread's
//!   keyboard layout in the mode that leaves the kernel's dead-key state untouched, and
//!   tracks dead keys itself. A key is a `Char` whenever translation yields text, so `AltGr`
//!   combinations are characters. Named keys are classified with
//!   [`ListenerEvent::named_key`](iamb_platform::ListenerEvent::named_key); a chord without
//!   text is `Boundary(Shortcut)`; a mouse button press is `Boundary(Click)`.
//! - Input-method composition is reported as `Composing`.
//! - `self_test` posts a no-op key event marked
//!   [`Marker::SelfTest`](iamb_platform::Marker::SelfTest) and waits until the hook saw it.
//! - Hotkeys registered through `hotkeys.rs` are served by this thread's message loop and
//!   delivered through the same sink.

use std::time::Duration;

use iamb_platform::{EventSink, KeyListener, PlatformError, Result};

use crate::Platform;

/// The listener's state: the hook thread and what it shares with the callers.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl KeyListener for Platform {
    fn start(&self, _sink: Box<dyn EventSink>) -> Result<()> {
        let State {} = &self.listener;
        Err(PlatformError::Unsupported)
    }

    fn stop(&self) -> Result<()> {
        Err(PlatformError::Unsupported)
    }

    fn self_test(&self, _timeout: Duration) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        assert_eq!(
            platform.start(Box::new(|_: iamb_platform::ListenerEvent| {})),
            Err(PlatformError::Unsupported)
        );
        assert_eq!(platform.stop(), Err(PlatformError::Unsupported));
        assert_eq!(
            platform.self_test(Duration::from_secs(1)),
            Err(PlatformError::Unsupported)
        );
    }
}
