//! Keeping typed characters out of the page file and swap (architecture §4.1, §5, §8 rule 1).

use crate::error::Result;

/// An address range in this process: the storage of the engine's keystroke buffer or of the
/// [`TypedHandoff`](crate::TypedHandoff).
///
/// Only an address and a length; building one reads nothing and needs no `unsafe`. The OS
/// locks whole pages, so neighbouring data on the same pages is locked too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryRegion {
    addr: usize,
    len: usize,
}

impl MemoryRegion {
    /// The range a slice occupies.
    #[must_use]
    pub fn of_slice<T>(slice: &[T]) -> Self {
        Self {
            addr: slice.as_ptr().addr(),
            len: size_of_val(slice),
        }
    }

    /// The range from an address and a length in bytes (for the accessor the engine exposes for
    /// its buffer, §4.1).
    #[must_use]
    pub const fn from_raw(addr: usize, len: usize) -> Self {
        Self { addr, len }
    }

    /// The first address.
    #[must_use]
    pub const fn addr(self) -> usize {
        self.addr
    }

    /// The length in bytes.
    #[must_use]
    pub const fn len(self) -> usize {
        self.len
    }

    /// True for a range of no bytes, which cannot be locked.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// Locks memory against swapping (`VirtualLock` on Windows, `mlock` on macOS).
pub trait MemoryLock: Send + Sync {
    /// Locks the pages that hold `region`. Locking a locked region again is not an error.
    ///
    /// # Errors
    ///
    /// [`Invalid`](crate::PlatformError::Invalid) for an empty region;
    /// [`Os`](crate::PlatformError::Os) when the OS refuses (the service then keeps running and
    /// records that the buffer is not locked).
    fn lock(&self, region: MemoryRegion) -> Result<()>;

    /// Unlocks the pages that hold `region`, before its storage is freed.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn unlock(&self, region: MemoryRegion) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_region_covers_its_bytes() {
        let chars = ['a'; 8];
        let region = MemoryRegion::of_slice(&chars);
        assert_eq!(region.addr(), chars.as_ptr().addr());
        assert_eq!(region.len(), 32);
        assert!(!region.is_empty());
        assert_eq!(region, MemoryRegion::from_raw(region.addr(), 32));
    }

    #[test]
    fn an_empty_slice_gives_an_empty_region() {
        let none: [u64; 0] = [];
        assert!(MemoryRegion::of_slice(&none).is_empty());
        assert!(MemoryRegion::from_raw(4096, 0).is_empty());
    }
}
