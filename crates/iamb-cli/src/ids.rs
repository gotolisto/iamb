//! Clock and id generation.
//!
//! Ids are UUID version 7: the current time from `std::time::SystemTime` plus 10 random bytes
//! from the operating system (`getrandom`). If the system source fails, the bytes are derived
//! from the time in nanoseconds, the process id and a counter, which still keeps ids unique.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use iamb_core::model::{Id, Timestamp};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The current time.
pub fn now() -> Timestamp {
    Timestamp(i64::try_from(now_ms()).unwrap_or(i64::MAX))
}

fn random_bytes() -> [u8; 10] {
    let mut buf = [0u8; 10];
    if getrandom::fill(&mut buf).is_ok() {
        return buf;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mix = u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or(0)
        ^ u64::from(std::process::id()).rotate_left(32)
        ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    buf[..8].copy_from_slice(&mix.to_le_bytes());
    buf[8..].copy_from_slice(&u16::try_from(n & 0xffff).unwrap_or(0).to_le_bytes());
    buf
}

/// A fresh id.
pub fn new_id() -> Id {
    Id::v7(now_ms(), random_bytes())
}
