//! The clipboard (architecture §5, §8 rules 2 and 8).
//!
//! Two users: the `{clipboard}` token (the service reads the text at the match, through
//! [`ClipboardAccess::read_text`]) and paste injection, which runs
//! [`snapshot`](ClipboardAccess::snapshot) → [`write`](ClipboardAccess::write) with
//! `transient` → paste shortcut → [`restore`](ClipboardAccess::restore).
//!
//! Clipboard content is the user's own words. Every type here that holds it has a `Debug` that
//! shows sizes only, and the snapshot keeps its bytes in [`Sensitive`].

use std::fmt;
use std::path::PathBuf;

use iamb_core::sensitive::Sensitive;

use crate::error::Result;

/// What Iamb puts on the clipboard to paste: the same content in up to three flavours.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ClipboardContent {
    /// Plain text.
    pub text: Option<String>,
    /// An HTML fragment (rich snippets; images embedded as data URIs).
    pub html: Option<String>,
    /// Files, offered as a file list (a snippet that is one media file).
    pub files: Vec<PathBuf>,
}

impl ClipboardContent {
    /// Plain text only.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// Plain text with an HTML version.
    #[must_use]
    pub fn rich(text: impl Into<String>, html: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            html: Some(html.into()),
            files: Vec::new(),
        }
    }

    /// True when there is nothing to put on the clipboard.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.files.is_empty()
    }
}

impl fmt::Debug for ClipboardContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardContent")
            .field("text_len", &self.text.as_ref().map(String::len))
            .field("html_len", &self.html.as_ref().map(String::len))
            .field("files", &self.files.len())
            .finish()
    }
}

/// The OS name of a clipboard flavour.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClipboardFormat {
    /// A numeric format (a Windows clipboard format).
    Id(u32),
    /// A named format (a macOS pasteboard type, or a registered Windows format).
    Name(String),
}

/// One flavour of saved clipboard content.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipboardFlavour {
    /// The format.
    pub format: ClipboardFormat,
    /// The bytes, exactly as the OS handed them out.
    pub data: Sensitive<Vec<u8>>,
}

impl fmt::Debug for ClipboardFlavour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardFlavour")
            .field("format", &self.format)
            .field("len", &self.data.expose().len())
            .finish()
    }
}

/// Everything that was on the clipboard, to put back after a paste.
///
/// Opaque to the service: only the [`ClipboardAccess`] that made a snapshot can restore it.
/// An implementation may leave out flavours it cannot copy (formats rendered on demand, or
/// larger than its limit) and says so with [`is_complete`](Self::is_complete).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ClipboardSnapshot {
    flavours: Vec<ClipboardFlavour>,
    complete: bool,
}

impl ClipboardSnapshot {
    /// A snapshot of `flavours`; `complete` is false when some flavour was left out.
    #[must_use]
    pub const fn new(flavours: Vec<ClipboardFlavour>, complete: bool) -> Self {
        Self { flavours, complete }
    }

    /// The saved flavours.
    #[must_use]
    pub fn flavours(&self) -> &[ClipboardFlavour] {
        &self.flavours
    }

    /// False when the clipboard held something the snapshot could not copy.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    /// True when the clipboard was empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.flavours.is_empty()
    }
}

impl fmt::Debug for ClipboardSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardSnapshot")
            .field("flavours", &self.flavours.len())
            .field("complete", &self.complete)
            .finish()
    }
}

/// Reads and writes the system clipboard.
pub trait ClipboardAccess: Send + Sync {
    /// The clipboard's plain text, if it holds any (`{clipboard}`, read once at the match).
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the clipboard cannot be opened (another program
    /// holds it); the service then treats the clipboard as empty.
    fn read_text(&self) -> Result<Option<Sensitive<String>>>;

    /// Saves everything on the clipboard.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the clipboard cannot be opened.
    fn snapshot(&self) -> Result<ClipboardSnapshot>;

    /// Replaces the clipboard with `content`. With `transient`, the content carries the
    /// markers that keep it out of clipboard history and cloud clipboard services (§8 rule 8):
    /// always set for a paste.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the clipboard cannot be opened or written.
    fn write(&self, content: &ClipboardContent, transient: bool) -> Result<()>;

    /// Puts a snapshot back, also without a history entry.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the clipboard cannot be opened or written.
    fn restore(&self, snapshot: ClipboardSnapshot) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors() {
        let plain = ClipboardContent::text("hello");
        assert_eq!(plain.text.as_deref(), Some("hello"));
        assert!(plain.html.is_none() && plain.files.is_empty());
        assert!(!plain.is_empty());
        let rich = ClipboardContent::rich("hi", "<b>hi</b>");
        assert_eq!(rich.html.as_deref(), Some("<b>hi</b>"));
        assert!(ClipboardContent::default().is_empty());
        let file = ClipboardContent {
            files: vec![PathBuf::from("/tmp/a.png")],
            ..ClipboardContent::default()
        };
        assert!(!file.is_empty());
    }

    #[test]
    fn debug_shows_sizes_only() {
        let content = ClipboardContent::rich("top secret", "<i>top secret</i>");
        let shown = format!("{content:?}");
        assert_eq!(
            shown,
            "ClipboardContent { text_len: Some(10), html_len: Some(17), files: 0 }"
        );
        let flavour = ClipboardFlavour {
            format: ClipboardFormat::Name("public.utf8-plain-text".to_owned()),
            data: Sensitive::new(b"top secret".to_vec()),
        };
        let shown = format!("{flavour:?}");
        assert!(shown.contains("len: 10") && !shown.contains("secret"));
        let snapshot = ClipboardSnapshot::new(vec![flavour], true);
        assert_eq!(
            format!("{snapshot:?}"),
            "ClipboardSnapshot { flavours: 1, complete: true }"
        );
    }

    #[test]
    fn snapshot_accessors() {
        let empty = ClipboardSnapshot::default();
        assert!(empty.is_empty() && !empty.is_complete());
        let snapshot = ClipboardSnapshot::new(
            vec![ClipboardFlavour {
                format: ClipboardFormat::Id(13),
                data: Sensitive::new(vec![1, 2]),
            }],
            false,
        );
        assert!(!snapshot.is_empty() && !snapshot.is_complete());
        assert_eq!(snapshot.flavours()[0].format, ClipboardFormat::Id(13));
    }
}
