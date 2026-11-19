//! The general pasteboard behind [`Board`], and `ClipboardAccess` for [`Platform`].
//!
//! Every call runs in its own autorelease pool, so it can be made from any thread (the engine
//! thread for `{clipboard}`, the injector thread for a paste) without leaking the objects the
//! pasteboard hands out.

use iamb_platform::core_types::Sensitive;
use iamb_platform::{ClipboardAccess, ClipboardContent, ClipboardSnapshot, PlatformError, Result};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardContentsOptions, NSPasteboardItem, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSString};

use super::board::{self, Board, Rules};
use super::layout::{PLAIN_TEXT_TYPE, RawItem, Reach, WriteItem};
use crate::Platform;

/// The clipboard's state: the change-count ledger.
#[derive(Debug, Default)]
pub(crate) struct State {
    rules: Rules,
}

/// The general pasteboard.
struct General(Retained<NSPasteboard>);

impl General {
    fn new() -> Self {
        Self(NSPasteboard::generalPasteboard())
    }
}

impl Board for General {
    fn change_count(&self) -> isize {
        self.0.changeCount()
    }

    fn items(&self) -> Result<Vec<RawItem>> {
        let items = self
            .0
            .pasteboardItems()
            .ok_or(PlatformError::os(0, "NSPasteboard.pasteboardItems"))?;
        // `to_vec`, not `iter`: iteration needs the `NSEnumerator` feature, which is not
        // enabled, and the arrays are short.
        Ok(items
            .to_vec()
            .into_iter()
            .map(|item| RawItem {
                types: item
                    .types()
                    .to_vec()
                    .into_iter()
                    .map(|uti| {
                        let data = item.dataForType(&uti).map(|data| data.to_vec());
                        (uti.to_string(), data)
                    })
                    .collect(),
            })
            .collect())
    }

    fn plain_text(&self) -> Result<Option<String>> {
        let uti = NSString::from_str(PLAIN_TEXT_TYPE);
        Ok(self.0.stringForType(&uti).map(|text| text.to_string()))
    }

    fn replace(&self, items: &[WriteItem<'_>], reach: Reach) -> Result<()> {
        // Both clear the pasteboard and make this process its owner; the option keeps what
        // follows from being offered to the user's other devices.
        match reach {
            Reach::AnyDevice => self.0.clearContents(),
            Reach::ThisDevice => self
                .0
                .prepareForNewContentsWithOptions(NSPasteboardContentsOptions::CurrentHostOnly),
        };
        if items.is_empty() {
            return Ok(());
        }
        let mut objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            Vec::with_capacity(items.len());
        for item in items {
            let written = NSPasteboardItem::new();
            for (uti, data) in &item.types {
                let data = NSData::with_bytes(data);
                if !written.setData_forType(&data, &NSString::from_str(uti)) {
                    return Err(PlatformError::os(0, "NSPasteboardItem.setData:forType:"));
                }
            }
            objects.push(ProtocolObject::from_retained(written));
        }
        if self.0.writeObjects(&NSArray::from_retained_slice(&objects)) {
            Ok(())
        } else {
            Err(PlatformError::os(0, "NSPasteboard.writeObjects"))
        }
    }
}

impl ClipboardAccess for Platform {
    fn read_text(&self) -> Result<Option<Sensitive<String>>> {
        autoreleasepool(|_| board::read_text(&General::new()))
    }

    fn snapshot(&self) -> Result<ClipboardSnapshot> {
        autoreleasepool(|_| board::snapshot(&General::new()))
    }

    fn write(&self, content: &ClipboardContent, transient: bool) -> Result<()> {
        autoreleasepool(|_| {
            self.clipboard
                .rules
                .write(&General::new(), content, transient)
        })
    }

    fn restore(&self, snapshot: ClipboardSnapshot) -> Result<()> {
        autoreleasepool(|_| {
            self.clipboard
                .rules
                .restore(&General::new(), &snapshot)
                .map(|_| ())
        })
    }
}
