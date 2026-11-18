//! The clipboard against the real general pasteboard: snapshot, write and restore round trips
//! byte for byte, the transient and concealed markers on Iamb's writes, and the change-count
//! rule. Needs no permission, so it runs on a hosted macOS runner.
//!
//! The tests share one pasteboard, so they take turns ([`Desk`]), and each puts back what the
//! pasteboard held before it started, so running them on a developer's Mac leaves the
//! clipboard as it was. The pasteboard is set up and read back here directly through `AppKit`,
//! not through the code under test.

#![cfg(target_os = "macos")]

use std::sync::{Mutex, MutexGuard, PoisonError};

use iamb_platform::{ClipboardAccess, ClipboardContent};
use iamb_platform_macos::Platform;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};

const PLAIN: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
const TRANSIENT: &str = "org.nspasteboard.TransientType";
const CONCEALED: &str = "org.nspasteboard.ConcealedType";

/// One pasteboard item: types and data, in order.
type Item = Vec<(String, Vec<u8>)>;

fn general() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

/// Replaces the pasteboard with `items`, as another application would.
fn put(items: &[Item]) {
    autoreleasepool(|_| {
        let board = general();
        board.clearContents();
        if items.is_empty() {
            return;
        }
        let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
            .iter()
            .map(|item| {
                let written = NSPasteboardItem::new();
                for (uti, data) in item {
                    assert!(
                        written
                            .setData_forType(&NSData::with_bytes(data), &NSString::from_str(uti)),
                        "set {uti}"
                    );
                }
                ProtocolObject::from_retained(written)
            })
            .collect();
        assert!(board.writeObjects(&NSArray::from_retained_slice(&objects)));
    });
}

/// Every item and type on the pasteboard, with its data.
fn read() -> Vec<Item> {
    autoreleasepool(|_| {
        general()
            .pasteboardItems()
            .map(|items| items.to_vec())
            .unwrap_or_default()
            .into_iter()
            .map(|item| {
                item.types()
                    .to_vec()
                    .into_iter()
                    .map(|uti| {
                        let data = item
                            .dataForType(&uti)
                            .map(|d| d.to_vec())
                            .unwrap_or_default();
                        (uti.to_string(), data)
                    })
                    .collect()
            })
            .collect()
    })
}

fn change_count() -> isize {
    autoreleasepool(|_| general().changeCount())
}

fn types(item: &Item) -> Vec<&str> {
    item.iter().map(|(t, _)| t.as_str()).collect()
}

/// The items without the history marker that a restore adds.
fn without_restore_marker(items: Vec<Item>) -> Vec<Item> {
    items
        .into_iter()
        .map(|item| item.into_iter().filter(|(t, _)| t != TRANSIENT).collect())
        .collect()
}

/// One test at a time on the pasteboard; puts back what was there when dropped.
struct Desk {
    before: Vec<Item>,
    _turn: MutexGuard<'static, ()>,
}

static TURN: Mutex<()> = Mutex::new(());

impl Desk {
    fn take() -> Self {
        let turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
        Self {
            before: read(),
            _turn: turn,
        }
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        put(&self.before);
    }
}

/// Several items with several types, including binary data with every byte value and a type
/// no system converter knows.
fn rich_items() -> Vec<Item> {
    let every_byte: Vec<u8> = (0..=255).collect();
    let big: Vec<u8> = (0..1_000_000u32)
        .map(|i| u8::try_from(i % 251).expect("below 251"))
        .collect();
    vec![
        vec![
            (
                PLAIN.to_owned(),
                "Grüße, 日本語 👋🏽 👩‍💻 🇯🇵".as_bytes().to_vec(),
            ),
            (
                HTML.to_owned(),
                b"<p><b>Gr\xc3\xbc\xc3\x9fe</b></p>".to_vec(),
            ),
            ("com.example.iamb-test.binary".to_owned(), every_byte),
        ],
        vec![
            ("com.example.iamb-test.big".to_owned(), big),
            ("com.example.iamb-test.empty".to_owned(), Vec::new()),
        ],
        vec![(PLAIN.to_owned(), b"third item".to_vec())],
    ]
}

