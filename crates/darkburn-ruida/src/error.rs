//! Error type for the whole crate.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuidaError {
    /// A value cannot be encoded (out of range).
    Encode(String),
    /// Bytes could not be decoded into a command or file.
    Decode(String),
    /// The transport failed (socket or USB error).
    Transport(String),
    /// No reply arrived in time, after all retries.
    Timeout(String),
    /// The controller answered with ERR (logical 0xCD).
    ControllerError(String),
    /// Checksum rejected too many times (NAK, logical 0xCF).
    ChecksumRejected,
    /// Not connected.
    NotConnected,
    /// A file name or index that does not exist on the controller.
    NoSuchFile(String),
    /// The user cancelled (for example at the overwrite prompt).
    Cancelled,
    /// The operation is not available on this transport or controller.
    Unsupported(String),
    /// Worker thread is gone.
    WorkerGone,
}

impl fmt::Display for RuidaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuidaError::Encode(s) => write!(f, "cannot encode: {s}"),
            RuidaError::Decode(s) => write!(f, "cannot decode: {s}"),
            RuidaError::Transport(s) => write!(f, "transport error: {s}"),
            RuidaError::Timeout(s) => write!(f, "timed out: {s}"),
            RuidaError::ControllerError(s) => write!(f, "controller reported an error: {s}"),
            RuidaError::ChecksumRejected => write!(f, "controller rejected the packet checksum"),
            RuidaError::NotConnected => write!(f, "not connected"),
            RuidaError::NoSuchFile(s) => write!(f, "no such file on the controller: {s}"),
            RuidaError::Cancelled => write!(f, "cancelled"),
            RuidaError::Unsupported(s) => write!(f, "not supported: {s}"),
            RuidaError::WorkerGone => write!(f, "the controller worker thread has stopped"),
        }
    }
}

impl std::error::Error for RuidaError {}

impl From<std::io::Error> for RuidaError {
    fn from(e: std::io::Error) -> Self {
        RuidaError::Transport(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, RuidaError>;
