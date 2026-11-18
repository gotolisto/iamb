//! The key listener. **Task 2.3** fills this file.
//!
//! What it must do (architecture §3.1, §4.1, §5; ADR-0010, ADR-0011, ADR-0021; the trait docs
//! of [`KeyListener`]):
//!
//! - `start` creates the listener thread, creates a listen-only event tap for key, flag and
//!   mouse-button events, and runs a run loop there. Without Input Monitoring the tap cannot
//!   be created: report
//!   [`PermissionDenied`](iamb_platform::PlatformError::PermissionDenied). `stop` removes the
//!   tap and ends the thread.
//! - The tap callback only observes and stays under 1 ms at the 99th percentile. Per event:
//!   decode the marker in the event-source user data
//!   ([`Marker::decode_i64`](iamb_platform::Marker::decode_i64)); drop Iamb's own output;
//!   report the injection markers; complete a pending self-test; translate a real key; take a
//!   sequence number ([`SeqSource`](iamb_platform::SeqSource)); deliver.
//! - When the system disables the tap (a timeout, or user input), re-enable it and deliver
//!   `Overflow`: events were lost.
//! - Translation uses the current keyboard layout without disturbing the target
//!   application's dead-key state, and tracks dead keys itself. A key is a `Char` whenever
//!   translation yields text, so Option combinations are characters. Named keys are
//!   classified with
//!   [`ListenerEvent::named_key`](iamb_platform::ListenerEvent::named_key); a chord without
//!   text is `Boundary(Shortcut)`; a mouse button press is `Boundary(Click)`.
//! - Input-method composition is reported as `Composing`.
//! - `self_test` posts a no-op event marked
//!   [`Marker::SelfTest`](iamb_platform::Marker::SelfTest) and waits until the tap saw it.
//! - Hotkeys registered through `hotkeys.rs` are served by this thread's run loop and
//!   delivered through the same sink.

use std::time::Duration;

use iamb_platform::{EventSink, KeyListener, PlatformError, Result};

use crate::Platform;

/// The listener's state: the tap thread and what it shares with the callers.
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
