//! The error type of the client and server helpers.

use std::fmt;

use iamb_platform::PlatformError;

use crate::frame::FrameError;
use crate::protocol::{DecodeError, ErrorBody, ErrorCode};

/// Why an IPC operation failed. Never carries message content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcError {
    /// Connecting or binding failed in the platform layer (`EndpointNotFound`, `EndpointInUse`,
    /// `PeerRejected`, …).
    Platform(PlatformError),
    /// A frame could not be read or written.
    Frame(FrameError),
    /// A frame did not decode.
    Decode(DecodeError),
    /// The peer broke the protocol (an unexpected kind of frame, a response to another id).
    Protocol(&'static str),
    /// The server answered with an error (or refused the hello).
    Remote(ErrorBody),
}

impl IpcError {
    /// The server's error code, when the server answered with one.
    #[must_use]
    pub const fn remote_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Remote(body) => Some(body.code),
            _ => None,
        }
    }

    /// True when the connection is unusable afterwards.
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        !matches!(self, Self::Remote(_) | Self::Frame(FrameError::Timeout))
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform(e) => write!(f, "{e}"),
            Self::Frame(e) => write!(f, "{e}"),
            Self::Decode(e) => write!(f, "{e}"),
            Self::Protocol(what) => write!(f, "protocol error: {what}"),
            Self::Remote(body) => write!(f, "{} ({})", body.message, body.code.as_str()),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<PlatformError> for IpcError {
    fn from(e: PlatformError) -> Self {
        Self::Platform(e)
    }
}

impl From<FrameError> for IpcError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

impl From<DecodeError> for IpcError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}
