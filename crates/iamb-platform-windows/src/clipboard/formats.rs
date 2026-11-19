//! Clipboard formats: which ones a snapshot copies, how text and file lists are encoded, and
//! the bounded retry around the clipboard's lock.
//!
//! # What a snapshot copies
//!
//! Data held in global memory is copied byte for byte and put back. Of the formats whose data
//! is a handle to a system object, the enhanced metafile is copied too, as its records
//! (`GetEnhMetaFileBits`), and made again from them on restore (`SetEnhMetaFileBits`): office
//! programs put a picture of the selection next to its text in an ordinary copy, and leaving
//! it out made every such snapshot incomplete, so no paste could be made while it was there
//! (the 2.8 follow-up review's N4). The other handle formats (the owner-display formats, the
//! private and GDI-object ranges, a bitmap or a palette without a device-independent bitmap,
//! a metafile picture without an enhanced metafile) cannot be copied and are left out; the
//! snapshot is then incomplete. Some formats are not left out but ignored, without making the
//! snapshot incomplete, because the system makes them again from another format when the
//! snapshot is restored: ANSI and OEM text when Unicode text is there, the bitmap and the
//! palette when a device-independent bitmap is there, the metafile picture when an enhanced
//! metafile is there. The system offers the device-independent bitmap whenever a bitmap is on
//! the clipboard, and the enhanced metafile whenever a metafile picture is (it makes each from
//! the other on request), so a copied picture is saved whole. Iamb's own history markers are
//! ignored too: a restore always sets them again (the trait's contract: a restore makes no
//! history entry).
//!
//! Registered formats (numbers from `0xC000`) hold global memory by the clipboard's rules (the
//! system frees them with `GlobalFree`), so they are copied: that is how the application
//! formats of office programs, browsers and editors survive a paste.
//!
//! The system has no call that says whether a format is rendered on demand by its owner:
//! asking for the data renders it. So a snapshot does render such formats, within limits: a
//! format larger than [`MAX_FLAVOUR_BYTES`], everything past [`MAX_SNAPSHOT_BYTES`] in total,
//! and every format still to be copied once [`SNAPSHOT_TIME_LIMIT`] has passed, is left out
//! and the snapshot marked incomplete.

use std::time::Duration;

pub(crate) const CF_TEXT: u32 = 1;
pub(crate) const CF_BITMAP: u32 = 2;
pub(crate) const CF_METAFILEPICT: u32 = 3;
pub(crate) const CF_SYLK: u32 = 4;
pub(crate) const CF_DIF: u32 = 5;
pub(crate) const CF_TIFF: u32 = 6;
pub(crate) const CF_OEMTEXT: u32 = 7;
pub(crate) const CF_DIB: u32 = 8;
pub(crate) const CF_PALETTE: u32 = 9;
pub(crate) const CF_PENDATA: u32 = 10;
pub(crate) const CF_RIFF: u32 = 11;
pub(crate) const CF_WAVE: u32 = 12;
pub(crate) const CF_UNICODETEXT: u32 = 13;
pub(crate) const CF_ENHMETAFILE: u32 = 14;
pub(crate) const CF_HDROP: u32 = 15;
pub(crate) const CF_LOCALE: u32 = 16;
pub(crate) const CF_DIBV5: u32 = 17;
pub(crate) const CF_OWNERDISPLAY: u32 = 0x0080;
pub(crate) const CF_DSPTEXT: u32 = 0x0081;
pub(crate) const CF_DSPBITMAP: u32 = 0x0082;
pub(crate) const CF_DSPMETAFILEPICT: u32 = 0x0083;
pub(crate) const CF_DSPENHMETAFILE: u32 = 0x008E;
pub(crate) const CF_PRIVATEFIRST: u32 = 0x0200;
pub(crate) const CF_PRIVATELAST: u32 = 0x02FF;
pub(crate) const CF_GDIOBJFIRST: u32 = 0x0300;
pub(crate) const CF_GDIOBJLAST: u32 = 0x03FF;
/// Registered formats start here.
pub(crate) const FIRST_REGISTERED: u32 = 0xC000;

