//! Locking memory. **Task 2.6** fills this file.
//!
//! What it must do (architecture §4.1, §5, §8 rule 1; the trait docs of [`MemoryLock`]):
//!
//! - `lock` keeps the pages of a region out of the page file; `unlock` releases them. The
//!   service locks the engine's keystroke buffer and the storage of the typed-characters
//!   hand-off at start.
//! - An empty region is [`Invalid`](iamb_platform::PlatformError::Invalid). If the OS refuses
//!   because the working-set quota is too small, raise the minimum working set once and try
//!   again before reporting [`Os`](iamb_platform::PlatformError::Os).

use iamb_platform::{MemoryLock, MemoryRegion, PlatformError, Result};

use crate::Platform;

/// The memory lock's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl MemoryLock for Platform {
    fn lock(&self, _region: MemoryRegion) -> Result<()> {
        let State {} = &self.memory;
        Err(PlatformError::Unsupported)
    }

    fn unlock(&self, _region: MemoryRegion) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        let region = MemoryRegion::of_slice(&[0_u8; 16]);
        assert_eq!(platform.lock(region), Err(PlatformError::Unsupported));
        assert_eq!(platform.unlock(region), Err(PlatformError::Unsupported));
    }
}
