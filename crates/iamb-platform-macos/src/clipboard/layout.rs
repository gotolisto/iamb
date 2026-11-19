//! How pasteboard items map to the platform's flat clipboard types, and what Iamb writes.
//!
//! # Snapshots
//!
//! The general pasteboard holds items, each with several types (UTIs) of data.
//! [`ClipboardSnapshot`] is a flat list of flavours, so a snapshot is encoded as:
//!
//! | Flavour | Format | Data |
//! | --- | --- | --- |
//! | start of an item | `Id(ITEM_START)` | empty |
//! | one type of the item | `Name(uti)` | the bytes, exactly as the pasteboard gave them |
//! | a type that could not be read | `Id(UNREADABLE)` | the type's name in UTF-8 (a UTI, never content) |
//!
//! A type is unreadable when the pasteboard gives no data for it (a promise the owner did not
//! keep) or when keeping it would pass [`SNAPSHOT_LIMIT`]; the snapshot is then incomplete
//! ([`ClipboardSnapshot::is_complete`]) and restoring brings back everything else.
//!
//! # What Iamb writes
//!
//! [`content_items`]: one item with plain text (`public.utf8-plain-text`) and HTML
//! (`public.html`), when either is present, then one item per file (`public.file-url`). With
//! `transient`, every item also carries the marker types of nspasteboard.org that clipboard
//! managers and history tools honour: [`TRANSIENT_TYPE`] (do not record) and
//! [`CONCEALED_TYPE`] (do not show), with empty data.
//!
//! [`restore_items`]: the saved items, type for type and byte for byte, each with
//! [`TRANSIENT_TYPE`] added when it did not have it, so that putting the user's own content
//! back does not record it in their clipboard history a second time.
//!
//! # This device only
//!
//! The markers above are for clipboard managers. The system's own clipboard, which offers
//! the pasteboard to the user's other devices, does not read them; it honours the "current
//! host only" option of the write instead ([`Reach`], §8 rule 8; the 2.8 review's finding
//! 7). Transient content is always written with it. A restore uses it when the saved content
//! carried either marker ([`restore_reach`]): the application that copied it asked for it to
//! be kept out of sight, and a password it copied must not become fetchable from another
//! device because Iamb put it back. Other content is put back as it was, offered to the
//! other devices again.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;

use iamb_platform::core_types::Sensitive;
use iamb_platform::{ClipboardContent, ClipboardFlavour, ClipboardFormat, ClipboardSnapshot};

/// Plain text, UTF-8.
pub(crate) const PLAIN_TEXT_TYPE: &str = "public.utf8-plain-text";
/// An HTML fragment, UTF-8.
pub(crate) const HTML_TYPE: &str = "public.html";
/// A file URL, UTF-8.
pub(crate) const FILE_URL_TYPE: &str = "public.file-url";
/// "Do not record this in a clipboard history" (nspasteboard.org).
pub(crate) const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";
/// "This is sensitive: do not show it" (nspasteboard.org).
pub(crate) const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";

/// The flavour that starts an item ("IB" and 1).
pub(crate) const ITEM_START: u32 = 0x4942_0001;
/// The flavour that records a type that could not be read ("IB" and 2).
pub(crate) const UNREADABLE: u32 = 0x4942_0002;

/// The most bytes a snapshot keeps. Larger types (a big image) are left out and the snapshot
/// is incomplete.
pub(crate) const SNAPSHOT_LIMIT: usize = 64 * 1024 * 1024;

/// Which devices a write is offered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The user's other devices too, as any copy is.
    AnyDevice,
    /// This device only (`NSPasteboardContentsCurrentHostOnly`).
    ThisDevice,
}

/// The reach of a write of new content: transient content stays on this device.
pub(crate) const fn content_reach(transient: bool) -> Reach {
    if transient {
        Reach::ThisDevice
    } else {
        Reach::AnyDevice
    }
}

