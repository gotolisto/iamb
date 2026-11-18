//! The error type shown to the user: one plain line, never a `Debug` dump.

use std::fmt;

use iamb_core::model::ModelError;
use iamb_store::StoreError;
use iamb_store::transfer::TransferError;

/// A message for the user.
#[derive(Debug)]
pub struct CliError(String);

impl CliError {
    /// Wraps a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<StoreError> for CliError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Locked { .. } => Self::new("the service is starting, try again"),
            other => Self(other.to_string()),
        }
    }
}

impl From<TransferError> for CliError {
    fn from(e: TransferError) -> Self {
        Self(e.to_string())
    }
}

impl From<ModelError> for CliError {
    fn from(e: ModelError) -> Self {
        Self(e.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}
