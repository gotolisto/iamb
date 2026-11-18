//! The rule that keeps Iamb from overwriting what someone else copied (architecture §8 rule 8).
//!
//! The pasteboard counts every change of ownership (`changeCount`). After Iamb writes, the
//! [`Ledger`] remembers the count its write left. When the time comes to put the previous
//! content back:
//!
//! | Ledger | Count now | Restore? | Why |
//! | --- | --- | --- | --- |
//! | nothing written by Iamb | any | yes | a plain `restore` call: there is nothing of Iamb's to protect |
//! | wrote, left count `c` | `c` | yes | the pasteboard still holds what Iamb wrote |
//! | wrote, left count `c` | not `c` | **no** | the user (or another application) copied something after Iamb's write; putting the old content back would destroy it, and Iamb's own content is already gone |
//!
//! Either way the ledger forgets the write: one write, at most one restore. The decision is
//! not an error: `restore` succeeds, having left the newer content alone.
//!
//! What this cannot see: a change made in the instant between Iamb's write and its reading of
//! the count. The two calls follow each other directly, so the window is a few microseconds.

/// What the ledger knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Ledger {
    /// Iamb has not written since the last restore.
    #[default]
    Clean,
    /// Iamb wrote; its write left the pasteboard at this change count.
    Wrote { change_count: isize },
}

/// What to do with a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Put it back.
    Restore,
    /// Leave the pasteboard alone: someone else changed it after Iamb's write.
    LeaveAlone,
}

impl Ledger {
    /// Iamb wrote, leaving the pasteboard at `change_count`.
    pub(crate) fn wrote(&mut self, change_count: isize) {
        *self = Self::Wrote { change_count };
    }

    /// Decides a restore when the pasteboard is at `now`, and forgets the write.
    pub(crate) fn restore(&mut self, now: isize) -> Decision {
        let decision = match *self {
            Self::Clean => Decision::Restore,
            Self::Wrote { change_count } if change_count == now => Decision::Restore,
            Self::Wrote { .. } => Decision::LeaveAlone,
        };
        *self = Self::Clean;
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_while_the_pasteboard_holds_iambs_write() {
        let mut ledger = Ledger::default();
        ledger.wrote(41);
        assert_eq!(ledger, Ledger::Wrote { change_count: 41 });
        assert_eq!(ledger.restore(41), Decision::Restore);
        assert_eq!(ledger, Ledger::Clean);
    }

    #[test]
    fn leaves_a_newer_copy_alone() {
        let mut ledger = Ledger::default();
        ledger.wrote(41);
        assert_eq!(ledger.restore(42), Decision::LeaveAlone);
        assert_eq!(ledger, Ledger::Clean, "the write is forgotten");
        // A later plain restore is not blocked by the old write.
        assert_eq!(ledger.restore(43), Decision::Restore);
    }

    #[test]
    fn a_plain_restore_always_restores() {
        let mut ledger = Ledger::default();
        assert_eq!(ledger.restore(0), Decision::Restore);
        assert_eq!(ledger.restore(isize::MAX), Decision::Restore);
    }

    #[test]
    fn the_last_write_counts() {
        let mut ledger = Ledger::default();
        ledger.wrote(5);
        ledger.wrote(6);
        assert_eq!(ledger.restore(5), Decision::LeaveAlone);
        ledger.wrote(6);
        assert_eq!(ledger.restore(6), Decision::Restore);
    }
}
