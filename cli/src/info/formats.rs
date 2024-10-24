use std::{
    convert::Infallible,
    fmt,
    io::{self, Write},
    path,
};

use zip::{CompressionMethod, DateTime};

use super::directives::Writeable;
use crate::{args::info::*, extract::receiver::EntryKind};

pub trait FormatValue {
    type Input<'a>;
    type Output<'a>;
    type E;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E>;
}

#[derive(Copy, Clone)]
pub struct NameString;

impl FormatValue for NameString {
    type Input<'a> = &'a str;
    type Output<'a> = &'a str;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(input)
    }
}

#[derive(Copy, Clone)]
pub struct PathString;

#[derive(Debug)]
pub enum PathWriter<'a> {
    Path(path::Display<'a>),
    None,
}

impl<'a> fmt::Display for PathWriter<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Path(p) => path::Display::fmt(p, f),
            Self::None => write!(f, "<no file path>"),
        }
    }
}

impl FormatValue for PathString {
    type Input<'a> = Option<&'a path::Path>;
    type Output<'a> = PathWriter<'a>;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match input {
            Some(p) => PathWriter::Path(p.display()),
            None => PathWriter::None,
        })
    }
}

#[derive(Copy, Clone)]
pub struct FileTypeValue(pub FileTypeFormat);

impl FormatValue for FileTypeValue {
    type Input<'a> = EntryKind;
    type Output<'a> = &'static str;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            FileTypeFormat::Full => match input {
                EntryKind::File => "file",
                EntryKind::Dir => "directory",
                EntryKind::Symlink => "symlink",
            },
            FileTypeFormat::Abbreviated => match input {
                EntryKind::File => "-",
                EntryKind::Dir => "d",
                EntryKind::Symlink => "l",
            },
        })
    }
}

#[derive(Copy, Clone)]
pub struct CompressionMethodValue(pub CompressionMethodFormat);

impl FormatValue for CompressionMethodValue {
    type Input<'a> = CompressionMethod;
    type Output<'a> = &'static str;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            CompressionMethodFormat::Full => match input {
                CompressionMethod::Stored => "stored",
                CompressionMethod::Deflated => "deflate",
                #[cfg(feature = "deflate64")]
                CompressionMethod::Deflate64 => "deflate64",
                #[cfg(feature = "bzip2")]
                CompressionMethod::Bzip2 => "bzip2",
                #[cfg(feature = "zstd")]
                CompressionMethod::Zstd => "zstd",
                #[cfg(feature = "lzma")]
                CompressionMethod::Lzma => "lzma",
                #[cfg(feature = "xz")]
                CompressionMethod::Xz => "xz",
                _ => "unknown",
            },
            CompressionMethodFormat::Abbreviated => match input {
                CompressionMethod::Stored => "stor",
                CompressionMethod::Deflated => "defl",
                #[cfg(feature = "deflate64")]
                CompressionMethod::Deflate64 => "df64",
                #[cfg(feature = "bzip2")]
                CompressionMethod::Bzip2 => "bz2",
                #[cfg(feature = "zstd")]
                CompressionMethod::Zstd => "zst",
                #[cfg(feature = "lzma")]
                CompressionMethod::Lzma => "lz",
                #[cfg(feature = "xz")]
                CompressionMethod::Xz => "xz",
                _ => "?",
            },
        })
    }
}

#[derive(Copy, Clone)]
pub struct UnixModeValue(pub UnixModeFormat);

impl UnixModeValue {
    const S_IRUSR: u32 = 256;
    const S_IWUSR: u32 = 128;
    const S_IXUSR: u32 = 64;

    const S_IRGRP: u32 = 32;
    const S_IWGRP: u32 = 16;
    const S_IXGRP: u32 = 8;

    const S_IROTH: u32 = 4;
    const S_IWOTH: u32 = 2;
    const S_IXOTH: u32 = 1;

    const UNKNOWN_MODE_BITS: [u8; 9] = [b'?'; 9];

