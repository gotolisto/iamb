//! Pasting through the pasteboard and putting the previous content back (architecture §8
//! rule 8): snapshot → transient write → paste shortcut → bounded wait → restore.
//!
//! The previous content is restored on every path after the write was attempted:
//!
//! | Step fails | Then |
//! | --- | --- |
//! | snapshot | nothing is written: without a snapshot the previous content could not come back. The paste fails |
//! | snapshot incomplete (a type the owner did not deliver, or past [`SNAPSHOT_LIMIT`](super::layout::SNAPSHOT_LIMIT)) | nothing is written either: that part could not come back. The paste fails with [`SNAPSHOT_INCOMPLETE`]; the injector types plain text with keys instead and stops for other content (the 2.8 review's finding 14) |
//! | write | restore at once (the write may have cleared the pasteboard); the paste fails |
//! | paste shortcut | restore at once; the paste fails |
//! | — | wait `settle` (the application reads the pasteboard when it handles the shortcut, which can be later than the post), then restore |
//! | restore | one more try after [`RETRY_DELAY`]; if that fails too, the error is returned |
//!
//! The restore follows the change-count rule of [`ledger`](super::ledger): content someone
//! copied after Iamb's write is left alone.

use std::time::Duration;

use iamb_platform::{ClipboardAccess, ClipboardContent, ClipboardSnapshot, PlatformError, Result};

/// The error of a paste that did not start because the snapshot could not save everything on
/// the pasteboard. Nothing was written.
pub(crate) const SNAPSHOT_INCOMPLETE: PlatformError = PlatformError::Invalid {
    context: "the pasteboard holds content that cannot be saved and put back after a paste",
};

/// The pause before the second restore attempt.
pub(crate) const RETRY_DELAY: Duration = Duration::from_millis(20);

/// Pastes `content` into the focused application. `press_paste` posts the paste shortcut;
/// `sleep` waits.
pub(crate) fn paste_with_restore(
    clipboard: &dyn ClipboardAccess,
    content: &ClipboardContent,
    settle: Duration,
    press_paste: &mut dyn FnMut() -> Result<()>,
    sleep: &mut dyn FnMut(Duration),
) -> Result<()> {
    let saved = clipboard.snapshot()?;
    if !saved.is_complete() {
        return Err(SNAPSHOT_INCOMPLETE);
    }
    if let Err(error) = clipboard.write(content, true) {
        // A failure to restore is less important than the write's own error.
        let _ = restore(clipboard, saved, sleep);
        return Err(error);
    }
    if let Err(error) = press_paste() {
        let _ = restore(clipboard, saved, sleep);
        return Err(error);
    }
    sleep(settle);
    restore(clipboard, saved, sleep)
}