#[test]
fn snapshot_write_restore_round_trips_byte_for_byte_fifty_times() {
    let _desk = Desk::take();
    let platform = Platform::new();
    put(&rich_items());
    let original = read();
    // What was put is what the pasteboard reports (the custom types exactly).
    for (got, want) in original.iter().zip(rich_items()) {
        for (uti, data) in want {
            assert!(
                got.iter().any(|(t, d)| *t == uti && *d == data),
                "{uti} is on the pasteboard as written"
            );
        }
    }
    for round in 0..50 {
        let snapshot = platform.snapshot().expect("snapshot");
        assert!(snapshot.is_complete(), "round {round}: every type was read");
        platform
            .write(
                &ClipboardContent::rich(format!("pasted {round}"), "<i>pasted</i>"),
                true,
            )
            .expect("write");
        assert_eq!(
            platform
                .read_text()
                .expect("read")
                .map(iamb_platform::core_types::Sensitive::into_inner),
            Some(format!("pasted {round}"))
        );
        platform.restore(snapshot).expect("restore");
        let restored = read();
        assert_eq!(
            without_restore_marker(restored.clone()),
            without_restore_marker(original.clone()),
            "round {round}: every item and type is back, byte for byte"
        );
        for item in &restored {
            assert!(
                types(item).contains(&TRANSIENT),
                "the restore stays out of history"
            );
            assert!(!types(item).contains(&CONCEALED));
        }
    }
}

#[test]
fn iambs_writes_carry_the_transient_and_concealed_markers() {
    let _desk = Desk::take();
    let platform = Platform::new();
    platform
        .write(
            &ClipboardContent {
                text: Some("Café 🇫🇷".to_owned()),
                html: Some("<b>Café</b>".to_owned()),
                files: vec!["/tmp/iamb test.png".into()],
            },
            true,
        )
        .expect("write");
    let items = read();
    assert_eq!(items.len(), 2, "the text item and the file item");
    for item in &items {
        assert!(types(item).contains(&TRANSIENT) && types(item).contains(&CONCEALED));
    }
    let first = &items[0];
    assert!(first.contains(&(PLAIN.to_owned(), "Café 🇫🇷".as_bytes().to_vec())));
    assert!(first.contains(&(HTML.to_owned(), b"<b>Caf\xc3\xa9</b>".to_vec())));
    assert!(items[1].contains(&(
        "public.file-url".to_owned(),
        b"file:///tmp/iamb%20test.png".to_vec()
    )));

    // A plain write has no markers.
    platform
        .write(&ClipboardContent::text("plain"), false)
        .expect("write");
    let items = read();
    assert_eq!(items.len(), 1);
    assert!(!types(&items[0]).contains(&TRANSIENT) && !types(&items[0]).contains(&CONCEALED));
}

#[test]
fn a_copy_made_after_iambs_write_is_not_overwritten() {
    let _desk = Desk::take();
    let platform = Platform::new();
    put(&[vec![(PLAIN.to_owned(), b"before".to_vec())]]);
    let snapshot = platform.snapshot().expect("snapshot");
    platform
        .write(&ClipboardContent::text("pasted"), true)
        .expect("write");
    let after_write = change_count();
    // The user copies something while the paste is in flight.
    let newer = vec![vec![(PLAIN.to_owned(), b"newer copy".to_vec())]];
    put(&newer);
    assert_ne!(change_count(), after_write);
    platform
        .restore(snapshot.clone())
        .expect("restore succeeds");
    assert_eq!(read(), newer, "the newer copy stays");

    // Without an intervening copy the snapshot comes back.
    platform
        .write(&ClipboardContent::text("pasted again"), true)
        .expect("write");
    platform.restore(snapshot).expect("restore");
    assert_eq!(
        without_restore_marker(read()),
        [vec![(PLAIN.to_owned(), b"before".to_vec())]]
    );
}

#[test]
fn an_empty_pasteboard_comes_back_empty() {
    let _desk = Desk::take();
    let platform = Platform::new();
    put(&[]);
    let snapshot = platform.snapshot().expect("snapshot");
    assert!(snapshot.is_empty() && snapshot.is_complete());
    assert_eq!(platform.read_text().map(|t| t.is_none()), Ok(true));
    platform
        .write(&ClipboardContent::text("x"), true)
        .expect("write");
    platform.restore(snapshot).expect("restore");
    assert!(read().is_empty());
}

#[test]
fn debug_output_shows_no_content() {
    let _desk = Desk::take();
    let platform = Platform::new();
    put(&[vec![(PLAIN.to_owned(), b"top secret words".to_vec())]]);
    let snapshot = platform.snapshot().expect("snapshot");
    let shown = format!("{snapshot:?} {platform:?}");
    assert!(!shown.contains("secret"), "{shown}");
}
