//! Where a key went: the [`KeyTarget`] the listener stamps keys with (the 2.8 review's finding
//! 5 on macOS).
//!
//! The service drops a key whose target is not the window of the last focus change it
//! delivered, so a key typed into a newly focused field is not handled with the verdict of the
//! field before. On macOS the listener knows the target in one of two ways, tried in order:
//!
//! 1. **The event's own field.** `kCGEventTargetUnixProcessID` names the process the system
//!    routes the event to. Whether the system has filled it in by the time a session event
//!    tap sees the event is not documented. So it is used only when it is not 0, and the
//!    listener counts how often it was ([`ListenerStats`](super::stats::ListenerStats)); the
//!    test `tests/focus.rs` says on the CI runner which source stamped its key.
//! 2. **The frontmost application as last reported to the tap thread.** [`FRONTMOST`] is an
//!    atomic. The focus watcher writes to it from the activation notification it already
//!    receives on the main thread, and from every look of its own thread, and clears it when
//!    its thread ends. The tap callback only loads it: it never calls into `AppKit`.
//!
//! When neither names a process, the key is delivered **without** a target
//! ([`EventSink::deliver`](iamb_platform::EventSink::deliver)). That is today's unstamped
//! path, which the service handles as before: an unknown target never makes a key dropped.
//!
//! # A stamp that is wrong
//!
//! A stamp that names another process than the watcher (a key window owned by a helper
//! process, a field the system fills with something else) would make the service drop every
//! key there. Two things keep that from lasting. The service reads the focus from the
//! inspector after a few such keys, and once the inspector has named the same window for
//! the service's `RESYNC_AFTER` (half a second), it handles keys
//! with that stamp as unstamped until the focus changes (`worker/engine.rs`, N5). And the
//! listener counts the keys whose field disagreed with the frontmost application, so a
//! systematic disagreement shows in the stats and in the CI test's output.
//!
//! Mouse clicks are not stamped. A click is a boundary, which only clears the engine's buffer:
//! delivering one that went elsewhere is always safe, dropping one is not.

use std::sync::atomic::{AtomicU32, Ordering};

use iamb_platform::KeyTarget;

/// The process id of the frontmost application, as last reported to the tap thread; 0 when
/// unknown.
#[derive(Debug, Default)]
pub(crate) struct Frontmost(AtomicU32);

/// The one the focus watcher writes and the listener reads.
pub(crate) static FRONTMOST: Frontmost = Frontmost::new();

impl Frontmost {
    /// Nothing known yet.
    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// The frontmost application is `pid` now; `None` (or 0) means nothing is known.
    pub(crate) fn note(&self, pid: Option<u32>) {
        self.0.store(pid.unwrap_or(0), Ordering::Release);
    }

    /// The last process noted, if any. One atomic load.
    pub(crate) fn get(&self) -> Option<u32> {
        match self.0.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        }
    }
}

/// Which source stamped a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetSource {
    /// The event's `kCGEventTargetUnixProcessID`.
    Event,
    /// The frontmost application as last reported ([`FRONTMOST`]).
    Frontmost,
    /// Neither: the key is delivered without a target.
    Unknown,
}

/// The stamp of one key: `event_pid` is the event's `kCGEventTargetUnixProcessID` (0 when the
/// system did not fill it in). Also says whether the event's field and the frontmost
/// application, both known, disagreed.
pub(crate) fn stamp(
    event_pid: i64,
    frontmost: &Frontmost,
) -> (Option<KeyTarget>, TargetSource, bool) {
    let front = frontmost.get();
    match u32::try_from(event_pid).ok().filter(|pid| *pid != 0) {
        Some(pid) => (
            Some(KeyTarget::process(pid)),
            TargetSource::Event,
            front.is_some_and(|f| f != pid),
        ),
        None => match front {
            Some(pid) => (
                Some(KeyTarget::process(pid)),
                TargetSource::Frontmost,
                false,
            ),
            None => (None, TargetSource::Unknown, false),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_field_comes_first_then_the_frontmost_application_then_nothing() {
        let front = Frontmost::new();
        assert_eq!(stamp(0, &front), (None, TargetSource::Unknown, false));
        assert_eq!(stamp(-1, &front), (None, TargetSource::Unknown, false));
        assert_eq!(
            stamp(412, &front),
            (Some(KeyTarget::process(412)), TargetSource::Event, false)
        );
        front.note(Some(77));
        assert_eq!(
            stamp(0, &front),
            (Some(KeyTarget::process(77)), TargetSource::Frontmost, false)
        );
        // A field that is filled in wins, and a disagreement is said.
        assert_eq!(
            stamp(412, &front),
            (Some(KeyTarget::process(412)), TargetSource::Event, true)
        );
        assert_eq!(
            stamp(77, &front),
            (Some(KeyTarget::process(77)), TargetSource::Event, false)
        );
        // A value no process id can have is no stamp.
        assert_eq!(
            stamp(i64::from(u32::MAX) + 1, &front),
            (Some(KeyTarget::process(77)), TargetSource::Frontmost, false)
        );
        front.note(None);
        assert_eq!(front.get(), None);
        assert_eq!(stamp(0, &front).1, TargetSource::Unknown);
    }
}
