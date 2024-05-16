#![allow(unknown_lints)] // non_local_definitions isn't in Rust 1.70
#![allow(non_local_definitions)]
//! Error types that can be emitted from this library

use std::borrow::Cow;
use displaydoc::Display;
use thiserror::Error;

use std::error::Error;
use std::fmt;
use std::io;
use std::num::TryFromIntError;

/// Generic result type with ZipError as its error variant
pub type ZipResult<T> = Result<T, ZipError>;

/// Error type for Zip
#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum ZipError {
    /// i/o error: {0}
    Io(#[from] io::Error),

    /// invalid Zip archive: {0}
    InvalidArchive(Cow<'static, str>),

    /// unsupported Zip archive: {0}
    UnsupportedArchive(Cow<'static, str>),

    /// specified file not found in archive
    FileNotFound(Box<str>),

    /// The password provided is incorrect
    InvalidPassword {
        filename: Box<str>,
        password: Option<Box<str>>
    },
}

pub(crate) fn invalid_archive<T, M: Into<Cow<'static, str>>>(message: M) -> ZipResult<T> {
    Err(ZipError::InvalidArchive(message.into()))
}

macro_rules! invalid {
    ($fmt_string:literal) => {
        {
            return crate::result::invalid_archive($fmt_string).into();
        }
    };
    ($fmt_string:literal, $($param:expr),+) => {
        {
            return crate::result::invalid_archive(alloc::format!($fmt_string, ($($param),+))).into();
        }
    };
}
pub(crate) use invalid;

impl From<ZipError> for io::Error {
    fn from(err: ZipError) -> io::Error {
        let kind = match &err {
            ZipError::Io(err) => err.kind(),
            ZipError::InvalidArchive(_) => io::ErrorKind::InvalidData,
            ZipError::UnsupportedArchive(_) => io::ErrorKind::Unsupported,
            ZipError::FileNotFound(_) => io::ErrorKind::NotFound,
            ZipError::InvalidPassword {..} => io::ErrorKind::InvalidInput,
        };

        io::Error::new(kind, err)
    }
}

/// Error type for time parsing
#[derive(Debug)]
pub struct DateTimeRangeError;

// TryFromIntError is also an out-of-range error.
impl From<TryFromIntError> for DateTimeRangeError {
    fn from(_value: TryFromIntError) -> Self {
        DateTimeRangeError
    }
}

impl fmt::Display for DateTimeRangeError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(
            fmt,
            "a date could not be represented within the bounds the MS-DOS date range (1980-2107)"
        )
    }
}

impl Error for DateTimeRangeError {}