/// Set (any data) on content that clipboard monitors, history included, must not process.
pub(crate) const EXCLUDE_FROM_MONITORS: &str = "ExcludeClipboardContentFromMonitorProcessing";
/// A `DWORD` 0 keeps the content out of the clipboard history.
pub(crate) const CAN_INCLUDE_IN_HISTORY: &str = "CanIncludeInClipboardHistory";
/// A `DWORD` 0 keeps the content out of the cloud clipboard.
pub(crate) const CAN_UPLOAD_TO_CLOUD: &str = "CanUploadToCloudClipboard";
/// The three history markers, each with the data Iamb writes.
pub(crate) const HISTORY_MARKERS: [(&str, [u8; 4]); 3] = [
    (EXCLUDE_FROM_MONITORS, [0; 4]),
    (CAN_INCLUDE_IN_HISTORY, [0; 4]),
    (CAN_UPLOAD_TO_CLOUD, [0; 4]),
];

/// The largest single format a snapshot copies.
pub(crate) const MAX_FLAVOUR_BYTES: usize = 64 << 20;
/// The most a snapshot copies in total.
pub(crate) const MAX_SNAPSHOT_BYTES: usize = 128 << 20;
/// How long a snapshot may spend copying (the owner of a format rendered on demand renders it
/// while the snapshot waits).
pub(crate) const SNAPSHOT_TIME_LIMIT: Duration = Duration::from_millis(500);

/// What a snapshot does with one format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Treatment {
    /// Copy its bytes.
    Copy,
    /// The enhanced metafile: copy its records, and make it again from them on restore.
    Metafile,
    /// Not needed: the system makes it again, or a restore sets it anyway.
    Ignore,
    /// Cannot be copied: the snapshot is incomplete.
    Skip,
}

/// What a snapshot does with `format`, given every format on the clipboard (`present`) and,
/// for a registered format, its name.
// The arms that skip are listed one by one: the list says which formats are handles.
#[allow(clippy::match_same_arms)]
pub(crate) fn treatment(format: u32, present: &[u32], name: Option<&str>) -> Treatment {
    let has = |f: u32| present.contains(&f);
    match format {
        CF_TEXT | CF_OEMTEXT if has(CF_UNICODETEXT) => Treatment::Ignore,
        CF_BITMAP | CF_PALETTE if has(CF_DIB) || has(CF_DIBV5) => Treatment::Ignore,
        CF_METAFILEPICT if has(CF_ENHMETAFILE) => Treatment::Ignore,
        CF_ENHMETAFILE => Treatment::Metafile,
        CF_TEXT | CF_SYLK | CF_DIF | CF_TIFF | CF_OEMTEXT | CF_DIB | CF_PENDATA | CF_RIFF
        | CF_WAVE | CF_UNICODETEXT | CF_HDROP | CF_LOCALE | CF_DIBV5 | CF_DSPTEXT => {
            Treatment::Copy
        }
        f if f >= FIRST_REGISTERED => match name {
            Some(n) if HISTORY_MARKERS.iter().any(|(m, _)| *m == n) => Treatment::Ignore,
            Some(_) => Treatment::Copy,
            // A registered format whose name cannot be read cannot be registered again.
            None => Treatment::Skip,
        },
        // Handles to system objects, and the private and GDI-object ranges, whose data the
        // owner frees itself.
        CF_BITMAP | CF_METAFILEPICT | CF_PALETTE | CF_OWNERDISPLAY | CF_DSPBITMAP
        | CF_DSPMETAFILEPICT | CF_DSPENHMETAFILE => Treatment::Skip,
        CF_PRIVATEFIRST..=CF_PRIVATELAST | CF_GDIOBJFIRST..=CF_GDIOBJLAST => Treatment::Skip,
        // Standard numbers this code does not know: their data could be anything.
        _ => Treatment::Skip,
    }
}

/// Unicode text for `CF_UNICODETEXT`: LF line endings become CR LF (a CR LF stays as it is),
/// then a terminating NUL, as UTF-16 little-endian bytes.
pub(crate) fn unicode_text(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2 + 4);
    let mut previous = '\0';
    for ch in text.chars() {
        if ch == '\n' && previous != '\r' {
            out.extend_from_slice(&u16::from(b'\r').to_le_bytes());
        }
        let mut units = [0; 2];
        for unit in ch.encode_utf16(&mut units) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        previous = ch;
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// The text of `CF_UNICODETEXT` data: up to the first NUL (or the end), CR LF as LF, unpaired
/// surrogates replaced.
pub(crate) fn text_from_unicode(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units).replace("\r\n", "\n")
}

