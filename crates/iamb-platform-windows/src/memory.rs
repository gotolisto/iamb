//! Locking memory with `VirtualLock` (architecture §4.1, §5, §8 rule 1).
//!
//! `lock` keeps the pages of a region out of the page file; `unlock` releases them. The
//! service locks the engine's keystroke buffer and the storage of the typed-characters
//! hand-off at start. A process may lock only as many pages as its minimum working set allows;
//! when `VirtualLock` refuses for that reason (`ERROR_WORKING_SET_QUOTA`), the minimum and
//! maximum working set are raised once by the region's size plus a margin and the lock is
//! tried again. Locks do not nest: the service never unlocks while it runs.

use iamb_platform::{MemoryLock, MemoryRegion, PlatformError, Result};
use windows_sys::Win32::Foundation::{ERROR_WORKING_SET_QUOTA, GetLastError};
use windows_sys::Win32::System::Memory::{VirtualLock, VirtualUnlock};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessWorkingSetSize, SetProcessWorkingSetSize,
};

use crate::Platform;

/// Extra working set requested on top of the region, for the pages the region straddles.
const MARGIN: usize = 64 * 1024;

/// The memory lock's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

fn last_error() -> u32 {
    // SAFETY: `GetLastError` only reads the calling thread's last-error value.
    unsafe { GetLastError() }
}

fn virtual_lock(region: MemoryRegion) -> std::result::Result<(), u32> {
    // SAFETY: `VirtualLock` only changes how the pages of the range are paged; it neither
    // reads nor writes them. A range that is not committed memory of this process makes it
    // fail with an error code.
    let ok = unsafe { VirtualLock(std::ptr::without_provenance(region.addr()), region.len()) };
    if ok != 0 { Ok(()) } else { Err(last_error()) }
}

/// Raises the working set by `extra` bytes, once.
fn grow_working_set(extra: usize) -> bool {
    let (mut min, mut max) = (0_usize, 0_usize);
    // SAFETY: the pseudo handle of the current process is always valid; both out-pointers are
    // valid.
    let ok = unsafe { GetProcessWorkingSetSize(GetCurrentProcess(), &raw mut min, &raw mut max) };
    if ok == 0 {
        return false;
    }
    // SAFETY: as above; the new sizes are larger than the current ones.
    unsafe {
        SetProcessWorkingSetSize(
            GetCurrentProcess(),
            min.saturating_add(extra),
            max.saturating_add(extra),
        ) != 0
    }
}

impl MemoryLock for Platform {
    fn lock(&self, region: MemoryRegion) -> Result<()> {
        let State {} = &self.memory;
        if region.is_empty() {
            return Err(PlatformError::Invalid {
                context: "an empty memory region cannot be locked",
            });
        }
        match virtual_lock(region) {
            Ok(()) => Ok(()),
            Err(ERROR_WORKING_SET_QUOTA) if grow_working_set(region.len() + MARGIN) => {
                virtual_lock(region)
                    .map_err(|code| PlatformError::os(i64::from(code), "VirtualLock"))
            }
            Err(code) => Err(PlatformError::os(i64::from(code), "VirtualLock")),
        }
    }

    fn unlock(&self, region: MemoryRegion) -> Result<()> {
        if region.is_empty() {
            return Err(PlatformError::Invalid {
                context: "an empty memory region cannot be unlocked",
            });
        }
        // SAFETY: as for `VirtualLock`: only the paging attributes of the range change.
        let ok =
            unsafe { VirtualUnlock(std::ptr::without_provenance(region.addr()), region.len()) };
        if ok != 0 {
            Ok(())
        } else {
            Err(PlatformError::os(i64::from(last_error()), "VirtualUnlock"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heap_buffer_locks_and_unlocks() {
        let platform = Platform::new();
        let buffer = vec![0_u8; 256];
        let region = MemoryRegion::of_slice(&buffer);
        platform
            .lock(region)
            .expect("VirtualLock of a small buffer");
        platform
            .lock(region)
            .expect("locking again is not an error");
        platform.unlock(region).expect("VirtualUnlock");
        assert!(matches!(
            platform.lock(MemoryRegion::from_raw(region.addr(), 0)),
            Err(PlatformError::Invalid { .. })
        ));
    }

    #[test]
    fn many_regions_lock_by_growing_the_working_set() {
        // More than the default minimum working set allows locking (about 200 KB).
        let platform = Platform::new();
        let buffers: Vec<Vec<u8>> = (0..64).map(|_| vec![1_u8; 16 * 1024]).collect();
        for b in &buffers {
            platform.lock(MemoryRegion::of_slice(b)).expect("locked");
        }
        for b in &buffers {
            let _ = platform.unlock(MemoryRegion::of_slice(b));
        }
    }
}
