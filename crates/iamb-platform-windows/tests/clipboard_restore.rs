//! The clipboard through the public `ClipboardAccess` trait, checked against raw clipboard
//! calls: what Iamb writes (with and without the history markers, the HTML format's header),
//! and snapshots that come back byte for byte. Runs on Windows only (CI's Windows leg); needs
//! no foreground window. The round trip through a paste is in `injector_roundtrip.rs`.
//!
//! A test that needs something the session does not have (a clipboard) says `SKIPPED` and
//! why, and fails instead when `IAMB_REQUIRE_DESKTOP` is set.

#![cfg(windows)]

#[path = "injector_support.rs"]
mod support;

use std::path::PathBuf;

use iamb_platform::{ClipboardAccess, ClipboardContent, ClipboardFormat};
use iamb_platform_windows::Platform;

use support::{CF_DIB, CF_HDROP, CF_UNICODETEXT, Clip, alone, registered, skip, utf16_bytes};

const MARKERS: [&str; 3] = [
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

/// The raw clipboard, or `None` with the reason said.
fn clip() -> Option<Clip> {
    let clip = Clip::open();
    if clip.is_none() {
        skip("this session has no clipboard");
    }
    clip
}

fn markers_present(clip: &Clip) -> Vec<bool> {
    let formats = clip.formats();
    MARKERS
        .iter()
        .map(|m| formats.contains(&registered(m)))
        .collect()
}

#[test]
fn transient_writes_carry_the_history_markers() {
    let _alone = alone();
    let Some(raw) = clip() else { return };
    drop(raw);
    let platform = Platform::new();

    platform
        .write(&ClipboardContent::text("a\nb"), true)
        .expect("write");
    let raw = Clip::open().expect("clipboard");
    assert_eq!(markers_present(&raw), [true, true, true]);
    // History and cloud: a DWORD 0.
    for name in &MARKERS[1..] {
        assert_eq!(raw.get(registered(name)), Some(vec![0, 0, 0, 0]), "{name}");
    }
    // Plain text with CR LF line endings and a NUL.
    assert_eq!(raw.get(CF_UNICODETEXT), Some(utf16_bytes("a\r\nb")));
    drop(raw);

    platform
        .write(&ClipboardContent::text("kept"), false)
        .expect("write");
    let raw = Clip::open().expect("clipboard");
    assert_eq!(markers_present(&raw), [false, false, false]);
    drop(raw);
    // read_text gives LF line endings back.
    platform
        .write(&ClipboardContent::text("one\ntwo"), true)
        .expect("write");
    let text = platform.read_text().expect("read").expect("text");
    assert_eq!(text.expose(), "one\ntwo");
}

/// The four offsets of a `CF_HTML` header.
fn offsets(data: &[u8]) -> [usize; 4] {
    let header = String::from_utf8_lossy(data);
    ["StartHTML:", "EndHTML:", "StartFragment:", "EndFragment:"].map(|key| {
        let at = header.find(key).unwrap_or_else(|| panic!("{key} missing")) + key.len();
        header[at..at + 10].parse().expect("ten digits")
    })
}

#[test]
fn html_goes_on_the_clipboard_with_exact_offsets() {
    let _alone = alone();
    let Some(raw) = clip() else { return };
    drop(raw);
    let platform = Platform::new();
    let fragment = "<p>café <b>👍🏽</b> 漢字</p>";
    platform
        .write(&ClipboardContent::rich("café 👍🏽 漢字", fragment), true)
        .expect("write");
    let raw = Clip::open().expect("clipboard");
    let data = raw.get(registered("HTML Format")).expect("HTML Format");
    let [start_html, end_html, start_fragment, end_fragment] = offsets(&data);
    assert_eq!(&data[start_fragment..end_fragment], fragment.as_bytes());
    assert!(data[start_html..].starts_with(b"<html>"));
    assert!(data[..end_html].ends_with(b"</html>"));
    assert!(data[..start_fragment].ends_with(b"<!--StartFragment-->"));
    assert!(data[end_fragment..].starts_with(b"<!--EndFragment-->"));
    assert_eq!(raw.get(CF_UNICODETEXT), Some(utf16_bytes("café 👍🏽 漢字")));
}

#[test]
fn files_go_on_the_clipboard_as_a_file_list() {
    let _alone = alone();
    let Some(raw) = clip() else { return };
    drop(raw);
    let platform = Platform::new();
    let content = ClipboardContent {
        files: vec![PathBuf::from(r"C:\Temp\é.png")],
        ..ClipboardContent::default()
    };
    platform.write(&content, true).expect("write");
    let raw = Clip::open().expect("clipboard");
    let data = raw.get(CF_HDROP).expect("a file list");
    assert_eq!(&data[0..4], &20u32.to_le_bytes());
    assert_eq!(&data[16..20], &1u32.to_le_bytes());
    let mut expected = utf16_bytes(r"C:\Temp\é.png");
    expected.extend_from_slice(&[0, 0]);
    assert_eq!(&data[20..20 + expected.len()], &expected[..]);
}

#[test]
fn a_snapshot_comes_back_byte_for_byte() {
    let _alone = alone();
    let Some(raw) = clip() else { return };
    let custom = registered("Iamb Snapshot Test");
    // A 1x1 32-bit device-independent bitmap: header and one pixel.
    let mut dib = Vec::new();
    for value in [40u32, 1, 1] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    dib.extend_from_slice(&[0x10, 0x20, 0x30, 0x00]);
    raw.put(&[
        (CF_UNICODETEXT, utf16_bytes("saved ✓")),
        (CF_DIB, dib),
        (custom, b"\x00binary\xffdata".to_vec()),
    ]);
    let baseline: Vec<(u32, Vec<u8>)> = [CF_UNICODETEXT, CF_DIB, custom]
        .iter()
        .map(|f| (*f, raw.get(*f).expect("just put")))
        .collect();
    drop(raw);

    let platform = Platform::new();
    let snapshot = platform.snapshot().expect("snapshot");
    // The system's bitmap made from the DIB is not needed: the snapshot is complete.
    assert!(snapshot.is_complete(), "{snapshot:?}");
    assert!(
        snapshot
            .flavours()
            .iter()
            .any(|f| f.format == ClipboardFormat::Name("Iamb Snapshot Test".to_owned()))
    );
    platform
        .write(&ClipboardContent::text("something else"), true)
        .expect("write");
    platform.restore(snapshot).expect("restore");

    let raw = Clip::open().expect("clipboard");
    for (format, bytes) in &baseline {
        assert_eq!(raw.get(*format).as_ref(), Some(bytes), "format {format}");
    }
    // A restore makes no history entry either.
    assert_eq!(markers_present(&raw), [true, true, true]);
}

#[test]
fn an_empty_clipboard_stays_empty() {
    let _alone = alone();
    let Some(raw) = clip() else { return };
    raw.put(&[]);
    drop(raw);
    let platform = Platform::new();
    assert!(platform.read_text().expect("read").is_none());
    let snapshot = platform.snapshot().expect("snapshot");
    assert!(snapshot.is_empty() && snapshot.is_complete());
    platform
        .write(&ClipboardContent::text("temporary"), true)
        .expect("write");
    platform.restore(snapshot).expect("restore");
    let raw = Clip::open().expect("clipboard");
    assert!(raw.formats().is_empty(), "{:?}", raw.formats());
}

/// `CF_ENHMETAFILE`: its data is a handle to an enhanced metafile.
const CF_ENHMETAFILE: u32 = 14;
/// `CF_METAFILEPICT`: the system makes it from an enhanced metafile.
const CF_METAFILEPICT: u32 = 3;

/// The records of the enhanced metafile on the clipboard, if there is one.
fn metafile_records(_open: &Clip) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::GetEnhMetaFileBits;
    use windows_sys::Win32::System::DataExchange::GetClipboardData;
    // SAFETY: the clipboard is open on this thread (`_open`); the data of `CF_ENHMETAFILE` is
    // a metafile handle, valid while it is open; a null buffer asks for the size, and the
    // second call writes at most the length passed.
    unsafe {
        let handle = GetClipboardData(CF_ENHMETAFILE);
        if handle.is_null() {
            return None;
        }
        let size = GetEnhMetaFileBits(handle, 0, std::ptr::null_mut());
        let mut bytes = vec![0_u8; usize::try_from(size).ok()?];
        let written = GetEnhMetaFileBits(handle, size, bytes.as_mut_ptr());
        (written == size && size > 0).then_some(bytes)
    }
}

