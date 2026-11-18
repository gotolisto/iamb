//! Framing (architecture §6.1): a 4-byte little-endian length, then one UTF-8 JSON document of
//! at most [`MAX_FRAME_LEN`] bytes.
//!
//! A frame is written with one `write_all` of header and payload together, so a reader that
//! has seen the first byte of a frame can expect the rest promptly. [`read_frame`] therefore
//! treats a read timeout before the first byte as "nothing yet" ([`FrameError::Timeout`]) and
//! keeps reading after it, up to [`FRAME_COMPLETION_LIMIT`], so a timeout never leaves the
//! stream between two frames' bytes.

use std::fmt;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// The largest payload a frame may carry: 8 MB (architecture §6.1). Bulk data travels as file
/// paths.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// How long a frame whose first byte has arrived may take to arrive completely.
pub const FRAME_COMPLETION_LIMIT: Duration = Duration::from_secs(10);

/// Why a frame could not be read or written. Carries no message content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The peer closed the stream cleanly between frames.
    Eof,
    /// The read timed out before the first byte of a frame.
    Timeout,
    /// The stream ended, or stalled past [`FRAME_COMPLETION_LIMIT`], in the middle of a frame.
    Truncated,
    /// The header announced more than [`MAX_FRAME_LEN`] bytes (or a payload to write is that
    /// large). The stream is out of step afterwards and must be closed.
    TooLarge(usize),
    /// Any other I/O failure.
    Io(io::ErrorKind),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof => f.write_str("the connection was closed"),
            Self::Timeout => f.write_str("timed out waiting for a message"),
            Self::Truncated => f.write_str("the connection ended in the middle of a message"),
            Self::TooLarge(n) => write!(
                f,
                "a message of {n} bytes is larger than the limit of {MAX_FRAME_LEN} bytes"
            ),
            Self::Io(kind) => write!(f, "i/o error: {kind}"),
        }
    }
}

impl std::error::Error for FrameError {}

fn is_timeout(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

/// Writes one frame.
///
/// # Errors
///
/// [`FrameError::TooLarge`] for a payload over [`MAX_FRAME_LEN`] (nothing is written);
/// [`FrameError::Io`] when the stream fails.
pub fn write_frame(w: &mut dyn Write, payload: &[u8]) -> Result<(), FrameError> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge(payload.len()))?;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf).map_err(|e| FrameError::Io(e.kind()))?;
    w.flush().map_err(|e| FrameError::Io(e.kind()))
}

/// Reads into `buf` completely, once at least one byte of the frame has been read.
fn fill(r: &mut dyn Read, buf: &mut [u8], started: Instant) -> Result<(), FrameError> {
    let mut done = 0;
    while done < buf.len() {
        match r.read(&mut buf[done..]) {
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if is_timeout(e.kind()) => {
                if started.elapsed() > FRAME_COMPLETION_LIMIT {
                    return Err(FrameError::Truncated);
                }
            }
            Err(e) => return Err(FrameError::Io(e.kind())),
        }
    }
    Ok(())
}

/// Reads one frame and returns its payload.
///
/// # Errors
///
/// [`FrameError::Eof`] when the stream ends before a frame starts; [`FrameError::Timeout`]
/// when the read timeout passes before a frame starts; [`FrameError::Truncated`];
/// [`FrameError::TooLarge`] (the payload is not read); [`FrameError::Io`].
pub fn read_frame(r: &mut dyn Read) -> Result<Vec<u8>, FrameError> {
    let mut header = [0_u8; 4];
    let first = loop {
        match r.read(&mut header) {
            Ok(0) => return Err(FrameError::Eof),
            Ok(n) => break n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if is_timeout(e.kind()) => return Err(FrameError::Timeout),
            Err(e) => return Err(FrameError::Io(e.kind())),
        }
    };
    let started = Instant::now();
    fill(r, &mut header[first..], started)?;
    let len = usize::try_from(u32::from_le_bytes(header)).unwrap_or(usize::MAX);
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0_u8; len];
    fill(r, &mut payload, started)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn a_frame_round_trips() {
        let mut out = Vec::new();
        write_frame(&mut out, b"{\"a\":1}").expect("written");
        assert_eq!(&out[..4], &7_u32.to_le_bytes());
        let mut input = Cursor::new(out);
        assert_eq!(read_frame(&mut input).expect("read"), b"{\"a\":1}");
        assert_eq!(read_frame(&mut input), Err(FrameError::Eof));
    }

    #[test]
    fn short_streams_are_truncated() {
        let mut input = Cursor::new(vec![5, 0]);
        assert_eq!(read_frame(&mut input), Err(FrameError::Truncated));
        let mut input = Cursor::new(vec![5, 0, 0, 0, b'{']);
        assert_eq!(read_frame(&mut input), Err(FrameError::Truncated));
    }

    #[test]
    fn oversized_frames_are_refused_both_ways() {
        let header = u32::try_from(MAX_FRAME_LEN + 1)
            .expect("fits")
            .to_le_bytes();
        let mut input = Cursor::new(header.to_vec());
        assert_eq!(
            read_frame(&mut input),
            Err(FrameError::TooLarge(MAX_FRAME_LEN + 1))
        );
        let big = vec![b' '; MAX_FRAME_LEN + 1];
        let mut out = Vec::new();
        assert_eq!(
            write_frame(&mut out, &big),
            Err(FrameError::TooLarge(MAX_FRAME_LEN + 1))
        );
        assert!(out.is_empty());
        // Exactly the limit is fine.
        let max = vec![b' '; MAX_FRAME_LEN];
        write_frame(&mut out, &max).expect("at the limit");
    }

    #[test]
    fn errors_describe_without_content() {
        for e in [
            FrameError::Eof,
            FrameError::Timeout,
            FrameError::Truncated,
            FrameError::TooLarge(9),
            FrameError::Io(io::ErrorKind::BrokenPipe),
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
