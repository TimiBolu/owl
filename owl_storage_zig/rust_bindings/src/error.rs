//! Error types for Owl Storage Engine

use std::fmt;

/// Result type for Owl Storage operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error types that can occur during storage operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Out of memory in the arena
    OutOfMemory,
    /// Invalid arena handle
    InvalidArena,
    /// Invalid object reference
    InvalidReference,
    /// Data buffer too small
    DataTooSmall,
    /// Access out of bounds
    OutOfBounds,
    /// Object has been deleted
    ObjectDeleted,
    /// Unknown error code
    Unknown(i32),
}

impl Error {
    /// Convert from C error code to Rust Error
    pub(crate) fn from_code(code: i32) -> Result<()> {
        match code {
            0 => Ok(()),
            -1 => Err(Error::OutOfMemory),
            -2 => Err(Error::InvalidArena),
            -3 => Err(Error::InvalidReference),
            -4 => Err(Error::DataTooSmall),
            -5 => Err(Error::OutOfBounds),
            -6 => Err(Error::ObjectDeleted),
            code => Err(Error::Unknown(code)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OutOfMemory => write!(f, "Out of memory in arena"),
            Error::InvalidArena => write!(f, "Invalid arena handle"),
            Error::InvalidReference => write!(f, "Invalid object reference"),
            Error::DataTooSmall => write!(f, "Data buffer too small"),
            Error::OutOfBounds => write!(f, "Access out of bounds"),
            Error::ObjectDeleted => write!(f, "Object has been deleted"),
            Error::Unknown(code) => write!(f, "Unknown error code: {}", code),
        }
    }
}

impl std::error::Error for Error {}