/// The reach of a restore of `saved`: this device only when any item carried the transient
/// or the concealed marker; see the module docs.
pub(crate) fn restore_reach(saved: &[SavedItem<'_>]) -> Reach {
    let marked = saved.iter().any(|item| {
        item.types
            .iter()
            .map(|(uti, _)| *uti)
            .chain(item.unreadable.iter().copied())
            .any(|uti| uti == TRANSIENT_TYPE || uti == CONCEALED_TYPE)
    });
    content_reach(marked)
}

/// One item as the pasteboard handed it out: each type with its data, `None` when the
/// pasteboard gave none. No `Debug`: it holds the user's content.
pub(crate) struct RawItem {
    pub(crate) types: Vec<(String, Option<Vec<u8>>)>,
}

/// Builds a [`ClipboardSnapshot`] from what the pasteboard handed out.
pub(crate) fn snapshot_from(items: Vec<RawItem>, limit: usize) -> ClipboardSnapshot {
    let mut flavours = Vec::new();
    let mut kept = 0usize;
    let mut complete = true;
    for item in items {
        flavours.push(ClipboardFlavour {
            format: ClipboardFormat::Id(ITEM_START),
            data: Sensitive::new(Vec::new()),
        });
        for (uti, data) in item.types {
            match data {
                Some(bytes) if kept.saturating_add(bytes.len()) <= limit => {
                    kept += bytes.len();
                    flavours.push(ClipboardFlavour {
                        format: ClipboardFormat::Name(uti),
                        data: Sensitive::new(bytes),
                    });
                }
                Some(mut bytes) => {
                    bytes.fill(0);
                    complete = false;
                    flavours.push(unreadable(uti));
                }
                None => {
                    complete = false;
                    flavours.push(unreadable(uti));
                }
            }
        }
    }
    ClipboardSnapshot::new(flavours, complete)
}

fn unreadable(uti: String) -> ClipboardFlavour {
    ClipboardFlavour {
        format: ClipboardFormat::Id(UNREADABLE),
        data: Sensitive::new(uti.into_bytes()),
    }
}

/// One saved item, borrowed from a snapshot.
pub(crate) struct SavedItem<'a> {
    /// Each type with its bytes, in the order the pasteboard listed them.
    pub(crate) types: Vec<(&'a str, &'a [u8])>,
    /// The types that could not be read.
    pub(crate) unreadable: Vec<&'a str>,
}

/// The items of a snapshot, in order. Flavours before the first item marker (a snapshot made
/// elsewhere) form one item; numeric formats other than Iamb's markers are skipped.
pub(crate) fn items(snapshot: &ClipboardSnapshot) -> Vec<SavedItem<'_>> {
    let mut out: Vec<SavedItem<'_>> = Vec::new();
    for flavour in snapshot.flavours() {
        let starts = matches!(flavour.format, ClipboardFormat::Id(ITEM_START));
        if starts || out.is_empty() {
            out.push(SavedItem {
                types: Vec::new(),
                unreadable: Vec::new(),
            });
            if starts {
                continue;
            }
        }
        let Some(item) = out.last_mut() else {
            continue;
        };
        match &flavour.format {
            ClipboardFormat::Name(uti) => item.types.push((uti, flavour.data.expose())),
            ClipboardFormat::Id(UNREADABLE) => {
                if let Ok(uti) = std::str::from_utf8(flavour.data.expose()) {
                    item.unreadable.push(uti);
                }
            }
            ClipboardFormat::Id(_) => {}
        }
    }
    out
}

/// One item to put on the pasteboard: types with their data.
#[derive(PartialEq, Eq)]
pub(crate) struct WriteItem<'a> {
    pub(crate) types: Vec<(&'a str, Cow<'a, [u8]>)>,
}

impl WriteItem<'_> {
    fn has(&self, uti: &str) -> bool {
        self.types.iter().any(|(t, _)| *t == uti)
    }

    fn mark(&mut self, uti: &'static str) {
        if !self.has(uti) {
            self.types.push((uti, Cow::Borrowed(&[])));
        }
    }
}

