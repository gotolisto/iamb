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
//!    atomic. The focus watcher writes to it in three places, each through a method here so
//!    that the rules run (and are tested) on every system: from the activation notification
//!    it already receives on the main thread ([`Frontmost::activated`]), from every look of
//!    its own thread ([`Frontmost::look`]), and when its thread ends
//!    ([`Frontmost::watcher_ended`]). The tap callback only loads it: it never calls into
//!    `AppKit`.
//!
//! # A look that is overtaken (the close-out review's C2)
//!
//! A look reads the workspace and then the window server, and only then notes the process it
//! read. An activation that arrives in between notes the newer application first; the look
//! must not overwrite it with the older one. So the atomic also counts activation notes: a
//! look reads the count before it reads the workspace and notes its process only if the
//! count has not moved since (compare and set). The activation also wakes the watcher, whose
//! next look notes what is frontmost then.
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

use std::sync::atomic::{AtomicU64, Ordering};

use iamb_platform::KeyTarget;

/// The process id of the frontmost application, as last reported to the tap thread (0 when
/// unknown), in the low 32 bits, and the count of activation notes (and of clears) in the
/// high 32 bits, which a look compares (module docs).
#[derive(Debug, Default)]
pub(crate) struct Frontmost(AtomicU64);

/// The one the focus watcher writes and the listener reads.
pub(crate) static FRONTMOST: Frontmost = Frontmost::new();

/// What a look read before it asked the system: the count of activation notes then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LookTicket(u32);

fn pid_of(word: u64) -> u32 {
    u32::try_from(word & 0xFFFF_FFFF).unwrap_or(0)
}

fn count_of(word: u64) -> u32 {
    u32::try_from(word >> 32).unwrap_or(0)
}

fn word(count: u32, pid: u32) -> u64 {
    (u64::from(count) << 32) | u64::from(pid)
}

impl Frontmost {
    /// Nothing known yet.
    pub(crate) const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Notes `pid` and moves the count, so that a look under way does not overwrite it.
    fn note_newer(&self, pid: u32) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                Some(word(count_of(old).wrapping_add(1), pid))
            });
    }

    /// Write 1 of 3, on the main thread: the workspace says `pid` was activated. Noted only
    /// while a watcher `living` keeps the note up to date (`watcher` module docs), and only a
    /// known process.
    pub(crate) fn activated(&self, living: bool, pid: Option<u32>) {
        if let (true, Some(pid)) = (living, pid.filter(|p| *p != 0)) {
            self.note_newer(pid);
        }
    }

    /// The count of activation notes now, before a look asks the system anything.
    pub(crate) fn begin_look(&self) -> LookTicket {
        LookTicket(count_of(self.0.load(Ordering::Acquire)))
    }

    /// Write 2 of 3, on the watcher thread: a look that began with `ticket` found `pid`
    /// frontmost. Noted only when no activation was noted since the look began (module docs:
    /// the activation is newer than what the look read), and only a known process.
    pub(crate) fn looked(&self, ticket: LookTicket, pid: Option<u32>) {
        let Some(pid) = pid.filter(|p| *p != 0) else {
            return;
        };
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                (count_of(old) == ticket.0).then(|| word(ticket.0, pid))
            });
    }

    /// A whole look: [`begin_look`](Self::begin_look), then `read` (which asks the system
    /// what is frontmost), then [`looked`](Self::looked) with the process `pid_of` names.
    pub(crate) fn look<T>(
        &self,
        read: impl FnOnce() -> Option<T>,
        pid_of: impl Fn(&T) -> u32,
    ) -> Option<T> {
        let ticket = self.begin_look();
        let seen = read();
        self.looked(ticket, seen.as_ref().map(&pid_of));
        seen
    }

    /// Write 3 of 3: the watcher thread ends; nothing keeps the note up to date any more, so
    /// keys are delivered without a stamp rather than with a stale one. Moves the count too,
    /// so a look still under way cannot note a process afterwards.
    pub(crate) fn watcher_ended(&self) {
        self.note_newer(0);
    }

    /// The last process noted, if any. One atomic load.
    pub(crate) fn get(&self) -> Option<u32> {
        match pid_of(self.0.load(Ordering::Acquire)) {
            0 => None,
            pid => Some(pid),
        }
    }

    /// Notes `pid` as the frontmost application, without the rules above (tests).
    #[cfg(test)]
    pub(crate) fn note(&self, pid: Option<u32>) {
        self.note_newer(pid.unwrap_or(0));
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

    /// The close-out review's C2: an activation noted while a look is under way is newer than
    /// what the look read, and stays.
    #[test]
    fn a_look_under_way_does_not_overwrite_a_newer_activation() {
        let front = Frontmost::new();
        front.activated(true, Some(100));
        let seen = front.look(
            || {
                // The look has read the workspace (application 100 frontmost); before it
                // notes that, the main thread hears that 200 was activated.
                front.activated(true, Some(200));
                Some(100_u32)
            },
            |pid| *pid,
        );
        assert_eq!(seen, Some(100), "the look returns what it read");
        assert_eq!(front.get(), Some(200), "the newer note stays");
        // The next look, after the activation, notes what it finds.
        front.look(|| Some(200_u32), |pid| *pid);
        assert_eq!(front.get(), Some(200));
        front.look(|| Some(300_u32), |pid| *pid);
        assert_eq!(
            front.get(),
            Some(300),
            "a look with nothing in between is noted"
        );
    }

    /// The watcher's three writes, as the watcher makes them (they exist only on macOS, so
    /// the rules are here, where every system tests them; the close-out review's C5).
    #[test]
    fn the_three_writes_of_the_watcher() {
        let front = Frontmost::new();
        // 1. An activation is noted only while a watcher lives, and only a real process.
        front.activated(false, Some(41));
        assert_eq!(
            front.get(),
            None,
            "no watcher keeps the note: nothing noted"
        );
        front.activated(true, None);
        front.activated(true, Some(0));
        assert_eq!(front.get(), None);
        front.activated(true, Some(41));
        assert_eq!(front.get(), Some(41));
        // 2. A look notes what it found; a look that found nothing leaves the note.
        front.look(|| Some(42_u32), |pid| *pid);
        assert_eq!(front.get(), Some(42));
        front.look(|| None::<u32>, |pid| *pid);
        assert_eq!(front.get(), Some(42));
        // 3. The watcher thread ends: the note is cleared, and a look that began before
        // cannot bring it back.
        let ticket = front.begin_look();
        front.watcher_ended();
        assert_eq!(front.get(), None);
        front.looked(ticket, Some(43));
        assert_eq!(
            front.get(),
            None,
            "a stale look after the end notes nothing"
        );
        // A new watcher's first look notes again.
        front.look(|| Some(44_u32), |pid| *pid);
        assert_eq!(front.get(), Some(44));
        assert_eq!(stamp(0, &front).1, TargetSource::Frontmost);
    }

    #[test]
    fn the_count_wraps_without_losing_the_rule() {
        let front = Frontmost(AtomicU64::new(word(u32::MAX, 7)));
        let ticket = front.begin_look();
        front.activated(true, Some(8));
        assert_eq!(count_of(front.0.load(Ordering::Acquire)), 0);
        front.looked(ticket, Some(9));
        assert_eq!(front.get(), Some(8));
    }

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