    fn pretty_format_mode_bits(mode: u32) -> [u8; 9] {
        let mut ret = [b'-'; 9];

        if mode & Self::S_IRUSR == Self::S_IRUSR {
            ret[0] = b'r';
        }
        if mode & Self::S_IWUSR == Self::S_IWUSR {
            ret[1] = b'w';
        }
        if mode & Self::S_IXUSR == Self::S_IXUSR {
            ret[2] = b'x';
        }

        if mode & Self::S_IRGRP == Self::S_IRGRP {
            ret[3] = b'r';
        }
        if mode & Self::S_IWGRP == Self::S_IWGRP {
            ret[4] = b'w';
        }
        if mode & Self::S_IXGRP == Self::S_IXGRP {
            ret[5] = b'x';
        }

        if mode & Self::S_IROTH == Self::S_IROTH {
            ret[6] = b'r';
        }
        if mode & Self::S_IWOTH == Self::S_IWOTH {
            ret[7] = b'w';
        }
        if mode & Self::S_IXOTH == Self::S_IXOTH {
            ret[8] = b'x';
        }

        ret
    }
}

#[derive(Debug)]
pub enum ModeValueWriter {
    Octal(Option<u32>),
    Pretty([u8; 9]),
}

impl Writeable for ModeValueWriter {
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error> {
        match self {
            Self::Octal(mode) => match mode {
                Some(bits) => write!(out, "{:o}", bits),
                None => write!(out, "?"),
            },
            Self::Pretty(bits) => out.write_all(bits.as_ref()),
        }
    }
}

impl FormatValue for UnixModeValue {
    type Input<'a> = Option<u32>;
    type Output<'a> = ModeValueWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            UnixModeFormat::Octal => ModeValueWriter::Octal(input),
            UnixModeFormat::Pretty => ModeValueWriter::Pretty(match input {
                Some(bits) => Self::pretty_format_mode_bits(bits),
                None => Self::UNKNOWN_MODE_BITS,
            }),
        })
    }
}

#[derive(Copy, Clone)]
pub struct ByteSizeValue(pub ByteSizeFormat);

#[derive(Debug)]
pub enum ByteSizeWriter {
    FullDecimal(u64),
}

impl fmt::Display for ByteSizeWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::FullDecimal(n) => write!(f, "{}", n),
        }
    }
}

impl FormatValue for ByteSizeValue {
    type Input<'a> = u64;
    type Output<'a> = ByteSizeWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            ByteSizeFormat::FullDecimal => ByteSizeWriter::FullDecimal(input),
            ByteSizeFormat::HumanAbbreviated => todo!("human abbreviated byte sizes"),
        })
    }
}

#[derive(Copy, Clone)]
pub struct DecimalNumberValue;

impl FormatValue for DecimalNumberValue {
    type Input<'a> = u64;
    type Output<'a> = u64;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(input)
    }
}

#[derive(Copy, Clone)]
pub struct OffsetValue(pub OffsetFormat);

#[derive(Debug)]
pub enum OffsetWriter {
    Unknown,
    Decimal(u64),
    Hexadecimal(u64),
}

impl fmt::Display for OffsetWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "?"),
            Self::Decimal(x) => write!(f, "{}", x),
            Self::Hexadecimal(x) => write!(f, "{:x}", x),
        }
    }
}

impl FormatValue for OffsetValue {
    type Input<'a> = Option<u64>;
    type Output<'a> = OffsetWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        let input = match input {
            None => return Ok(OffsetWriter::Unknown),
            Some(input) => input,
        };
        Ok(match self.0 {
            OffsetFormat::Decimal => OffsetWriter::Decimal(input),
            OffsetFormat::Hexadecimal => OffsetWriter::Hexadecimal(input),
        })
    }
}

#[derive(Copy, Clone)]
pub struct BinaryNumericValue(pub BinaryNumericValueFormat);

#[derive(Debug)]
pub enum BinaryNumericValueWriter {
    Decimal(u32),
    Hexadecimal(u32),
}

