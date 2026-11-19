//! The clipboard (architecture §5, §8 rules 2 and 8; the trait docs of
//! [`ClipboardAccess`](iamb_platform::ClipboardAccess)).
//!
//! # Parts
//!
//! | Module | Runs on | Does |
//! | --- | --- | --- |
//! | `formats` | every system | which formats a snapshot copies; text and file-list encodings; the retry around the clipboard's lock |
//! | `html` | every system | the HTML format with its header of byte offsets |
//! | `paste` | every system | snapshot, transient write, paste shortcut, wait, restore, and what each failure does |
//! | `os` | Windows | each system call behind a small safe function |
//!
//! # Behaviour
//!
//! - `read_text`: the Unicode text, CR LF turned into LF (the engine's line ending).
//! - `snapshot`: every format whose data is global memory (text, HTML, RTF, device-independent
//!   bitmaps, file lists, and the registered formats applications put there), byte for byte,
//!   and the enhanced metafile as its records; the bitmap, the palette and the metafile picture
//!   the system makes again from those; other formats that are handles to system objects are
//!   left out and the snapshot says it is incomplete (`formats` has the rules and the limits).
//! - `write`: plain text (CR LF line endings), HTML in the HTML format, files as a file list.
//!   With `transient`, also the three markers that keep content out of clipboard monitors,
//!   the clipboard history and the cloud clipboard.
//! - `restore`: the snapshot's formats, always with the three markers: putting the user's
//!   content back must not add it to the history a second time. An empty snapshot empties
//!   the clipboard.
//! - Another program can hold the clipboard: opening is retried, then
//!   [`Os`](iamb_platform::PlatformError::Os) names `OpenClipboard`. `read_text` (on the
//!   engine thread) retries for about a tenth of a second; `snapshot`, `write` and `restore`,
//!   the steps of a paste on the injector thread, for about half a second, because right after
//!   a copy and right after the paste shortcut other programs open the clipboard too
//!   (`formats::PASTE_OPEN_RETRY_DELAYS` says who, and the totals).
//!
//! # Privacy
//!
//! Nothing here logs. Errors carry the name of a system call and its code. Bytes read for
//! `read_text` are overwritten before they are freed; snapshot bytes live in
//! [`Sensitive`](iamb_platform::core_types::Sensitive).

// The parts that run on every system are only called from the Windows-only parts; on other
// systems they exist for their tests.
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) mod formats;
pub(crate) mod html;
pub(crate) mod paste;

#[cfg(windows)]
mod os;

#[cfg(windows)]
pub(crate) use windows::{State, restore_now, sequence, snapshot_now, write_now};

#[cfg(windows)]
mod windows {
    use std::time::Instant;

    use iamb_platform::core_types::Sensitive;
    use iamb_platform::{
        ClipboardAccess, ClipboardContent, ClipboardFlavour, ClipboardFormat, ClipboardSnapshot,
        PlatformError, Result,
    };

    use super::formats::{
        CF_ENHMETAFILE, CF_HDROP, CF_UNICODETEXT, FIRST_REGISTERED, HISTORY_MARKERS,
        MAX_FLAVOUR_BYTES, MAX_SNAPSHOT_BYTES, SNAPSHOT_TIME_LIMIT, Treatment, file_list,
        text_from_unicode, treatment, unicode_text,
    };
    use super::html;
    use super::os::{self, Data, MemoryFormat, Open};
    use crate::Platform;

    /// The clipboard keeps no state: format numbers are registered on use (the system
    /// answers from its atom table).
    #[derive(Debug, Default)]
    pub(crate) struct State {}

    /// Overwrites bytes that held clipboard content before they are freed.
    fn wipe(mut bytes: Vec<u8>) {
        bytes.fill(0);
        std::hint::black_box(&mut bytes);
    }

    fn put_markers(open: &Open) -> Result<()> {
        for (name, data) in HISTORY_MARKERS {
            open.put(os::register(name)?, &data)?;
        }
        Ok(())
    }

