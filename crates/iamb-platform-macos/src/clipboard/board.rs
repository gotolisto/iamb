//! The clipboard operations, over any pasteboard that can list, count and replace its items.
//!
//! [`Board`] is the little the operations need from a pasteboard; `mac.rs` implements it for
//! the general pasteboard and the tests for an in-memory one. [`Rules`] holds the
//! [`Ledger`](super::ledger::Ledger) and applies the layout of [`layout`](super::layout).

use std::sync::{Mutex, MutexGuard, PoisonError};

use iamb_platform::core_types::Sensitive;
use iamb_platform::{ClipboardContent, ClipboardSnapshot, Result};

use super::layout::{
    RawItem, Reach, SNAPSHOT_LIMIT, WriteItem, content_items, content_reach, items, restore_items,
    restore_reach, snapshot_from,
};
use super::ledger::{Decision, Ledger};

/// What the clipboard operations need from a pasteboard.
pub(crate) trait Board {
    /// The pasteboard's change count.
    fn change_count(&self) -> isize;
    /// Every item, every type, with its data.
    fn items(&self) -> Result<Vec<RawItem>>;
    /// The plain text of the first item that has some.
    fn plain_text(&self) -> Result<Option<String>>;
    /// Clears the pasteboard and writes `items` (none: the pasteboard stays empty), offered to
    /// the devices `reach` says.
    fn replace(&self, items: &[WriteItem<'_>], reach: Reach) -> Result<()>;
}

/// How a restore ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Restored {
    /// The snapshot is back.
    Restored,
    /// Someone copied something after Iamb's write; it was left alone.
    LeftAlone,
}

/// The clipboard rules: layout and the change-count ledger.
#[derive(Debug, Default)]
pub(crate) struct Rules {
    ledger: Mutex<Ledger>,
}

/// The plain text on the pasteboard.
pub(crate) fn read_text(board: &dyn Board) -> Result<Option<Sensitive<String>>> {
    Ok(board.plain_text()?.map(Sensitive::new))
}

/// Everything on the pasteboard.
pub(crate) fn snapshot(board: &dyn Board) -> Result<ClipboardSnapshot> {
    Ok(snapshot_from(board.items()?, SNAPSHOT_LIMIT))
}

impl Rules {
    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        // The ledger is a plain value, valid after every statement.
        self.ledger.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the pasteboard with `content`, and remembers the change count it left.
    pub(crate) fn write(
        &self,
        board: &dyn Board,
        content: &ClipboardContent,
        transient: bool,
    ) -> Result<()> {
        let mut ledger = self.ledger();
        let before = board.change_count();
        let result = board.replace(&content_items(content, transient), content_reach(transient));
        let after = board.change_count();
        // Even a failed write may have cleared the pasteboard: then the restore is still due.
        if result.is_ok() || after != before {
            ledger.wrote(after);
        }
        result
    }

