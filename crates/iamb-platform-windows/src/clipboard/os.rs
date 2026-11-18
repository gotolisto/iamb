//! The clipboard's system calls, each behind a small safe function.
//!
//! The clipboard is a lock shared by every program: [`open`] retries briefly when another
//! program holds it, and [`Open`] closes it when dropped. Each opening uses a message-only
//! window of its own as the clipboard's owner (the system's documentation says that content
//! set while the clipboard was opened without an owner window may be refused), created on
//! the calling thread and destroyed with the opening.

use std::marker::PhantomData;
use std::ptr;

use iamb_platform::{PlatformError, Result};
use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HWND, SetLastError};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE};

use super::formats::{CF_UNICODETEXT, OPEN_RETRY_DELAYS, Treatment, retry, treatment};

/// The last error of the calling thread as a [`PlatformError::Os`].
fn last_error(context: &'static str) -> PlatformError {
    // SAFETY: the call takes no arguments and reads a value of the calling thread.
    let code = unsafe { GetLastError() };
    PlatformError::os(i64::from(code), context)
}

/// A NUL-terminated UTF-16 copy of `text`.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A message-only window that owns the clipboard while it is open.
struct OwnerWindow(HWND);

impl OwnerWindow {
    fn create() -> Result<Self> {
        let class = wide("STATIC");
        // SAFETY: `class` names a system window class and is NUL-terminated; the parent
        // `HWND_MESSAGE` makes a message-only window; the other arguments are plain values or
        // null, which the call accepts.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if window.is_null() {
            Err(last_error("CreateWindowExW"))
        } else {
            Ok(Self(window))
        }
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created by this thread (the type is not `Send`, see `Open`)
        // and is destroyed once.
        unsafe { DestroyWindow(self.0) };
    }
}

/// The clipboard, open on this thread. Closed when dropped.
pub(super) struct Open {
    /// Destroyed after the clipboard is closed (fields drop after `Drop::drop`).
    _owner: OwnerWindow,
    /// The clipboard is open for the thread that opened it.
    _thread_bound: PhantomData<*const ()>,
}

/// Opens the clipboard, retrying for about a tenth of a second while another program holds
/// it.
pub(super) fn open() -> Result<Open> {
    let owner = OwnerWindow::create()?;
    retry(
        &OPEN_RETRY_DELAYS,
        || {
            // SAFETY: `owner.0` is a live window of this thread.
            if unsafe { OpenClipboard(owner.0) } == 0 {
                Err(last_error("OpenClipboard"))
            } else {
                Ok(())
            }
        },
        std::thread::sleep,
    )?;
    Ok(Open {
        _owner: owner,
        _thread_bound: PhantomData,
    })
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by this thread in `open`, and is closed once,
        // before the owner window is destroyed (fields are dropped after this body).
        unsafe { CloseClipboard() };
    }
}

/// A format whose data is global memory, which may be read and copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MemoryFormat(u32);

impl MemoryFormat {
    /// Unicode text.
    pub(super) const UNICODE_TEXT: Self = Self(CF_UNICODETEXT);

    /// `format`, if a snapshot copies it (see `formats::treatment`).
    pub(super) fn new(format: u32, present: &[u32], name: Option<&str>) -> Option<Self> {
        (treatment(format, present, name) == Treatment::Copy).then_some(Self(format))
    }
}

/// The data of one format.
pub(super) enum Data {
    Bytes(Vec<u8>),
    /// Not on the clipboard, not renderable, or not readable as memory.
    Missing,
    /// Larger than the limit asked for.
    TooBig,
}

