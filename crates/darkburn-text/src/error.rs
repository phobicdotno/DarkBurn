//! Error type shared by the crate.

use std::fmt;

/// Everything that can go wrong in this crate.
#[derive(Debug, Clone, PartialEq)]
pub enum TextError {
    /// A font file or font data could not be read or parsed.
    Font(String),
    /// A SHX file is malformed.
    Shx(String),
    /// QR content too long or otherwise not encodable.
    Qr(String),
    /// Barcode data not valid for the chosen symbology.
    Barcode(String),
    /// Variable-text problem (bad format string, CSV parse error...).
    Variable(String),
    /// File access failed.
    Io(String),
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (k, m) = match self {
            TextError::Font(m) => ("font", m),
            TextError::Shx(m) => ("shx", m),
            TextError::Qr(m) => ("qr", m),
            TextError::Barcode(m) => ("barcode", m),
            TextError::Variable(m) => ("variable text", m),
            TextError::Io(m) => ("io", m),
        };
        write!(f, "{k}: {m}")
    }
}

impl std::error::Error for TextError {}

impl From<std::io::Error> for TextError {
    fn from(e: std::io::Error) -> Self {
        TextError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TextError>;
