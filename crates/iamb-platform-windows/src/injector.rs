//! The injector. **Task 2.2** fills this file.
//!
//! What it must do (architecture §4.2, §4.6, §5; ADR-0019; the protocol in the trait docs of
//! [`Injector`]):
//!
//! - Post the start marker, wait for the hand-off to seal
//!   ([`TypedHandoff::wait_sealed`]), then send the output and the end marker.
//! - Synthesise Unicode key events and send them in atomic batches: the leading deletion
//!   (the expansion's own plus the characters typed after the trigger,
//!   [`compensate`](iamb_platform::compensate)), the text, the characters to type again
//!   ([`TypedHandoff::read`], copied into the batch and zeroed after sending) and the end
//!   marker go to the OS in one call, which real keys cannot interleave. A `Wait` or a
//!   `Paste` step ends a batch.
//! - Mark every synthetic event in its extra-info field:
//!   [`Marker::Output`](iamb_platform::Marker::Output) for output, the injection markers for
//!   the two no-op events.
//! - Choose how text is sent (key events, or a paste for long text) and how many presses a
//!   deletion or caret move needs from a per-application strategy table kept here, with the
//!   request's overrides on top.
//! - A window at a higher privilege level refuses synthetic input: report
//!   [`InjectionRefused`](iamb_platform::PlatformError::InjectionRefused).
//! - Pasting uses `clipboard.rs`: snapshot, transient write, paste shortcut, restore.

use iamb_platform::{InjectionOutcome, InjectionRequest, Injector, PlatformError, TypedHandoff};

use crate::Platform;

/// The injector's state: the strategy table.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl Injector for Platform {
    fn inject(&self, _request: &InjectionRequest, _typed: &TypedHandoff) -> InjectionOutcome {
        let State {} = &self.injector;
        InjectionOutcome::Failed(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::core_types::ExpansionId;

    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        let request = InjectionRequest::new(ExpansionId::from_raw(1), Vec::new());
        assert_eq!(
            platform.inject(&request, &TypedHandoff::new()),
            InjectionOutcome::Failed(PlatformError::Unsupported)
        );
    }
}
