//! A paste with the clipboard put back (architecture §8 rule 8): the order of the steps and
//! what happens when one fails.
//!
//! ```text
//!   snapshot ──▶ write (transient) ──▶ paste shortcut ──▶ wait ──▶ restore
//!      │ fails        │ fails               │ fails                  │ fails
//!      ▼              ▼                     ▼                        ▼
//!   untouched      restore              restore                  retried, then
//!   (error)        (error)              (error)                  "not restored" (error)
//! ```
//!
//! - Nothing is written before the snapshot has succeeded, so a failed snapshot leaves the
//!   clipboard as it was.
//! - From the moment the write may have emptied the clipboard, every path ends with a restore,
//!   which is tried [`RESTORE_ATTEMPTS`] times.
//! - The application reads a paste when it handles the shortcut, some time after the keys
//!   were sent; the clipboard is restored after the strategy's wait. If another program put
//!   something on the clipboard in the meantime (its sequence number moved on since Iamb's
//!   write), that is the user's newer content and is not overwritten: the outcome is
//!   [`Restoration::Superseded`].
//!
//! The steps are a trait, [`PasteHost`], so that every path is tested on the Linux build
//! machine; the Windows layer implements it with the clipboard and `SendInput`.

use iamb_platform::{ClipboardContent, PlatformError};

/// How often a restore is tried before the paste reports that the clipboard was not put back.
pub(crate) const RESTORE_ATTEMPTS: usize = 3;
/// The pause between restore attempts, in milliseconds (on top of the clipboard's own retry).
pub(crate) const RESTORE_RETRY_MS: u32 = 25;

/// The steps of a paste.
pub(crate) trait PasteHost {
    /// What a snapshot holds.
    type Saved;

    /// Saves the clipboard.
    fn snapshot(&mut self) -> Result<Self::Saved, PlatformError>;
    /// Writes the content with the history markers; returns the clipboard's sequence number
    /// after the write.
    fn write_transient(&mut self, content: &ClipboardContent) -> Result<u32, PlatformError>;
    /// Sends the paste shortcut.
    fn press_paste(&mut self) -> Result<(), PlatformError>;
    /// Pauses.
    fn wait(&mut self, ms: u32);
    /// The clipboard's sequence number now.
    fn sequence(&mut self) -> u32;
    /// Puts the saved clipboard back, without a history entry.
    fn restore(&mut self, saved: &Self::Saved) -> Result<(), PlatformError>;
}

/// What became of the user's clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Restoration {
    /// Never changed (the snapshot failed, so nothing was written).
    Untouched,
    /// Put back.
    Restored,
    /// Another program wrote to the clipboard after Iamb did; its content was left there.
    Superseded,
    /// Could not be put back.
    NotRestored(PlatformError),
}

/// How a paste ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PasteReport {
    /// The first step that failed, if any (a failed restore is reported in `clipboard`).
    pub(crate) result: Result<(), PlatformError>,
    /// What became of the user's clipboard.
    pub(crate) clipboard: Restoration,
}

impl PasteReport {
    /// The paste as one result: the first failure, or a failed restore.
    pub(crate) fn into_result(self) -> Result<(), PlatformError> {
        self.result?;
        match self.clipboard {
            Restoration::NotRestored(error) => Err(error),
            Restoration::Untouched | Restoration::Restored | Restoration::Superseded => Ok(()),
        }
    }
}

/// Pastes `content` and puts the clipboard back. See the module docs.
pub(crate) fn paste<H: PasteHost>(
    host: &mut H,
    content: &ClipboardContent,
    wait_ms: u32,
) -> PasteReport {
    let saved = match host.snapshot() {
        Ok(saved) => saved,
        Err(error) => {
            return PasteReport {
                result: Err(error),
                clipboard: Restoration::Untouched,
            };
        }
    };
    let written = match host.write_transient(content) {
        Ok(sequence) => sequence,
        Err(error) => {
            return PasteReport {
                result: Err(error),
                clipboard: restore(host, &saved),
            };
        }
    };
    if let Err(error) = host.press_paste() {
        return PasteReport {
            result: Err(error),
            clipboard: restore(host, &saved),
        };
    }
    host.wait(wait_ms);
    let clipboard = if host.sequence() == written {
        restore(host, &saved)
    } else {
        Restoration::Superseded
    };
    PasteReport {
        result: Ok(()),
        clipboard,
    }
}