    /// Puts `snapshot` back, unless someone else changed the pasteboard after Iamb's last
    /// write (the ledger's rule).
    pub(crate) fn restore(
        &self,
        board: &dyn Board,
        snapshot: &ClipboardSnapshot,
    ) -> Result<Restored> {
        let mut ledger = self.ledger();
        if ledger.restore(board.change_count()) == Decision::LeaveAlone {
            return Ok(Restored::LeftAlone);
        }
        let saved = items(snapshot);
        board.replace(&restore_items(&saved), restore_reach(&saved))?;
        Ok(Restored::Restored)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! An in-memory pasteboard with a change count, as the general pasteboard behaves.

    use std::cell::{Cell, RefCell};

    use iamb_platform::PlatformError;

    use super::super::layout::PLAIN_TEXT_TYPE;
    use super::*;

    /// One item: types and data, in order.
    pub(crate) type Item = Vec<(String, Vec<u8>)>;

    #[derive(Default)]
    pub(crate) struct FakeBoard {
        pub(crate) items: RefCell<Vec<Item>>,
        pub(crate) count: Cell<isize>,
        /// Types the owner promised but does not deliver.
        pub(crate) broken_promises: RefCell<Vec<String>>,
        /// The next `replace` clears and then fails.
        pub(crate) fail_next_write: Cell<bool>,
        /// `items` fails.
        pub(crate) unreadable: Cell<bool>,
        /// The reach of the last `replace`.
        pub(crate) reach: Cell<Option<Reach>>,
    }

    impl FakeBoard {
        /// Another application copies `items`.
        pub(crate) fn copy(&self, items: Vec<Item>) {
            self.count.set(self.count.get() + 1);
            *self.items.borrow_mut() = items;
        }

        pub(crate) fn contents(&self) -> Vec<Item> {
            self.items.borrow().clone()
        }
    }

    impl Board for FakeBoard {
        fn change_count(&self) -> isize {
            self.count.get()
        }

        fn items(&self) -> Result<Vec<RawItem>> {
            if self.unreadable.get() {
                return Err(PlatformError::os(0, "NSPasteboard.pasteboardItems"));
            }
            let promised = self.broken_promises.borrow();
            Ok(self
                .items
                .borrow()
                .iter()
                .map(|item| RawItem {
                    types: item
                        .iter()
                        .map(|(t, d)| (t.clone(), (!promised.contains(t)).then(|| d.clone())))
                        .collect(),
                })
                .collect())
        }

        fn plain_text(&self) -> Result<Option<String>> {
            Ok(self.items.borrow().iter().find_map(|item| {
                item.iter()
                    .find(|(t, _)| t == PLAIN_TEXT_TYPE)
                    .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
            }))
        }

        fn replace(&self, items: &[WriteItem<'_>], reach: Reach) -> Result<()> {
            // clearContents: a new owner, a new count.
            self.count.set(self.count.get() + 1);
            self.reach.set(Some(reach));
            self.items.borrow_mut().clear();
            if self.fail_next_write.replace(false) {
                return Err(PlatformError::os(0, "NSPasteboard.writeObjects"));
            }
            *self.items.borrow_mut() = items
                .iter()
                .map(|item| {
                    item.types
                        .iter()
                        .map(|(t, d)| ((*t).to_owned(), d.to_vec()))
                        .collect()
                })
                .collect();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::PlatformError;

    use super::super::layout::{CONCEALED_TYPE, HTML_TYPE, PLAIN_TEXT_TYPE, Reach, TRANSIENT_TYPE};
    use super::testing::{FakeBoard, Item};
    use super::*;

    fn item(types: &[(&str, &[u8])]) -> Item {
        types
            .iter()
            .map(|(t, d)| ((*t).to_owned(), d.to_vec()))
            .collect()
    }

    /// The board's items without the history marker restore adds.
    fn without_restore_marker(items: Vec<Item>) -> Vec<Item> {
        items
            .into_iter()
            .map(|item| {
                item.into_iter()
                    .filter(|(t, _)| t != TRANSIENT_TYPE)
                    .collect()
            })
            .collect()
    }

    fn users_content() -> Vec<Item> {
        vec![
            item(&[
                (PLAIN_TEXT_TYPE, "Grüße 🇩🇪".as_bytes()),
                (HTML_TYPE, b"<i>x</i>"),
                ("com.example.binary", &[0, 159, 146, 150, 255]),
            ]),
            item(&[
                ("public.png", &[137, 80, 78, 71]),
                ("public.file-url", b"file:///x"),
            ]),
        ]
    }

    #[test]
    fn snapshot_write_restore_round_trips_byte_for_byte() {
        let board = FakeBoard::default();
        board.copy(users_content());
        let rules = Rules::default();
        for _ in 0..50 {
            let saved = snapshot(&board).expect("snapshot");
            assert!(saved.is_complete());
            rules
                .write(
                    &board,
                    &ClipboardContent::rich("pasted", "<b>pasted</b>"),
                    true,
                )
                .expect("write");
            assert_eq!(
                read_text(&board).expect("read").map(Sensitive::into_inner),
                Some("pasted".to_owned())
            );
            assert_eq!(
                rules.restore(&board, &saved).expect("restore"),
                Restored::Restored
            );
            assert_eq!(without_restore_marker(board.contents()), users_content());
        }
    }

    #[test]
    fn iambs_writes_carry_the_markers() {
        let board = FakeBoard::default();
        let rules = Rules::default();
        rules
            .write(&board, &ClipboardContent::text("x"), true)
            .expect("write");
        let types: Vec<String> = board.contents()[0].iter().map(|(t, _)| t.clone()).collect();
        assert_eq!(types, [PLAIN_TEXT_TYPE, TRANSIENT_TYPE, CONCEALED_TYPE]);
        // The restored content carries the history marker, not the concealed one.
        let empty = ClipboardSnapshot::default();
        board.copy(vec![item(&[(PLAIN_TEXT_TYPE, b"mine")])]);
        let saved = snapshot(&board).expect("snapshot");
        rules
            .write(&board, &ClipboardContent::text("y"), true)
            .expect("write");
        rules.restore(&board, &saved).expect("restore");
        let types: Vec<String> = board.contents()[0].iter().map(|(t, _)| t.clone()).collect();
        assert_eq!(types, [PLAIN_TEXT_TYPE, TRANSIENT_TYPE]);
        // An empty snapshot restores an empty pasteboard.
        rules
            .write(&board, &ClipboardContent::text("z"), true)
            .expect("write");
        rules.restore(&board, &empty).expect("restore");
        assert!(board.contents().is_empty());
    }

    /// Review 2.8, finding 7: what Iamb pastes is never offered to the user's other devices,
    /// nor is content it puts back that its owner marked.
    #[test]
    fn transient_writes_and_restores_of_marked_content_stay_on_this_device() {
        let board = FakeBoard::default();
        let rules = Rules::default();
        rules
            .write(&board, &ClipboardContent::text("form values"), true)
            .expect("write");
        assert_eq!(board.reach.get(), Some(Reach::ThisDevice));
        rules
            .write(&board, &ClipboardContent::text("copied by the user"), false)
            .expect("write");
        assert_eq!(board.reach.get(), Some(Reach::AnyDevice));

        // A password manager's copy: marked concealed, or transient, or both.
        for marker in [CONCEALED_TYPE, TRANSIENT_TYPE] {
            board.copy(vec![item(&[(PLAIN_TEXT_TYPE, b"hunter22"), (marker, b"")])]);
            let saved = snapshot(&board).expect("snapshot");
            rules
                .write(&board, &ClipboardContent::text("pasted"), true)
                .expect("write");
            assert_eq!(
                rules.restore(&board, &saved).expect("restore"),
                Restored::Restored
            );
            assert_eq!(board.reach.get(), Some(Reach::ThisDevice), "{marker}");
        }

        // Ordinary content goes back as it was.
        board.copy(users_content());
        let saved = snapshot(&board).expect("snapshot");
        rules
            .write(&board, &ClipboardContent::text("pasted"), true)
            .expect("write");
        rules.restore(&board, &saved).expect("restore");
        assert_eq!(board.reach.get(), Some(Reach::AnyDevice));
    }

    #[test]
    fn a_copy_after_iambs_write_is_never_overwritten() {
        let board = FakeBoard::default();
        board.copy(users_content());
        let rules = Rules::default();
        let saved = snapshot(&board).expect("snapshot");
        rules
            .write(&board, &ClipboardContent::text("pasted"), true)
            .expect("write");
        // The user copies something while the paste is in flight.
        let newer = vec![item(&[(PLAIN_TEXT_TYPE, b"newer copy")])];
        board.copy(newer.clone());
        assert_eq!(
            rules.restore(&board, &saved).expect("restore"),
            Restored::LeftAlone
        );
        assert_eq!(board.contents(), newer, "the newer copy stays");
        // The rule applies once: a later plain restore puts the snapshot back.
        assert_eq!(
            rules.restore(&board, &saved).expect("restore"),
            Restored::Restored
        );
        assert_eq!(without_restore_marker(board.contents()), users_content());
    }

    #[test]
    fn a_failed_write_still_leads_to_a_restore() {
        let board = FakeBoard::default();
        board.copy(users_content());
        let rules = Rules::default();
        let saved = snapshot(&board).expect("snapshot");
        board.fail_next_write.set(true);
        assert_eq!(
            rules.write(&board, &ClipboardContent::text("x"), true),
            Err(PlatformError::os(0, "NSPasteboard.writeObjects"))
        );
        assert!(board.contents().is_empty(), "the clear happened");
        assert_eq!(
            rules.restore(&board, &saved).expect("restore"),
            Restored::Restored
        );
        assert_eq!(without_restore_marker(board.contents()), users_content());
    }

    #[test]
    fn unreadable_types_are_left_out_and_the_rest_restored() {
        let board = FakeBoard::default();
        board.copy(users_content());
        board
            .broken_promises
            .borrow_mut()
            .push("com.example.binary".to_owned());
        let rules = Rules::default();
        let saved = snapshot(&board).expect("snapshot");
        assert!(!saved.is_complete());
        rules
            .write(&board, &ClipboardContent::text("x"), true)
            .expect("write");
        rules.restore(&board, &saved).expect("restore");
        let mut expected = users_content();
        expected[0].retain(|(t, _)| t != "com.example.binary");
        assert_eq!(without_restore_marker(board.contents()), expected);
    }

    #[test]
    fn an_unreadable_pasteboard_is_an_error() {
        let board = FakeBoard::default();
        board.unreadable.set(true);
        assert!(snapshot(&board).is_err());
        assert_eq!(read_text(&board).map(|t| t.is_none()), Ok(true));
    }
}
