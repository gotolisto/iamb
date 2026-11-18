//! The wall clock, read only where the service needs it: timestamps for the store and the log,
//! and the time templates see.
//!
//! The engine's clock is UTC for now: the platform layer has no time-zone query yet, and the
//! service takes no time-zone dependency (the command-line tool has the same limit).

use std::time::{SystemTime, UNIX_EPOCH};

use iamb_core::datetime::{LocalDateTime, civil_from_days};
use iamb_core::model::Timestamp;

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Now, for the store.
#[must_use]
pub fn timestamp() -> Timestamp {
    Timestamp(now_ms())
}

/// The UTC date and time of `ms` since the epoch, split into fields.
fn civil(ms: i64) -> (i64, u8, u8, u8, u8, u8, u16) {
    let secs = ms.div_euclid(1000);
    let millis = u16::try_from(ms.rem_euclid(1000)).unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    let part = |v: i64| u8::try_from(v).unwrap_or(0);
    (
        i64::from(y),
        m,
        d,
        part(rem / 3600),
        part(rem % 3600 / 60),
        part(rem % 60),
        millis,
    )
}

/// Now as the engine's date and time (UTC; see the module docs).
///
/// # Panics
///
/// Never: the fallback date 2000-01-01 is valid.
#[must_use]
pub fn local_now() -> LocalDateTime {
    let (year, month, day, hour, minute, second, _) = civil(now_ms());
    let year = i32::try_from(year).unwrap_or(2000);
    LocalDateTime::new(year, month, day, hour, minute, second)
        .unwrap_or_else(|_| LocalDateTime::new(2000, 1, 1, 0, 0, 0).expect("valid date"))
}

/// `ms` as `YYYY-MM-DDTHH:MM:SS.mmmZ`, for log lines.
#[must_use]
pub fn iso_utc(ms: i64) -> String {
    let (year, month, day, hour, minute, second, millis) = civil(ms);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc(1_763_400_000_123), "2025-11-17T17:20:00.123Z");
        let _ = local_now();
        assert!(timestamp().0 > 0);
    }
}
