//! What the helpers report for logging: message name, size and duration, never the body
//! (architecture §6.1, §8 rule 3).

use std::time::Duration;

/// Which way a frame went, seen from the side that logs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Received.
    In,
    /// Sent.
    Out,
}

/// One frame, as the log sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageRecord {
    /// Received or sent.
    pub direction: Direction,
    /// The message name (`list_snippets`, `library_changed`, `hello`, …).
    pub name: &'static str,
    /// The frame's payload size in bytes.
    pub size: usize,
    /// For a response: how long the request took, from its arrival to the answer.
    pub duration: Option<Duration>,
    /// For an error response: its code.
    pub error: Option<&'static str>,
}

/// Where the helpers send their [`MessageRecord`]s. The service writes them to its log file.
pub trait MessageLog: Send + Sync {
    /// Takes one record.
    fn record(&self, record: &MessageRecord);
}

/// A [`MessageLog`] that drops everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLog;

impl MessageLog for NoLog {
    fn record(&self, _record: &MessageRecord) {}
}