    /// Everything on the clipboard that can be copied.
    pub(crate) fn snapshot_now() -> Result<ClipboardSnapshot> {
        let open = os::open(os::Patience::Paste)?;
        let present = open.formats()?;
        let started = Instant::now();
        let mut total = 0usize;
        let mut complete = true;
        let mut flavours = Vec::new();
        for &format in &present {
            let name = if format >= FIRST_REGISTERED {
                os::format_name(format)
            } else {
                None
            };
            let treated = treatment(format, &present, name.as_deref());
            match treated {
                Treatment::Ignore => continue,
                Treatment::Skip => {
                    complete = false;
                    continue;
                }
                Treatment::Copy | Treatment::Metafile => {}
            }
            if started.elapsed() > SNAPSHOT_TIME_LIMIT {
                complete = false;
                continue;
            }
            let limit = MAX_FLAVOUR_BYTES.min(MAX_SNAPSHOT_BYTES.saturating_sub(total));
            let data = if treated == Treatment::Metafile {
                open.read_enhanced_metafile(limit)
            } else {
                let Some(memory) = MemoryFormat::new(format, &present, name.as_deref()) else {
                    complete = false;
                    continue;
                };
                open.read(memory, limit)
            };
            match data {
                Data::Bytes(bytes) => {
                    total += bytes.len();
                    flavours.push(ClipboardFlavour {
                        format: name.map_or(ClipboardFormat::Id(format), ClipboardFormat::Name),
                        data: Sensitive::new(bytes),
                    });
                }
                Data::Missing | Data::TooBig => complete = false,
            }
        }
        Ok(ClipboardSnapshot::new(flavours, complete))
    }

    /// Writes `content`; returns the clipboard's sequence number after the write.
    pub(crate) fn write_now(content: &ClipboardContent, transient: bool) -> Result<u32> {
        let html_format = match &content.html {
            Some(_) => Some(os::register(html::FORMAT_NAME)?),
            None => None,
        };
        let open = os::open(os::Patience::Paste)?;
        open.empty()?;
        if transient {
            put_markers(&open)?;
        }
        if let Some(text) = &content.text {
            let data = unicode_text(text);
            let put = open.put(CF_UNICODETEXT, &data);
            wipe(data);
            put?;
        }
        if let (Some(fragment), Some(format)) = (&content.html, html_format) {
            let data = html::encode(fragment);
            let put = open.put(format, &data);
            wipe(data);
            put?;
        }
        if !content.files.is_empty() {
            use std::os::windows::ffi::OsStrExt;
            let paths: Vec<Vec<u16>> = content
                .files
                .iter()
                .map(|p| p.as_os_str().encode_wide().collect())
                .collect();
            open.put(CF_HDROP, &file_list(&paths))?;
        }
        drop(open);
        Ok(sequence())
    }

    /// Puts a snapshot back, with the history markers.
    pub(crate) fn restore_now(snapshot: &ClipboardSnapshot) -> Result<()> {
        // Register first, so that a failure leaves the clipboard as it is.
        let mut formats = Vec::with_capacity(snapshot.flavours().len());
        for flavour in snapshot.flavours() {
            formats.push(match &flavour.format {
                ClipboardFormat::Id(id) => *id,
                ClipboardFormat::Name(name) => os::register(name)?,
            });
        }
        let open = os::open(os::Patience::Paste)?;
        open.empty()?;
        if snapshot.is_empty() {
            return Ok(());
        }
        put_markers(&open)?;
        // Put back as much as possible; report the first failure.
        let mut first_error: Option<PlatformError> = None;
        for (flavour, format) in snapshot.flavours().iter().zip(formats) {
            let put = if format == CF_ENHMETAFILE {
                // Saved as its records (`formats` module docs): made again here.
                open.put_enhanced_metafile(flavour.data.expose())
            } else {
                open.put(format, flavour.data.expose())
            };
            if let Err(error) = put {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// The clipboard's sequence number.
    pub(crate) fn sequence() -> u32 {
        os::sequence()
    }

    impl ClipboardAccess for Platform {
        fn read_text(&self) -> Result<Option<Sensitive<String>>> {
            let State {} = &self.clipboard;
            let open = os::open(os::Patience::Brief)?;
            match open.read(MemoryFormat::UNICODE_TEXT, MAX_FLAVOUR_BYTES) {
                Data::Bytes(bytes) => {
                    let text = text_from_unicode(&bytes);
                    wipe(bytes);
                    Ok(Some(Sensitive::new(text)))
                }
                Data::Missing | Data::TooBig => Ok(None),
            }
        }

        fn snapshot(&self) -> Result<ClipboardSnapshot> {
            snapshot_now()
        }

        fn write(&self, content: &ClipboardContent, transient: bool) -> Result<()> {
            write_now(content, transient).map(|_| ())
        }

        fn restore(&self, snapshot: ClipboardSnapshot) -> Result<()> {
            restore_now(&snapshot)
        }
    }
}