impl fmt::Display for BinaryNumericValueWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Decimal(x) => write!(f, "{}", x),
            Self::Hexadecimal(x) => write!(f, "{:x}", x),
        }
    }
}

impl FormatValue for BinaryNumericValue {
    type Input<'a> = u32;
    type Output<'a> = BinaryNumericValueWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            BinaryNumericValueFormat::Decimal => BinaryNumericValueWriter::Decimal(input),
            BinaryNumericValueFormat::Hexadecimal => BinaryNumericValueWriter::Hexadecimal(input),
        })
    }
}

#[derive(Copy, Clone)]
pub struct BinaryStringValue(pub BinaryStringFormat);

#[derive(Debug)]
pub enum BinaryStringWriter<'a> {
    ReplaceNonUnicode(&'a [u8]),
    EscapeAscii(&'a [u8]),
    WriteExactly(&'a [u8]),
}

impl<'a> BinaryStringWriter<'a> {
    const INVALID_CHUNK_BUFS: [&'static str; 4] = ["", "�", "��", "���"];
}

impl<'a> Writeable for BinaryStringWriter<'a> {
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error> {
        match self {
            Self::ReplaceNonUnicode(s) => {
                for chunk in s.utf8_chunks() {
                    write!(out, "{}", chunk.valid())?;
                    /* The length of invalid bytes is never longer than 3. */
                    write!(out, "{}", Self::INVALID_CHUNK_BUFS[chunk.invalid().len()])?;
                }
                Ok(())
            }
            Self::EscapeAscii(s) => {
                if s.is_empty() {
                    return write!(out, "\"\"");
                }
                write!(out, "\" ")?;
                for b in s.iter().copied() {
                    write!(out, "{} ", b.escape_ascii())?;
                }
                write!(out, "\"")?;
                Ok(())
            }
            Self::WriteExactly(s) => out.write_all(s),
        }
    }
}

impl FormatValue for BinaryStringValue {
    type Input<'a> = Option<&'a [u8]>;
    type Output<'a> = BinaryStringWriter<'a>;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        let input = input.unwrap_or(&[]);
        Ok(match self.0 {
            BinaryStringFormat::PrintAsString => BinaryStringWriter::ReplaceNonUnicode(input),
            BinaryStringFormat::EscapeAscii => BinaryStringWriter::EscapeAscii(input),
            BinaryStringFormat::WriteBinaryContents => BinaryStringWriter::WriteExactly(input),
        })
    }
}

#[derive(Copy, Clone)]
pub struct TimestampValue(pub TimestampFormat);

#[derive(Debug)]
pub enum TimestampValueWriter {
    None,
    DateOnly(DateTime),
    TimeOnly(DateTime),
    DateAndTime(DateTime),
}

impl fmt::Display for TimestampValueWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::None => write!(f, "?"),
            Self::DateOnly(d) => write!(f, "{}-{}-{}", d.year(), d.month(), d.day()),
            Self::TimeOnly(t) => write!(f, "{}:{}:{}", t.hour(), t.minute(), t.second()),
            Self::DateAndTime(dt) => {
                write!(
                    f,
                    "{}-{}-{} {}:{}:{}",
                    dt.year(),
                    dt.month(),
                    dt.day(),
                    dt.hour(),
                    dt.minute(),
                    dt.second()
                )
            }
        }
    }
}

impl FormatValue for TimestampValue {
    type Input<'a> = Option<DateTime>;
    type Output<'a> = TimestampValueWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        let input = match input {
            None => return Ok(TimestampValueWriter::None),
            Some(input) => input,
        };
        Ok(match self.0 {
            TimestampFormat::DateOnly => TimestampValueWriter::DateOnly(input),
            TimestampFormat::TimeOnly => TimestampValueWriter::TimeOnly(input),
            TimestampFormat::DateAndTime => TimestampValueWriter::DateAndTime(input),
        })
    }
}