fn restore(
    clipboard: &dyn ClipboardAccess,
    saved: ClipboardSnapshot,
    sleep: &mut dyn FnMut(Duration),
) -> Result<()> {
    if clipboard.restore(saved.clone()).is_ok() {
        return Ok(());
    }
    sleep(RETRY_DELAY);
    clipboard.restore(saved)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};

    use iamb_platform::core_types::Sensitive;
    use iamb_platform::mock::MockPlatform;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        Snapshot,
        Write,
        Press,
        Sleep(u128),
        Restore,
    }

    /// A clipboard that logs its calls and fails where told. Holds no content.
    #[derive(Default)]
    struct Scripted {
        calls: Mutex<Vec<Call>>,
        fail_snapshot: bool,
        incomplete: bool,
        fail_write: bool,
        restore_failures: Mutex<usize>,
    }

    impl Scripted {
        fn log(&self, call: Call) {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }

        fn calls(&self) -> Vec<Call> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    fn err(context: &'static str) -> PlatformError {
        PlatformError::os(0, context)
    }

    impl ClipboardAccess for Scripted {
        fn read_text(&self) -> Result<Option<Sensitive<String>>> {
            Ok(None)
        }

        fn snapshot(&self) -> Result<ClipboardSnapshot> {
            self.log(Call::Snapshot);
            if self.fail_snapshot {
                Err(err("snapshot"))
            } else {
                Ok(ClipboardSnapshot::new(Vec::new(), !self.incomplete))
            }
        }

        fn write(&self, _content: &ClipboardContent, transient: bool) -> Result<()> {
            assert!(transient, "a paste always writes transient content");
            self.log(Call::Write);
            if self.fail_write {
                Err(err("write"))
            } else {
                Ok(())
            }
        }

        fn restore(&self, _snapshot: ClipboardSnapshot) -> Result<()> {
            self.log(Call::Restore);
            let mut left = self
                .restore_failures
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if *left > 0 {
                *left -= 1;
                Err(err("restore"))
            } else {
                Ok(())
            }
        }
    }

    fn run(clipboard: &Scripted, press_fails: bool) -> (Result<()>, Vec<Call>) {
        let mut press = || {
            clipboard.log(Call::Press);
            if press_fails {
                Err(err("CGEventPost"))
            } else {
                Ok(())
            }
        };
        let mut sleep = |d: Duration| clipboard.log(Call::Sleep(d.as_millis()));
        let result = paste_with_restore(
            clipboard,
            &ClipboardContent::text("x"),
            Duration::from_millis(250),
            &mut press,
            &mut sleep,
        );
        (result, clipboard.calls())
    }

    #[test]
    fn the_happy_path_waits_then_restores() {
        let (result, calls) = run(&Scripted::default(), false);
        assert_eq!(result, Ok(()));
        assert_eq!(
            calls,
            [
                Call::Snapshot,
                Call::Write,
                Call::Press,
                Call::Sleep(250),
                Call::Restore
            ]
        );
    }

    #[test]
    fn without_a_snapshot_nothing_is_written() {
        let clipboard = Scripted {
            fail_snapshot: true,
            ..Scripted::default()
        };
        let (result, calls) = run(&clipboard, false);
        assert_eq!(result, Err(err("snapshot")));
        assert_eq!(calls, [Call::Snapshot]);
    }

    #[test]
    fn an_incomplete_snapshot_writes_nothing() {
        let clipboard = Scripted {
            incomplete: true,
            ..Scripted::default()
        };
        let (result, calls) = run(&clipboard, false);
        assert_eq!(result, Err(SNAPSHOT_INCOMPLETE));
        assert_eq!(calls, [Call::Snapshot]);
    }

    #[test]
    fn a_failed_write_restores_at_once() {
        let clipboard = Scripted {
            fail_write: true,
            ..Scripted::default()
        };
        let (result, calls) = run(&clipboard, false);
        assert_eq!(result, Err(err("write")));
        assert_eq!(calls, [Call::Snapshot, Call::Write, Call::Restore]);
    }

    #[test]
    fn a_failed_shortcut_restores_at_once() {
        let (result, calls) = run(&Scripted::default(), true);
        assert_eq!(result, Err(err("CGEventPost")));
        assert_eq!(
            calls,
            [Call::Snapshot, Call::Write, Call::Press, Call::Restore]
        );
    }

    #[test]
    fn a_failed_restore_is_tried_again_once() {
        let clipboard = Scripted {
            restore_failures: Mutex::new(1),
            ..Scripted::default()
        };
        let (result, calls) = run(&clipboard, false);
        assert_eq!(result, Ok(()));
        assert_eq!(
            &calls[3..],
            [
                Call::Sleep(250),
                Call::Restore,
                Call::Sleep(RETRY_DELAY.as_millis()),
                Call::Restore
            ]
        );
        let clipboard = Scripted {
            restore_failures: Mutex::new(2),
            ..Scripted::default()
        };
        let (result, _) = run(&clipboard, false);
        assert_eq!(result, Err(err("restore")));
    }

    #[test]
    fn with_the_mock_the_previous_content_comes_back_and_history_stays_clean() {
        let mock = MockPlatform::new();
        let before = ClipboardContent::rich("user copy", "<b>user copy</b>");
        mock.copy(before.clone());
        let mut pasted_while_on_clipboard = None;
        let mut press = || {
            pasted_while_on_clipboard = mock.clipboard().text;
            Ok(())
        };
        let result = paste_with_restore(
            &mock,
            &ClipboardContent::text("snippet"),
            Duration::ZERO,
            &mut press,
            &mut |_| {},
        );
        assert_eq!(result, Ok(()));
        assert_eq!(pasted_while_on_clipboard.as_deref(), Some("snippet"));
        assert_eq!(mock.clipboard(), before);
        assert_eq!(
            mock.clipboard_history(),
            [before],
            "the paste left no history"
        );
    }
}