/// `CF_HDROP` data for a list of paths, each given as UTF-16 code units: a `DROPFILES`
/// header (offset of the list, drop point, non-client flag, wide flag) followed by each path
/// with a NUL and a final NUL.
pub(crate) fn file_list(paths: &[Vec<u16>]) -> Vec<u8> {
    const HEADER: u32 = 20;
    let mut out = Vec::new();
    out.extend_from_slice(&HEADER.to_le_bytes()); // pFiles
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0i32.to_le_bytes()); // fNC
    out.extend_from_slice(&1i32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.iter().take_while(|u| **u != 0) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// The delays between attempts to open the clipboard for `read_text` (the `{clipboard}`
/// token, on the engine thread, which must not wait long): about a tenth of a second in all.
pub(crate) const OPEN_RETRY_DELAYS: [Duration; 9] = [
    Duration::from_millis(1),
    Duration::from_millis(2),
    Duration::from_millis(4),
    Duration::from_millis(8),
    Duration::from_millis(12),
    Duration::from_millis(16),
    Duration::from_millis(16),
    Duration::from_millis(16),
    Duration::from_millis(20),
];

/// The delays between attempts to open the clipboard for the steps of a paste (snapshot,
/// transient write, restore), on the injector's thread: about half a second in all.
///
/// A paste opens the clipboard at the moments other programs do too. Right after anything
/// is copied, every clipboard monitor (the history, the cloud clipboard, a remote-desktop
/// client's clipboard channel) opens it to read the new content or to see the markers that
/// tell it not to; after the paste shortcut, the target application opens it to read the
/// paste. On a busy machine one of them can hold it for longer than a tenth of a second
/// (seen on a CI runner: the snapshot right after the test's own copy failed with access
/// denied). Giving up then would fail the paste, or leave the transient content on the
/// clipboard when it is the restore that gives up; waiting half a second is the smaller harm.
/// With [`paste::RESTORE_ATTEMPTS`](super::paste::RESTORE_ATTEMPTS) a restore waits at most
/// about 1.6 s before it reports that the clipboard was not put back.
pub(crate) const PASTE_OPEN_RETRY_DELAYS: [Duration; 15] = [
    Duration::from_millis(1),
    Duration::from_millis(2),
    Duration::from_millis(4),
    Duration::from_millis(8),
    Duration::from_millis(16),
    Duration::from_millis(32),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
    Duration::from_millis(48),
];

/// Calls `attempt` until it succeeds, sleeping the given delays in between: one attempt more
/// than there are delays. Returns the last error.
pub(crate) fn retry<T, E>(
    delays: &[Duration],
    mut attempt: impl FnMut() -> Result<T, E>,
    mut sleep: impl FnMut(Duration),
) -> Result<T, E> {
    let mut result = attempt();
    for delay in delays {
        if result.is_ok() {
            break;
        }
        sleep(*delay);
        result = attempt();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_formats_are_copied() {
        for f in [
            CF_UNICODETEXT,
            CF_DIB,
            CF_DIBV5,
            CF_HDROP,
            CF_LOCALE,
            CF_RIFF,
            CF_WAVE,
            CF_TIFF,
            CF_SYLK,
            CF_DIF,
            CF_PENDATA,
            CF_DSPTEXT,
        ] {
            assert_eq!(treatment(f, &[f], None), Treatment::Copy, "{f}");
        }
        assert_eq!(
            treatment(0xC0F0, &[0xC0F0], Some("HTML Format")),
            Treatment::Copy
        );
        assert_eq!(
            treatment(0xC0F1, &[0xC0F1], Some("Rich Text Format")),
            Treatment::Copy
        );
        assert_eq!(
            treatment(0xFFFF, &[], Some("Private App Data")),
            Treatment::Copy
        );
    }

    #[test]
    fn handle_formats_are_skipped() {
        for f in [
            CF_BITMAP,
            CF_METAFILEPICT,
            CF_PALETTE,
            CF_OWNERDISPLAY,
            CF_DSPBITMAP,
            CF_DSPMETAFILEPICT,
            CF_DSPENHMETAFILE,
            CF_PRIVATEFIRST,
            0x0250,
            CF_PRIVATELAST,
            CF_GDIOBJFIRST,
            CF_GDIOBJLAST,
            18,
            0x7FFF,
        ] {
            assert_eq!(treatment(f, &[f], None), Treatment::Skip, "{f:#x}");
        }
        // A registered format without a readable name cannot be put back.
        assert_eq!(treatment(0xC123, &[0xC123], None), Treatment::Skip);
    }

    #[test]
    fn synthesised_formats_and_history_markers_are_ignored() {
        let text = [CF_UNICODETEXT, CF_TEXT, CF_OEMTEXT, CF_LOCALE];
        assert_eq!(treatment(CF_TEXT, &text, None), Treatment::Ignore);
        assert_eq!(treatment(CF_OEMTEXT, &text, None), Treatment::Ignore);
        assert_eq!(treatment(CF_LOCALE, &text, None), Treatment::Copy);
        // Without Unicode text, the ANSI text is the original.
        assert_eq!(treatment(CF_TEXT, &[CF_TEXT], None), Treatment::Copy);

        let image = [CF_DIB, CF_BITMAP, CF_PALETTE, CF_DIBV5];
        assert_eq!(treatment(CF_BITMAP, &image, None), Treatment::Ignore);
        assert_eq!(treatment(CF_PALETTE, &image, None), Treatment::Ignore);
        assert_eq!(
            treatment(CF_BITMAP, &[CF_DIBV5, CF_BITMAP], None),
            Treatment::Ignore
        );
        assert_eq!(treatment(CF_DIB, &image, None), Treatment::Copy);

        for (name, _) in HISTORY_MARKERS {
            assert_eq!(treatment(0xC200, &[0xC200], Some(name)), Treatment::Ignore);
        }
    }

    /// The follow-up review's N4: what an office program's copy of a cell or a paragraph
    /// leaves on the clipboard, a picture of it next to its text, is saved whole: the
    /// enhanced metafile as its records, the metafile picture and the bitmap made again by
    /// the system from the enhanced metafile and the device-independent bitmap.
    #[test]
    fn a_copy_with_a_picture_of_the_selection_is_saved_whole() {
        let office = [
            0xC0F0,
            CF_ENHMETAFILE,
            CF_METAFILEPICT,
            CF_BITMAP,
            CF_UNICODETEXT,
            CF_TEXT,
            CF_OEMTEXT,
            CF_LOCALE,
            CF_DIB,
            CF_DIBV5,
        ];
        let treatments: Vec<Treatment> = office
            .iter()
            .map(|&f| treatment(f, &office, (f == 0xC0F0).then_some("Rich Text Format")))
            .collect();
        assert!(
            !treatments.contains(&Treatment::Skip),
            "nothing is left out: {treatments:?}"
        );
        assert_eq!(
            treatment(CF_ENHMETAFILE, &office, None),
            Treatment::Metafile
        );
        assert_eq!(treatment(CF_METAFILEPICT, &office, None), Treatment::Ignore);
        // A metafile picture the system offers no enhanced metafile for stays a handle.
        assert_eq!(
            treatment(CF_METAFILEPICT, &[CF_METAFILEPICT], None),
            Treatment::Skip
        );
        // The enhanced metafile alone is copied as well.
        assert_eq!(
            treatment(CF_ENHMETAFILE, &[CF_ENHMETAFILE], None),
            Treatment::Metafile
        );
    }

    #[test]
    fn unicode_text_has_crlf_and_a_nul() {
        let units = |bytes: Vec<u8>| -> Vec<u16> {
            bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect()
        };
        assert_eq!(units(unicode_text("")), [0]);
        assert_eq!(units(unicode_text("a\nb")), [0x61, 0x0D, 0x0A, 0x62, 0]);
        assert_eq!(
            units(unicode_text("a\r\nb\n")),
            [0x61, 0x0D, 0x0A, 0x62, 0x0D, 0x0A, 0]
        );
        assert_eq!(units(unicode_text("👍é")), [0xD83D, 0xDC4D, 0xE9, 0]);
        for text in ["", "plain", "two\nlines\n", "café 👨‍👩‍👧 🇫🇷 漢字", "\n\n"]
        {
            assert_eq!(text_from_unicode(&unicode_text(text)), text);
        }
    }

    #[test]
    fn text_is_read_up_to_the_nul() {
        let mut data = unicode_text("visible");
        data.extend_from_slice(&unicode_text("hidden"));
        assert_eq!(text_from_unicode(&data), "visible");
        // No NUL, and an odd trailing byte.
        let mut data: Vec<u8> = "ab".encode_utf16().flat_map(u16::to_le_bytes).collect();
        data.push(0x41);
        assert_eq!(text_from_unicode(&data), "ab");
        // A lone surrogate is replaced, not an error.
        assert_eq!(text_from_unicode(&[0x3D, 0xD8, 0, 0]), "\u{FFFD}");
    }

    #[test]
    fn file_lists_have_the_dropfiles_layout() {
        let paths: Vec<Vec<u16>> = [r"C:\a.png", r"D:\dir\é.txt"]
            .iter()
            .map(|p| p.encode_utf16().collect())
            .collect();
        let data = file_list(&paths);
        assert_eq!(&data[0..4], &20u32.to_le_bytes());
        assert_eq!(&data[4..16], &[0; 12]);
        assert_eq!(&data[16..20], &1u32.to_le_bytes());
        let units: Vec<u16> = data[20..]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        let mut expected: Vec<u16> = r"C:\a.png".encode_utf16().collect();
        expected.push(0);
        expected.extend(r"D:\dir\é.txt".encode_utf16());
        expected.extend([0, 0]);
        assert_eq!(units, expected);
        assert_eq!(file_list(&[]).len(), 22);
    }

    #[test]
    fn retry_is_bounded() {
        let mut calls = 0;
        let mut slept = Duration::ZERO;
        let result: Result<(), &str> = retry(
            &OPEN_RETRY_DELAYS,
            || {
                calls += 1;
                Err("busy")
            },
            |d| slept += d,
        );
        assert_eq!(result, Err("busy"));
        assert_eq!(calls, OPEN_RETRY_DELAYS.len() + 1);
        assert!(slept <= Duration::from_millis(120), "{slept:?}");
        assert!(slept >= Duration::from_millis(80), "{slept:?}");

        let mut calls = 0;
        let mut sleeps = 0;
        let result: Result<u32, &str> = retry(
            &OPEN_RETRY_DELAYS,
            || {
                calls += 1;
                if calls < 3 { Err("busy") } else { Ok(calls) }
            },
            |_| sleeps += 1,
        );
        assert_eq!(result, Ok(3));
        assert_eq!(sleeps, 2);

        let result: Result<u8, &str> = retry(&[], || Ok(1), |_| panic!("no sleep"));
        assert_eq!(result, Ok(1));
    }

    /// CI failure on PR #9: a paste's snapshot found the clipboard held for longer than the
    /// tenth of a second `read_text` waits. A paste waits about half a second, starting with
    /// short delays so that the usual brief hold costs a millisecond or two.
    #[test]
    fn a_paste_waits_longer_for_the_clipboard_than_read_text() {
        let total = |delays: &[Duration]| delays.iter().sum::<Duration>();
        let paste = total(&PASTE_OPEN_RETRY_DELAYS);
        assert!(paste >= Duration::from_millis(450), "{paste:?}");
        assert!(paste <= Duration::from_millis(550), "{paste:?}");
        assert!(paste >= 4 * total(&OPEN_RETRY_DELAYS));
        assert_eq!(PASTE_OPEN_RETRY_DELAYS[0], Duration::from_millis(1));
        assert!(
            PASTE_OPEN_RETRY_DELAYS
                .windows(2)
                .all(|pair| pair[0] <= pair[1]),
            "the delays grow"
        );
        // A holder that lets go after 300 ms: the paste gets the clipboard.
        let waited = std::cell::Cell::new(Duration::ZERO);
        let result: Result<(), &str> = retry(
            &PASTE_OPEN_RETRY_DELAYS,
            || {
                if waited.get() >= Duration::from_millis(300) {
                    Ok(())
                } else {
                    Err("held")
                }
            },
            |d| waited.set(waited.get() + d),
        );
        assert_eq!(result, Ok(()));
    }
}