// The methods take `&self` as proof that the clipboard is open on this thread.
#[allow(clippy::unused_self)]
impl Open {
    /// The formats on the clipboard, in the order the system lists them.
    pub(super) fn formats(&self) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut format = 0;
        loop {
            // SAFETY: plain value; the call reads a value of the calling thread.
            unsafe { SetLastError(0) };
            // SAFETY: the clipboard is open on this thread; any format number is accepted.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                // SAFETY: as above.
                let code = unsafe { GetLastError() };
                return if code == 0 {
                    Ok(out)
                } else {
                    Err(PlatformError::os(i64::from(code), "EnumClipboardFormats"))
                };
            }
            if out.contains(&format) || out.len() >= 4096 {
                // A clipboard that lists a format twice, or thousands of them: stop.
                return Ok(out);
            }
            out.push(format);
        }
    }

    /// A copy of the data of `format`, if it is at most `limit` bytes. A format rendered on
    /// demand is rendered by its owner first.
    pub(super) fn read(&self, format: MemoryFormat, limit: usize) -> Data {
        // SAFETY: the clipboard is open on this thread; any format number is accepted.
        let handle = unsafe { GetClipboardData(format.0) };
        if handle.is_null() {
            return Data::Missing;
        }
        // SAFETY: the data of a `MemoryFormat` is a global memory handle by the clipboard's
        // rules, and stays valid while the clipboard is open; a stale handle is answered with 0.
        let size = unsafe { GlobalSize(handle) };
        if size == 0 {
            return Data::Missing;
        }
        if size > limit {
            return Data::TooBig;
        }
        // SAFETY: as above.
        let base = unsafe { GlobalLock(handle) };
        if base.is_null() {
            return Data::Missing;
        }
        // SAFETY: the block is locked, and `GlobalSize` said it holds `size` bytes, readable
        // until it is unlocked below.
        let bytes = unsafe { std::slice::from_raw_parts(base.cast::<u8>(), size) }.to_vec();
        // SAFETY: the block was locked once above.
        unsafe { GlobalUnlock(handle) };
        Data::Bytes(bytes)
    }

    /// Empties the clipboard and makes this opening's window its owner.
    pub(super) fn empty(&self) -> Result<()> {
        // SAFETY: the clipboard is open on this thread with an owner window.
        if unsafe { EmptyClipboard() } == 0 {
            Err(last_error("EmptyClipboard"))
        } else {
            Ok(())
        }
    }

    /// Puts `bytes` on the clipboard as `format` (after [`empty`](Self::empty)).
    pub(super) fn put(&self, format: u32, bytes: &[u8]) -> Result<()> {
        // A block of zero bytes cannot be allocated as data; one byte more is harmless.
        let size = bytes.len().max(1);
        // SAFETY: plain values.
        let block = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) };
        if block.is_null() {
            return Err(last_error("GlobalAlloc"));
        }
        // SAFETY: `block` was just allocated and is not locked.
        let base = unsafe { GlobalLock(block) };
        if base.is_null() {
            let error = last_error("GlobalLock");
            // SAFETY: `block` is ours and is freed once.
            unsafe { GlobalFree(block) };
            return Err(error);
        }
        // SAFETY: the locked block holds `size >= bytes.len()` bytes and does not overlap
        // `bytes`; the spare byte of an empty block is set to zero.
        unsafe {
            ptr::write_bytes(base.cast::<u8>(), 0, size);
            ptr::copy_nonoverlapping(bytes.as_ptr(), base.cast::<u8>(), bytes.len());
        }
        // SAFETY: the block was locked once above.
        unsafe { GlobalUnlock(block) };
        // SAFETY: the clipboard is open on this thread and owned by its window; `block` is
        // moveable global memory, which the system takes over when the call succeeds.
        let set = unsafe { SetClipboardData(format, block) };
        if set.is_null() {
            let error = last_error("SetClipboardData");
            // SAFETY: the system did not take the block, so it is still ours; freed once.
            unsafe { GlobalFree(block) };
            return Err(error);
        }
        Ok(())
    }
}

/// The number of a registered format.
pub(super) fn register(name: &str) -> Result<u32> {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated.
    let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    if format == 0 {
        Err(last_error("RegisterClipboardFormatW"))
    } else {
        Ok(format)
    }
}

/// The name of a registered format.
pub(super) fn format_name(format: u32) -> Option<String> {
    let mut buffer = [0u16; 256];
    let capacity = i32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` has room for `capacity` units, the terminating NUL included.
    let len = unsafe { GetClipboardFormatNameW(format, buffer.as_mut_ptr(), capacity) };
    let len = usize::try_from(len).ok().filter(|n| *n > 0)?;
    String::from_utf16(&buffer[..len.min(buffer.len())]).ok()
}

/// The clipboard's sequence number: it changes whenever the content does.
pub(super) fn sequence() -> u32 {
    // SAFETY: the call takes no arguments.
    unsafe { GetClipboardSequenceNumber() }
}