fn restore<H: PasteHost>(host: &mut H, saved: &H::Saved) -> Restoration {
    let mut last = None;
    for attempt in 0..RESTORE_ATTEMPTS {
        if attempt > 0 {
            host.wait(RESTORE_RETRY_MS);
        }
        match host.restore(saved) {
            Ok(()) => return Restoration::Restored,
            Err(error) => last = Some(error),
        }
    }
    Restoration::NotRestored(last.unwrap_or(PlatformError::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipboard in memory with scripted failures.
    struct Fake {
        clipboard: String,
        sequence: u32,
        log: Vec<String>,
        fail_snapshot: bool,
        /// The write empties the clipboard, then fails.
        fail_write: bool,
        fail_paste: bool,
        /// How many restores fail before one succeeds.
        failing_restores: usize,
        /// Another program copies something during the wait.
        copy_during_wait: Option<&'static str>,
        /// What the application read when the shortcut arrived.
        pasted: Option<String>,
    }

    impl Fake {
        fn new(clipboard: &str) -> Self {
            Self {
                clipboard: clipboard.to_owned(),
                sequence: 1,
                log: Vec::new(),
                fail_snapshot: false,
                fail_write: false,
                fail_paste: false,
                failing_restores: 0,
                copy_during_wait: None,
                pasted: None,
            }
        }

        fn set(&mut self, text: &str) {
            text.clone_into(&mut self.clipboard);
            self.sequence += 1;
        }
    }

    impl PasteHost for Fake {
        type Saved = String;

        fn snapshot(&mut self) -> Result<String, PlatformError> {
            self.log.push("snapshot".to_owned());
            if self.fail_snapshot {
                return Err(PlatformError::os(5, "OpenClipboard"));
            }
            Ok(self.clipboard.clone())
        }

        fn write_transient(&mut self, content: &ClipboardContent) -> Result<u32, PlatformError> {
            self.log.push("write".to_owned());
            self.set("");
            if self.fail_write {
                return Err(PlatformError::os(8, "SetClipboardData"));
            }
            let text = content.text.clone().unwrap_or_default();
            self.set(&text);
            Ok(self.sequence)
        }

        fn press_paste(&mut self) -> Result<(), PlatformError> {
            self.log.push("paste".to_owned());
            if self.fail_paste {
                return Err(PlatformError::InjectionRefused);
            }
            self.pasted = Some(self.clipboard.clone());
            Ok(())
        }

        fn wait(&mut self, ms: u32) {
            self.log.push(format!("wait {ms}"));
            if let Some(text) = self.copy_during_wait.take() {
                self.set(text);
            }
        }

        fn sequence(&mut self) -> u32 {
            self.sequence
        }

        fn restore(&mut self, saved: &String) -> Result<(), PlatformError> {
            self.log.push("restore".to_owned());
            if self.failing_restores > 0 {
                self.failing_restores -= 1;
                return Err(PlatformError::os(5, "OpenClipboard"));
            }
            let saved = saved.clone();
            self.set(&saved);
            Ok(())
        }
    }

    fn content() -> ClipboardContent {
        ClipboardContent::text("expansion")
    }

    #[test]
    fn the_target_reads_the_paste_and_the_clipboard_comes_back() {
        let mut fake = Fake::new("mine");
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(
            report,
            PasteReport {
                result: Ok(()),
                clipboard: Restoration::Restored
            }
        );
        assert_eq!(fake.pasted.as_deref(), Some("expansion"));
        assert_eq!(fake.clipboard, "mine");
        assert_eq!(
            fake.log,
            ["snapshot", "write", "paste", "wait 150", "restore"]
        );
        assert_eq!(report.into_result(), Ok(()));
    }

    #[test]
    fn a_failed_snapshot_touches_nothing() {
        let mut fake = Fake::new("mine");
        fake.fail_snapshot = true;
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(report.clipboard, Restoration::Untouched);
        assert_eq!(report.result, Err(PlatformError::os(5, "OpenClipboard")));
        assert_eq!(fake.log, ["snapshot"]);
        assert_eq!(fake.clipboard, "mine");
        assert!(fake.pasted.is_none());
    }

    #[test]
    fn a_failed_write_is_restored() {
        let mut fake = Fake::new("mine");
        fake.fail_write = true;
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(report.result, Err(PlatformError::os(8, "SetClipboardData")));
        assert_eq!(report.clipboard, Restoration::Restored);
        assert_eq!(fake.clipboard, "mine");
        assert_eq!(fake.log, ["snapshot", "write", "restore"]);
        assert!(fake.pasted.is_none());
    }

    #[test]
    fn a_refused_shortcut_is_restored() {
        let mut fake = Fake::new("mine");
        fake.fail_paste = true;
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(report.result, Err(PlatformError::InjectionRefused));
        assert_eq!(report.clipboard, Restoration::Restored);
        assert_eq!(fake.clipboard, "mine");
        assert_eq!(fake.log, ["snapshot", "write", "paste", "restore"]);
        assert_eq!(report.into_result(), Err(PlatformError::InjectionRefused));
    }

    #[test]
    fn a_failed_restore_is_retried() {
        let mut fake = Fake::new("mine");
        fake.failing_restores = RESTORE_ATTEMPTS - 1;
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(report.clipboard, Restoration::Restored);
        assert_eq!(fake.clipboard, "mine");
        assert_eq!(
            fake.log,
            [
                "snapshot", "write", "paste", "wait 150", "restore", "wait 25", "restore",
                "wait 25", "restore"
            ]
        );
    }

    #[test]
    fn a_restore_that_keeps_failing_is_reported() {
        let mut fake = Fake::new("mine");
        fake.failing_restores = RESTORE_ATTEMPTS;
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(report.result, Ok(()));
        assert_eq!(
            report.clipboard,
            Restoration::NotRestored(PlatformError::os(5, "OpenClipboard"))
        );
        assert_eq!(
            fake.log.iter().filter(|l| *l == "restore").count(),
            RESTORE_ATTEMPTS
        );
        assert_eq!(
            report.into_result(),
            Err(PlatformError::os(5, "OpenClipboard"))
        );

        // A failed write whose restore fails too reports the write's error first.
        let mut fake = Fake::new("mine");
        fake.fail_write = true;
        fake.failing_restores = RESTORE_ATTEMPTS;
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(
            report.clone().into_result(),
            Err(PlatformError::os(8, "SetClipboardData"))
        );
        assert!(matches!(report.clipboard, Restoration::NotRestored(_)));
    }

    #[test]
    fn a_newer_copy_is_not_overwritten() {
        let mut fake = Fake::new("mine");
        fake.copy_during_wait = Some("copied meanwhile");
        let report = paste(&mut fake, &content(), 150);
        assert_eq!(report.clipboard, Restoration::Superseded);
        assert_eq!(report.result, Ok(()));
        assert_eq!(fake.clipboard, "copied meanwhile");
        assert_eq!(fake.log, ["snapshot", "write", "paste", "wait 150"]);
    }

    #[test]
    fn every_path_that_wrote_ends_restored() {
        // Every combination of failures after the snapshot: the clipboard is back unless the
        // restore itself keeps failing.
        for fail_write in [false, true] {
            for fail_paste in [false, true] {
                for failing_restores in 0..RESTORE_ATTEMPTS {
                    let mut fake = Fake::new("mine");
                    fake.fail_write = fail_write;
                    fake.fail_paste = fail_paste;
                    fake.failing_restores = failing_restores;
                    let report = paste(&mut fake, &content(), 10);
                    assert_eq!(report.clipboard, Restoration::Restored);
                    assert_eq!(fake.clipboard, "mine");
                    assert_eq!(fake.log.last().map(String::as_str), Some("restore"));
                }
            }
        }
    }
}