/// The items for `content`; see the module docs.
pub(crate) fn content_items(content: &ClipboardContent, transient: bool) -> Vec<WriteItem<'_>> {
    let mut items = Vec::new();
    let mut first = WriteItem { types: Vec::new() };
    if let Some(text) = &content.text {
        first
            .types
            .push((PLAIN_TEXT_TYPE, Cow::Borrowed(text.as_bytes())));
    }
    if let Some(html) = &content.html {
        first
            .types
            .push((HTML_TYPE, Cow::Borrowed(html.as_bytes())));
    }
    if !first.types.is_empty() {
        items.push(first);
    }
    for path in &content.files {
        items.push(WriteItem {
            types: vec![(FILE_URL_TYPE, Cow::Owned(file_url(path).into_bytes()))],
        });
    }
    if transient {
        for item in &mut items {
            item.mark(TRANSIENT_TYPE);
            item.mark(CONCEALED_TYPE);
        }
    }
    items
}

/// The items to put a snapshot back; see the module docs.
pub(crate) fn restore_items<'a>(saved: &[SavedItem<'a>]) -> Vec<WriteItem<'a>> {
    saved
        .iter()
        .filter(|item| !item.types.is_empty())
        .map(|item| {
            let mut write = WriteItem {
                types: item
                    .types
                    .iter()
                    .map(|(uti, data)| (*uti, Cow::Borrowed(*data)))
                    .collect(),
            };
            write.mark(TRANSIENT_TYPE);
            write
        })
        .collect()
}