/// The follow-up review's N4: an office program's copy leaves a picture of the selection next
/// to its text, as an enhanced metafile (a handle). The snapshot copies its records, so it is
/// complete and a paste can be made; the restore makes the metafile again, and the system
/// offers the metafile picture from it as before.
#[test]
fn a_copy_with_a_picture_of_the_selection_is_saved_and_restored_whole() {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        CloseEnhMetaFile, CreateEnhMetaFileW, DeleteEnhMetaFile, Rectangle,
    };
    use windows_sys::Win32::System::DataExchange::SetClipboardData;

    let _alone = alone();
    let Some(raw) = clip() else { return };
    raw.put(&[(CF_UNICODETEXT, utf16_bytes("cell A1"))]);
    let bounds = RECT {
        left: 0,
        top: 0,
        right: 2540,
        bottom: 1270,
    };
    // SAFETY: a metafile in memory (no file name), drawn into once and closed; the clipboard
    // is open on this thread and takes the metafile over when `SetClipboardData` succeeds.
    unsafe {
        let dc = CreateEnhMetaFileW(
            std::ptr::null_mut(),
            std::ptr::null(),
            &raw const bounds,
            std::ptr::null(),
        );
        assert!(!dc.is_null(), "CreateEnhMetaFileW");
        Rectangle(dc, 10, 10, 90, 40);
        let metafile = CloseEnhMetaFile(dc);
        assert!(!metafile.is_null(), "CloseEnhMetaFile");
        if SetClipboardData(CF_ENHMETAFILE, metafile).is_null() {
            DeleteEnhMetaFile(metafile);
            panic!("SetClipboardData");
        }
    }
    let records = metafile_records(&raw).expect("the metafile is on the clipboard");
    let text = raw.get(CF_UNICODETEXT).expect("just put");
    assert!(
        raw.formats().contains(&CF_METAFILEPICT),
        "{:?}",
        raw.formats()
    );
    drop(raw);

    let platform = Platform::new();
    let snapshot = platform.snapshot().expect("snapshot");
    assert!(snapshot.is_complete(), "{snapshot:?}");
    assert!(
        snapshot
            .flavours()
            .iter()
            .any(|f| f.format == ClipboardFormat::Id(CF_ENHMETAFILE))
    );
    platform
        .write(&ClipboardContent::text("expansion"), true)
        .expect("write");
    platform.restore(snapshot).expect("restore");

    let raw = Clip::open().expect("clipboard");
    assert_eq!(metafile_records(&raw), Some(records), "the same picture");
    assert_eq!(raw.get(CF_UNICODETEXT), Some(text));
    assert!(
        raw.formats().contains(&CF_METAFILEPICT),
        "{:?}",
        raw.formats()
    );
    assert_eq!(markers_present(&raw), [true, true, true]);
}
