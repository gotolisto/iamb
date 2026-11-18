//! Locking memory with `mlock` (architecture §4.1, §5, §8 rule 1).
//!
//! `lock` keeps the pages of a region out of swap: it locks the page-aligned range that
//! contains the region (the kernel locks whole pages, so neighbouring data on those pages is
//! locked too). `unlock` releases them. The service locks the engine's keystroke buffer and the
//! storage of the typed-characters hand-off at start.
//!
//! Locks do not nest: unlocking a region unlocks its pages for every region that shares them.
//! The service therefore never unlocks while it runs; the pages are released when it exits.

use iamb_platform::{MemoryLock, MemoryRegion, PlatformError, Result};

use crate::Platform;

/// The memory lock's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

/// The page-aligned `(start, length)` that covers `region` for pages of `page` bytes.
pub(crate) fn page_span(region: MemoryRegion, page: usize) -> Option<(usize, usize)> {
    if region.is_empty() || page == 0 || !page.is_power_of_two() {
        return None;
    }
    let start = region.addr() & !(page - 1);
    let end = region.addr().checked_add(region.len())?;
    let end = end.checked_add(page - 1)? & !(page - 1);
    Some((start, end - start))
}

fn page_size() -> usize {
    // SAFETY: `sysconf` only reads a configuration value; it has no memory-safety
    // preconditions.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size).unwrap_or(16 * 1024)
}

fn errno() -> i64 {
    i64::from(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1))
}

impl MemoryLock for Platform {
    fn lock(&self, region: MemoryRegion) -> Result<()> {
        let State {} = &self.memory;
        let (start, len) = page_span(region, page_size()).ok_or(PlatformError::Invalid {
            context: "an empty memory region cannot be locked",
        })?;
        // SAFETY: `mlock` only changes the paging attributes of the range; it neither reads
        // nor writes it. The range covers memory of this process (the caller's region rounded
        // to whole pages); a range that is not mapped makes it fail with ENOMEM.
        let rc = unsafe { libc::mlock(std::ptr::without_provenance(start), len) };
        if rc == 0 {
            Ok(())
        } else {
            Err(PlatformError::os(errno(), "mlock"))
        }
    }

    fn unlock(&self, region: MemoryRegion) -> Result<()> {
        let (start, len) = page_span(region, page_size()).ok_or(PlatformError::Invalid {
            context: "an empty memory region cannot be unlocked",
        })?;
        // SAFETY: as for `mlock`: only the paging attributes of the range change.
        let rc = unsafe { libc::munlock(std::ptr::without_provenance(start), len) };
        if rc == 0 {
            Ok(())
        } else {
            Err(PlatformError::os(errno(), "munlock"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_cover_whole_pages() {
        let r = MemoryRegion::from_raw(0x1_0010, 0x20);
        assert_eq!(page_span(r, 0x1000), Some((0x1_0000, 0x1000)));
        let r = MemoryRegion::from_raw(0x1_0ff0, 0x20);
        assert_eq!(page_span(r, 0x1000), Some((0x1_0000, 0x2000)));
        assert_eq!(page_span(MemoryRegion::from_raw(0x1000, 0), 0x1000), None);
        assert_eq!(page_span(r, 3000), None);
    }

    #[test]
    fn a_heap_buffer_locks_and_unlocks() {
        let platform = Platform::new();
        let buffer = vec![0_u8; 256];
        let region = MemoryRegion::of_slice(&buffer);
        platform.lock(region).expect("mlock of a small buffer");
        platform
            .lock(region)
            .expect("locking again is not an error");
        platform.unlock(region).expect("munlock");
        assert!(matches!(
            platform.lock(MemoryRegion::from_raw(region.addr(), 0)),
            Err(PlatformError::Invalid { .. })
        ));
    }
}