/// `file://` and the absolute path, percent-encoded as RFC 3986 requires for a path.
pub(crate) fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.first() != Some(&b'/') {
        url.push('/');
    }
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b) {
            url.push(char::from(b));
        } else {
            let _ = write!(url, "%{b:02X}");
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn raw(types: &[(&str, Option<&[u8]>)]) -> RawItem {
        RawItem {
            types: types
                .iter()
                .map(|(t, d)| ((*t).to_owned(), d.map(<[u8]>::to_vec)))
                .collect(),
        }
    }

    #[test]
    fn a_snapshot_keeps_every_item_and_type_in_order() {
        let snapshot = snapshot_from(
            vec![
                raw(&[
                    (PLAIN_TEXT_TYPE, Some(b"hello")),
                    (HTML_TYPE, Some(b"<b>hello</b>")),
                    ("com.example.private", Some(&[0, 1, 2, 255])),
                ]),
                raw(&[
                    (FILE_URL_TYPE, Some(b"file:///tmp/a.png")),
                    ("public.png", Some(&[])),
                ]),
            ],
            SNAPSHOT_LIMIT,
        );
        assert!(snapshot.is_complete());
        let saved = items(&snapshot);
        assert_eq!(saved.len(), 2);
        assert_eq!(
            saved[0].types,
            [
                (PLAIN_TEXT_TYPE, b"hello".as_slice()),
                (HTML_TYPE, b"<b>hello</b>".as_slice()),
                ("com.example.private", [0u8, 1, 2, 255].as_slice()),
            ]
        );
        assert_eq!(saved[1].types[1], ("public.png", [].as_slice()));
        assert!(saved.iter().all(|item| item.unreadable.is_empty()));
    }

    #[test]
    fn unreadable_and_oversized_types_are_recorded_and_make_it_incomplete() {
        let snapshot = snapshot_from(
            vec![raw(&[
                (PLAIN_TEXT_TYPE, Some(b"abc")),
                ("com.example.promised", None),
                ("public.tiff", Some(&[7; 10])),
                (HTML_TYPE, Some(b"<p>")),
            ])],
            6,
        );
        assert!(!snapshot.is_complete());
        let saved = items(&snapshot);
        assert_eq!(
            saved[0].types,
            [
                (PLAIN_TEXT_TYPE, b"abc".as_slice()),
                (HTML_TYPE, b"<p>".as_slice())
            ]
        );
        assert_eq!(saved[0].unreadable, ["com.example.promised", "public.tiff"]);
        // Restoring brings back what was read, with the history marker.
        let restore = restore_items(&saved);
        assert_eq!(restore.len(), 1);
        let types: Vec<&str> = restore[0].types.iter().map(|(t, _)| *t).collect();
        assert_eq!(types, [PLAIN_TEXT_TYPE, HTML_TYPE, TRANSIENT_TYPE]);
    }

    #[test]
    fn an_empty_pasteboard_snapshots_and_restores_as_nothing() {
        let snapshot = snapshot_from(Vec::new(), SNAPSHOT_LIMIT);
        assert!(snapshot.is_empty() && snapshot.is_complete());
        assert!(items(&snapshot).is_empty());
        assert!(restore_items(&items(&snapshot)).is_empty());
        // An item whose every type was unreadable is not written back as an empty item.
        let snapshot = snapshot_from(vec![raw(&[("x.y", None)])], SNAPSHOT_LIMIT);
        assert!(restore_items(&items(&snapshot)).is_empty());
    }

    #[test]
    fn restore_is_byte_for_byte_and_marks_once() {
        let snapshot = snapshot_from(
            vec![
                raw(&[(PLAIN_TEXT_TYPE, Some(b"one")), (TRANSIENT_TYPE, Some(b""))]),
                raw(&[("public.rtf", Some(b"{\\rtf1}"))]),
            ],
            SNAPSHOT_LIMIT,
        );
        let saved = items(&snapshot);
        let restore = restore_items(&saved);
        assert_eq!(restore.len(), 2);
        assert_eq!(
            restore[0].types,
            [
                (PLAIN_TEXT_TYPE, Cow::Borrowed(b"one".as_slice())),
                (TRANSIENT_TYPE, Cow::Borrowed(b"".as_slice()))
            ]
        );
        assert_eq!(
            restore[1].types,
            [
                ("public.rtf", Cow::Borrowed(b"{\\rtf1}".as_slice())),
                (TRANSIENT_TYPE, Cow::Borrowed(b"".as_slice()))
            ]
        );
    }

    #[test]
    fn foreign_snapshots_are_one_item() {
        let snapshot = ClipboardSnapshot::new(
            vec![
                ClipboardFlavour {
                    format: ClipboardFormat::Name(PLAIN_TEXT_TYPE.to_owned()),
                    data: Sensitive::new(b"x".to_vec()),
                },
                ClipboardFlavour {
                    format: ClipboardFormat::Id(13),
                    data: Sensitive::new(b"y".to_vec()),
                },
            ],
            true,
        );
        let saved = items(&snapshot);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].types, [(PLAIN_TEXT_TYPE, b"x".as_slice())]);
    }

    #[test]
    fn transient_writes_carry_both_markers_on_every_item() {
        let content = ClipboardContent {
            text: Some("Café 👋🏽".to_owned()),
            html: Some("<b>Café</b>".to_owned()),
            files: vec![PathBuf::from("/tmp/a b/ü.png"), PathBuf::from("/tmp/c.mov")],
        };
        let written = content_items(&content, true);
        assert_eq!(written.len(), 3);
        for item in &written {
            assert!(item.has(TRANSIENT_TYPE) && item.has(CONCEALED_TYPE));
        }
        assert_eq!(
            written[0].types[0],
            (PLAIN_TEXT_TYPE, Cow::Borrowed("Café 👋🏽".as_bytes()))
        );
        assert_eq!(
            written[0].types[1],
            (HTML_TYPE, Cow::Borrowed("<b>Café</b>".as_bytes()))
        );
        assert_eq!(
            written[1].types[0],
            (
                FILE_URL_TYPE,
                Cow::Owned(b"file:///tmp/a%20b/%C3%BC.png".to_vec())
            )
        );
        // Not transient: no markers.
        let x = ClipboardContent::text("x");
        let plain = content_items(&x, false);
        assert_eq!(plain.len(), 1);
        assert!(!plain[0].has(TRANSIENT_TYPE) && !plain[0].has(CONCEALED_TYPE));
        // Nothing to write: no items.
        assert!(content_items(&ClipboardContent::default(), true).is_empty());
    }

    #[test]
    fn file_urls() {
        assert_eq!(file_url(Path::new("/a/b.txt")), "file:///a/b.txt");
        assert_eq!(file_url(Path::new("rel")), "file:///rel");
        assert_eq!(file_url(Path::new("/50%#?")), "file:///50%25%23%3F");
    }
}
