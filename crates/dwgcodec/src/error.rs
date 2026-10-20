//! The codec's errors.

/// Why a DWG file could not be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not a DWG file.
    NotDwg,
    /// A DWG of a version or form this codec does not handle yet.
    Unsupported(String),
    /// Data ran out before a value was complete.
    Truncated(&'static str),
    /// A value is out of range or contradicts the rest of the file.
    Corrupt(String),
    /// A stored check value does not match the data.
    BadCrc { what: &'static str, stored: u32, computed: u32 },
    /// The file asks for more than [`crate::Limits`] allows.
    TooLarge(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotDwg => write!(f, "not a DWG file"),
            Error::Unsupported(what) => write!(f, "{what}"),
            Error::Truncated(what) => write!(f, "the file ends inside {what}"),
            Error::Corrupt(what) => write!(f, "damaged file: {what}"),
            Error::BadCrc { what, stored, computed } => write!(f, "damaged file: {what} check value {stored:#x}, the data gives {computed:#x}"),
            Error::TooLarge(what) => write!(f, "{what} is larger than CADaddy reads"),
        }
    }
}

impl std::error::Error for Error {}

/// A [`Error::Corrupt`] with a message.
pub(crate) fn corrupt(what: impl Into<String>) -> Error {
    Error::Corrupt(what.into())
}
