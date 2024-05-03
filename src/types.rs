#![allow(clippy::wrong_self_convention)]

//! Types that specify what is contained in a ZIP.
use crate::cp437::FromCp437;
use crate::write::{FileOptionExtension, FileOptions};
use num_enum::{FromPrimitive, IntoPrimitive};
use path::{Component, Path, PathBuf};
use std::path;
use std::sync::{Arc, OnceLock};

#[cfg(doc)]
use crate::read::ZipFile;
#[cfg(feature = "chrono")]
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::result::{ZipError, ZipResult};
use crate::spec::{self, Block};

pub(crate) mod ffi {
    pub const S_IFDIR: u32 = 0o0040000;
    pub const S_IFREG: u32 = 0o0100000;
}

#[cfg(any(
    all(target_arch = "arm", target_pointer_width = "32"),
    target_arch = "mips",
    target_arch = "powerpc"
))]
mod atomic {
    use crossbeam_utils::sync::ShardedLock;
    pub use std::sync::atomic::Ordering;

    #[derive(Debug, Default)]
    pub struct AtomicU64 {
        value: ShardedLock<u64>,
    }

    impl AtomicU64 {
        pub fn new(v: u64) -> Self {
            Self {
                value: ShardedLock::new(v),
            }
        }
        pub fn get_mut(&mut self) -> &mut u64 {
            self.value.get_mut().unwrap()
        }
        pub fn load(&self, _: Ordering) -> u64 {
            *self.value.read().unwrap()
        }
        pub fn store(&self, value: u64, _: Ordering) {
            *self.value.write().unwrap() = value;
        }
    }
}

use crate::extra_fields::ExtraField;
use crate::result::DateTimeRangeError;
#[cfg(feature = "time")]
use time::{error::ComponentRange, Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

pub(crate) struct ZipRawValues {
    pub(crate) crc32: u32,
    pub(crate) compressed_size: u64,
    pub(crate) uncompressed_size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum System {
    Dos = 0,
    Unix = 3,
    #[num_enum(default)]
    Unknown,
}

/// Representation of a moment in time.
///
/// Zip files use an old format from DOS to store timestamps,
/// with its own set of peculiarities.
/// For example, it has a resolution of 2 seconds!
///
/// A [`DateTime`] can be stored directly in a zipfile with [`FileOptions::last_modified_time`],
/// or read from one with [`ZipFile::last_modified`]
///
/// # Warning
///
/// Because there is no timezone associated with the [`DateTime`], they should ideally only
/// be used for user-facing descriptions. This also means [`DateTime::to_time`] returns an
/// [`OffsetDateTime`] (which is the equivalent of chrono's `NaiveDateTime`).
///
/// Modern zip files store more precise timestamps, which are ignored by [`crate::read::ZipArchive`],
/// so keep in mind that these timestamps are unreliable. [We're working on this](https://github.com/zip-rs/zip/issues/156#issuecomment-652981904).
#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

#[cfg(fuzzing)]
impl arbitrary::Arbitrary<'_> for DateTime {
    fn arbitrary(u: &mut arbitrary::Unstructured) -> arbitrary::Result<Self> {
        Ok(DateTime {
            year: u.int_in_range(1980..=2107)?,
            month: u.int_in_range(1..=12)?,
            day: u.int_in_range(1..=31)?,
            hour: u.int_in_range(0..=23)?,
            minute: u.int_in_range(0..=59)?,
            second: u.int_in_range(0..=60)?,
        })
    }
}

#[cfg(feature = "chrono")]
impl TryFrom<NaiveDateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_from(value: NaiveDateTime) -> Result<Self, Self::Error> {
        DateTime::from_date_and_time(
            value.year().try_into()?,
            value.month().try_into()?,
            value.day().try_into()?,
            value.hour().try_into()?,
            value.minute().try_into()?,
            value.second().try_into()?,
        )
    }
}

#[cfg(feature = "chrono")]
impl TryInto<NaiveDateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_into(self) -> Result<NaiveDateTime, Self::Error> {
        let date = NaiveDate::from_ymd_opt(self.year.into(), self.month.into(), self.day.into())
            .ok_or(DateTimeRangeError)?;
        let time =
            NaiveTime::from_hms_opt(self.hour.into(), self.minute.into(), self.second.into())
                .ok_or(DateTimeRangeError)?;
        Ok(NaiveDateTime::new(date, time))
    }
}

impl Default for DateTime {
    /// Constructs an 'default' datetime of 1980-01-01 00:00:00
    fn default() -> DateTime {
        DateTime {
            year: 1980,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }
}

impl DateTime {
    /// Converts an msdos (u16, u16) pair to a DateTime object
    pub const fn from_msdos(datepart: u16, timepart: u16) -> DateTime {
        let seconds = (timepart & 0b0000000000011111) << 1;
        let minutes = (timepart & 0b0000011111100000) >> 5;
        let hours = (timepart & 0b1111100000000000) >> 11;
        let days = datepart & 0b0000000000011111;
        let months = (datepart & 0b0000000111100000) >> 5;
        let years = (datepart & 0b1111111000000000) >> 9;

        DateTime {
            year: years + 1980,
            month: months as u8,
            day: days as u8,
            hour: hours as u8,
            minute: minutes as u8,
            second: seconds as u8,
        }
    }

    /// Constructs a DateTime from a specific date and time
    ///
    /// The bounds are:
    /// * year: [1980, 2107]
    /// * month: [1, 12]
    /// * day: [1, 31]
    /// * hour: [0, 23]
    /// * minute: [0, 59]
    /// * second: [0, 60]
    pub fn from_date_and_time(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<DateTime, DateTimeRangeError> {
        if (1980..=2107).contains(&year)
            && (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour <= 23
            && minute <= 59
            && second <= 60
        {
            Ok(DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
            })
        } else {
            Err(DateTimeRangeError)
        }
    }

    /// Indicates whether this date and time can be written to a zip archive.
    pub fn is_valid(&self) -> bool {
        DateTime::from_date_and_time(
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
        )
        .is_ok()
    }

    #[cfg(feature = "time")]
    /// Converts a OffsetDateTime object to a DateTime
    ///
    /// Returns `Err` when this object is out of bounds
    #[deprecated(note = "use `DateTime::try_from()`")]
    pub fn from_time(dt: OffsetDateTime) -> Result<DateTime, DateTimeRangeError> {
        dt.try_into().map_err(|_err| DateTimeRangeError)
    }

    /// Gets the time portion of this datetime in the msdos representation
    pub const fn timepart(&self) -> u16 {
        ((self.second as u16) >> 1) | ((self.minute as u16) << 5) | ((self.hour as u16) << 11)
    }

    /// Gets the date portion of this datetime in the msdos representation
    pub const fn datepart(&self) -> u16 {
        (self.day as u16) | ((self.month as u16) << 5) | ((self.year - 1980) << 9)
    }

    #[cfg(feature = "time")]
    /// Converts the DateTime to a OffsetDateTime structure
    pub fn to_time(&self) -> Result<OffsetDateTime, ComponentRange> {
        let date =
            Date::from_calendar_date(self.year as i32, Month::try_from(self.month)?, self.day)?;
        let time = Time::from_hms(self.hour, self.minute, self.second)?;
        Ok(PrimitiveDateTime::new(date, time).assume_utc())
    }

    /// Get the year. There is no epoch, i.e. 2018 will be returned as 2018.
    pub const fn year(&self) -> u16 {
        self.year
    }

    /// Get the month, where 1 = january and 12 = december
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    pub const fn month(&self) -> u8 {
        self.month
    }

    /// Get the day
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    pub const fn day(&self) -> u8 {
        self.day
    }

    /// Get the hour
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    pub const fn hour(&self) -> u8 {
        self.hour
    }

    /// Get the minute
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    pub const fn minute(&self) -> u8 {
        self.minute
    }

    /// Get the second
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    pub const fn second(&self) -> u8 {
        self.second
    }
}

#[cfg(feature = "time")]
impl TryFrom<OffsetDateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_from(dt: OffsetDateTime) -> Result<Self, Self::Error> {
        if dt.year() >= 1980 && dt.year() <= 2107 {
            Ok(DateTime {
                year: dt.year().try_into()?,
                month: dt.month().into(),
                day: dt.day(),
                hour: dt.hour(),
                minute: dt.minute(),
                second: dt.second(),
            })
        } else {
            Err(DateTimeRangeError)
        }
    }
}

pub const DEFAULT_VERSION: u8 = 46;

/// Structure representing a ZIP file.
#[derive(Debug, Clone)]
pub struct ZipFileData {
    /// Compatibility of the file attribute information
    pub system: System,
    /// Specification version
    pub version_made_by: u8,
    /// True if the file is encrypted.
    pub encrypted: bool,
    /// True if the file uses a data-descriptor section
    pub using_data_descriptor: bool,
    /// Compression method used to store the file
    pub compression_method: crate::compression::CompressionMethod,
    /// Compression level to store the file
    pub compression_level: Option<i64>,
    /// Last modified time. This will only have a 2 second precision.
    pub last_modified_time: DateTime,
    /// CRC32 checksum
    pub crc32: u32,
    /// Size of the file in the ZIP
    pub compressed_size: u64,
    /// Size of the file when extracted
    pub uncompressed_size: u64,
    /// Name of the file
    pub file_name: Box<str>,
    /// Raw file name. To be used when file_name was incorrectly decoded.
    pub file_name_raw: Box<[u8]>,
    /// Extra field usually used for storage expansion
    pub extra_field: Option<Arc<Vec<u8>>>,
    /// Extra field only written to central directory
    pub central_extra_field: Option<Arc<Vec<u8>>>,
    /// File comment
    pub file_comment: Box<str>,
    /// Specifies where the local header of the file starts
    pub header_start: u64,
    /// Specifies where the central header of the file starts
    ///
    /// Note that when this is not known, it is set to 0
    pub central_header_start: u64,
    /// Specifies where the compressed data of the file starts
    pub data_start: OnceLock<u64>,
    /// External file attributes
    pub external_attributes: u32,
    /// Reserve local ZIP64 extra field
    pub large_file: bool,
    /// AES mode if applicable
    pub aes_mode: Option<(AesMode, AesVendorVersion)>,

    /// extra fields, see <https://libzip.org/specifications/extrafld.txt>
    pub extra_fields: Vec<ExtraField>,
}

impl ZipFileData {
    pub fn file_name_sanitized(&self) -> PathBuf {
        let no_null_filename = match self.file_name.find('\0') {
            Some(index) => &self.file_name[0..index],
            None => &self.file_name,
        }
        .to_string();

        // zip files can contain both / and \ as separators regardless of the OS
        // and as we want to return a sanitized PathBuf that only supports the
        // OS separator let's convert incompatible separators to compatible ones
        let separator = path::MAIN_SEPARATOR;
        let opposite_separator = match separator {
            '/' => '\\',
            _ => '/',
        };
        let filename =
            no_null_filename.replace(&opposite_separator.to_string(), &separator.to_string());

        Path::new(&filename)
            .components()
            .filter(|component| matches!(*component, Component::Normal(..)))
            .fold(PathBuf::new(), |mut path, ref cur| {
                path.push(cur.as_os_str());
                path
            })
    }

    pub(crate) fn enclosed_name(&self) -> Option<PathBuf> {
        if self.file_name.contains('\0') {
            return None;
        }
        let path = PathBuf::from(self.file_name.to_string());
        let mut depth = 0usize;
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => return None,
                Component::ParentDir => depth = depth.checked_sub(1)?,
                Component::Normal(_) => depth += 1,
                Component::CurDir => (),
            }
        }
        Some(path)
    }

    /// Get unix mode for the file
    pub(crate) const fn unix_mode(&self) -> Option<u32> {
        if self.external_attributes == 0 {
            return None;
        }

        match self.system {
            System::Unix => Some(self.external_attributes >> 16),
            System::Dos => {
                // Interpret MS-DOS directory bit
                let mut mode = if 0x10 == (self.external_attributes & 0x10) {
                    ffi::S_IFDIR | 0o0775
                } else {
                    ffi::S_IFREG | 0o0664
                };
                if 0x01 == (self.external_attributes & 0x01) {
                    // Read-only bit; strip write permissions
                    mode &= 0o0555;
                }
                Some(mode)
            }
            _ => None,
        }
    }

    pub const fn zip64_extension(&self) -> bool {
        self.uncompressed_size > 0xFFFFFFFF
            || self.compressed_size > 0xFFFFFFFF
            || self.header_start > 0xFFFFFFFF
    }

    pub const fn version_needed(&self) -> u16 {
        // higher versions matched first
        match (self.zip64_extension(), self.compression_method) {
            #[cfg(feature = "bzip2")]
            (_, crate::compression::CompressionMethod::Bzip2) => 46,
            (true, _) => 45,
            _ => 20,
        }
    }
    #[inline(always)]
    pub(crate) fn extra_field_len(&self) -> usize {
        self.extra_field
            .as_ref()
            .map(|v| v.len())
            .unwrap_or_default()
    }
    #[inline(always)]
    pub(crate) fn central_extra_field_len(&self) -> usize {
        self.central_extra_field
            .as_ref()
            .map(|v| v.len())
            .unwrap_or_default()
    }

    pub(crate) fn initialize_local_block<S, T: FileOptionExtension>(
        name: S,
        options: &FileOptions<T>,
        raw_values: ZipRawValues,
        header_start: u64,
    ) -> Self
    where
        S: Into<Box<str>>,
    {
        let permissions = options.permissions.unwrap_or(0o100644);
        ZipFileData {
            system: System::Unix,
            version_made_by: DEFAULT_VERSION,
            encrypted: options.encrypt_with.is_some(),
            using_data_descriptor: false,
            compression_method: options.compression_method,
            compression_level: options.compression_level,
            last_modified_time: options.last_modified_time,
            crc32: raw_values.crc32,
            compressed_size: raw_values.compressed_size,
            uncompressed_size: raw_values.uncompressed_size,
            file_name: name.into(),
            file_name_raw: vec![].into_boxed_slice(), // Never used for saving
            extra_field: options.extended_options.extra_data().cloned(),
            central_extra_field: options.extended_options.central_extra_data().cloned(),
            file_comment: String::with_capacity(0).into_boxed_str(),
            header_start,
            data_start: OnceLock::new(),
            central_header_start: 0,
            external_attributes: permissions << 16,
            large_file: options.large_file,
            aes_mode: None,
            extra_fields: Vec::new(),
        }
    }

    pub(crate) fn from_local_block<R: std::io::Read>(
        block: ZipLocalEntryBlock,
        reader: &mut R,
    ) -> ZipResult<Self> {
        let ZipLocalEntryBlock {
            // magic,
            version_made_by,
            flags,
            compression_method,
            last_mod_time,
            last_mod_date,
            crc32,
            compressed_size,
            uncompressed_size,
            file_name_length,
            extra_field_length,
            ..
        } = block;

        let encrypted: bool = flags & 1 == 1;
        let is_utf8: bool = flags & (1 << 1) != 0;
        let using_data_descriptor: bool = flags & (1 << 3) != 0;
        #[allow(deprecated)]
        let compression_method = crate::CompressionMethod::from_u16(compression_method);
        let file_name_length: usize = file_name_length.into();
        let extra_field_length: usize = extra_field_length.into();

        if encrypted {
            return Err(ZipError::UnsupportedArchive(
                "Encrypted files are not supported",
            ));
        }
        if using_data_descriptor {
            return Err(ZipError::UnsupportedArchive(
                "The file length is not available in the local header",
            ));
        }

        let mut file_name_raw = vec![0u8; file_name_length];
        reader.read_exact(&mut file_name_raw)?;
        let mut extra_field = vec![0u8; extra_field_length];
        reader.read_exact(&mut extra_field)?;

        let file_name: Box<str> = match is_utf8 {
            true => String::from_utf8_lossy(&file_name_raw).into(),
            false => file_name_raw.clone().from_cp437().into(),
        };

        let system: u8 = (version_made_by >> 8).try_into().unwrap();
        Ok(ZipFileData {
            system: System::from(system),
            version_made_by: version_made_by.try_into().unwrap(),
            encrypted,
            using_data_descriptor,
            compression_method,
            compression_level: None,
            last_modified_time: DateTime::from_msdos(last_mod_date, last_mod_time),
            crc32,
            compressed_size: compressed_size.into(),
            uncompressed_size: uncompressed_size.into(),
            file_name,
            file_name_raw: file_name_raw.into(),
            extra_field: Some(Arc::new(extra_field)),
            central_extra_field: None,
            file_comment: String::with_capacity(0).into_boxed_str(), // file comment is only available in the central directory
            // header_start and data start are not available, but also don't matter, since seeking is
            // not available.
            header_start: 0,
            data_start: OnceLock::new(),
            central_header_start: 0,
            // The external_attributes field is only available in the central directory.
            // We set this to zero, which should be valid as the docs state 'If input came
            // from standard input, this field is set to zero.'
            external_attributes: 0,
            large_file: false,
            aes_mode: None,
            extra_fields: Vec::new(),
        })
    }

    pub(crate) fn local_block(&self) -> ZipResult<ZipLocalEntryBlock> {
        let (compressed_size, uncompressed_size) = if self.large_file {
            (spec::ZIP64_BYTES_THR as u32, spec::ZIP64_BYTES_THR as u32)
        } else {
            (
                self.compressed_size.try_into().unwrap(),
                self.uncompressed_size.try_into().unwrap(),
            )
        };
        let mut extra_field_length = self.extra_field_len();
        if self.large_file {
            extra_field_length += 20;
        }
        if extra_field_length + self.central_extra_field_len() > u16::MAX as usize {
            return Err(ZipError::InvalidArchive("Extra data field is too large"));
        }
        let extra_field_length: u16 = extra_field_length.try_into().unwrap();
        Ok(ZipLocalEntryBlock {
            magic: spec::LOCAL_FILE_HEADER_SIGNATURE,
            version_made_by: self.version_needed(),
            flags: if !self.file_name.is_ascii() {
                1u16 << 11
            } else {
                0
            } | if self.encrypted { 1u16 << 0 } else { 0 },
            #[allow(deprecated)]
            compression_method: self.compression_method.to_u16(),
            last_mod_time: self.last_modified_time.timepart(),
            last_mod_date: self.last_modified_time.datepart(),
            crc32: self.crc32,
            compressed_size,
            uncompressed_size,
            file_name_length: self.file_name.as_bytes().len().try_into().unwrap(),
            extra_field_length,
        })
    }

    pub(crate) fn block(&self, zip64_extra_field_length: u16) -> ZipEntryBlock {
        let extra_field_len: u16 = self.extra_field_len().try_into().unwrap();
        let central_extra_field_len: u16 = self.central_extra_field_len().try_into().unwrap();
        ZipEntryBlock {
            magic: spec::CENTRAL_DIRECTORY_HEADER_SIGNATURE,
            version_made_by: (self.system as u16) << 8 | (self.version_made_by as u16),
            version_to_extract: self.version_needed(),
            flags: if !self.file_name.is_ascii() {
                1u16 << 11
            } else {
                0
            } | if self.encrypted { 1u16 << 0 } else { 0 },
            #[allow(deprecated)]
            compression_method: self.compression_method.to_u16(),
            last_mod_time: self.last_modified_time.timepart(),
            last_mod_date: self.last_modified_time.datepart(),
            crc32: self.crc32,
            compressed_size: self
                .compressed_size
                .min(spec::ZIP64_BYTES_THR)
                .try_into()
                .unwrap(),
            uncompressed_size: self
                .uncompressed_size
                .min(spec::ZIP64_BYTES_THR)
                .try_into()
                .unwrap(),
            file_name_length: self.file_name.as_bytes().len().try_into().unwrap(),
            extra_field_length: zip64_extra_field_length
                + extra_field_len
                + central_extra_field_len,
            file_comment_length: self.file_comment.as_bytes().len().try_into().unwrap(),
            disk_number: 0,
            internal_file_attributes: 0,
            external_file_attributes: self.external_attributes,
            offset: self
                .header_start
                .min(spec::ZIP64_BYTES_THR)
                .try_into()
                .unwrap(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(packed)]
pub(crate) struct ZipEntryBlock {
    pub magic: spec::Magic,
    pub version_made_by: u16,
    pub version_to_extract: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub last_mod_time: u16,
    pub last_mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
    pub file_comment_length: u16,
    pub disk_number: u16,
    pub internal_file_attributes: u16,
    pub external_file_attributes: u32,
    pub offset: u32,
}

impl ZipEntryBlock {
    #[inline(always)]
    fn from_le(mut self) -> Self {
        from_le![
            self,
            [
                (magic, spec::Magic),
                (version_made_by, u16),
                (version_to_extract, u16),
                (flags, u16),
                (compression_method, u16),
                (last_mod_time, u16),
                (last_mod_date, u16),
                (crc32, u32),
                (compressed_size, u32),
                (uncompressed_size, u32),
                (file_name_length, u16),
                (extra_field_length, u16),
                (file_comment_length, u16),
                (disk_number, u16),
                (internal_file_attributes, u16),
                (external_file_attributes, u32),
                (offset, u32),
            ]
        ];
        self
    }

    #[inline(always)]
    fn to_le(mut self) -> Self {
        to_le![
            self,
            [
                (magic, spec::Magic),
                (version_made_by, u16),
                (version_to_extract, u16),
                (flags, u16),
                (compression_method, u16),
                (last_mod_time, u16),
                (last_mod_date, u16),
                (crc32, u32),
                (compressed_size, u32),
                (uncompressed_size, u32),
                (file_name_length, u16),
                (extra_field_length, u16),
                (file_comment_length, u16),
                (disk_number, u16),
                (internal_file_attributes, u16),
                (external_file_attributes, u32),
                (offset, u32),
            ]
        ];
        self
    }
}

impl Block for ZipEntryBlock {
    fn interpret(bytes: Box<[u8]>) -> ZipResult<Self> {
        let block = Self::deserialize(&bytes).from_le();

        if block.magic != spec::CENTRAL_DIRECTORY_HEADER_SIGNATURE {
            return Err(ZipError::InvalidArchive("Invalid Central Directory header"));
        }

        Ok(block)
    }

    fn encode(self) -> Box<[u8]> {
        self.to_le().serialize()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(packed)]
pub(crate) struct ZipLocalEntryBlock {
    pub magic: spec::Magic,
    pub version_made_by: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub last_mod_time: u16,
    pub last_mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
}

impl ZipLocalEntryBlock {
    #[inline(always)]
    fn from_le(mut self) -> Self {
        from_le![
            self,
            [
                (magic, spec::Magic),
                (version_made_by, u16),
                (flags, u16),
                (compression_method, u16),
                (last_mod_time, u16),
                (last_mod_date, u16),
                (crc32, u32),
                (compressed_size, u32),
                (uncompressed_size, u32),
                (file_name_length, u16),
                (extra_field_length, u16),
            ]
        ];
        self
    }

    #[inline(always)]
    fn to_le(mut self) -> Self {
        to_le![
            self,
            [
                (magic, spec::Magic),
                (version_made_by, u16),
                (flags, u16),
                (compression_method, u16),
                (last_mod_time, u16),
                (last_mod_date, u16),
                (crc32, u32),
                (compressed_size, u32),
                (uncompressed_size, u32),
                (file_name_length, u16),
                (extra_field_length, u16),
            ]
        ];
        self
    }
}

impl Block for ZipLocalEntryBlock {
    fn interpret(bytes: Box<[u8]>) -> ZipResult<Self> {
        let block = Self::deserialize(&bytes).from_le();

        if block.magic != spec::LOCAL_FILE_HEADER_SIGNATURE {
            return Err(ZipError::InvalidArchive("Invalid local file header"));
        }

        Ok(block)
    }

    fn encode(self) -> Box<[u8]> {
        self.to_le().serialize()
    }
}

/// The encryption specification used to encrypt a file with AES.
///
/// According to the [specification](https://www.winzip.com/win/en/aes_info.html#winzip11) AE-2
/// does not make use of the CRC check.
#[derive(Copy, Clone, Debug)]
pub enum AesVendorVersion {
    Ae1,
    Ae2,
}

/// AES variant used.
#[derive(Copy, Clone, Debug)]
pub enum AesMode {
    Aes128,
    Aes192,
    Aes256,
}

#[cfg(feature = "aes-crypto")]
impl AesMode {
    pub const fn salt_length(&self) -> usize {
        self.key_length() / 2
    }

    pub const fn key_length(&self) -> usize {
        match self {
            Self::Aes128 => 16,
            Self::Aes192 => 24,
            Self::Aes256 => 32,
        }
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn system() {
        use super::System;
        assert_eq!(u8::from(System::Dos), 0u8);
        assert_eq!(System::Dos as u8, 0u8);
        assert_eq!(System::Unix as u8, 3u8);
        assert_eq!(u8::from(System::Unix), 3u8);
        assert_eq!(System::from(0), System::Dos);
        assert_eq!(System::from(3), System::Unix);
        assert_eq!(u8::from(System::Unknown), 4u8);
        assert_eq!(System::Unknown as u8, 4u8);
    }

    #[test]
    fn sanitize() {
        use super::*;
        let file_name = "/path/../../../../etc/./passwd\0/etc/shadow".to_string();
        let data = ZipFileData {
            system: System::Dos,
            version_made_by: 0,
            encrypted: false,
            using_data_descriptor: false,
            compression_method: crate::compression::CompressionMethod::Stored,
            compression_level: None,
            last_modified_time: DateTime::default(),
            crc32: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            file_name: file_name.clone().into_boxed_str(),
            file_name_raw: file_name.into_bytes().into_boxed_slice(),
            extra_field: None,
            central_extra_field: None,
            file_comment: String::with_capacity(0).into_boxed_str(),
            header_start: 0,
            data_start: OnceLock::new(),
            central_header_start: 0,
            external_attributes: 0,
            large_file: false,
            aes_mode: None,
            extra_fields: Vec::new(),
        };
        assert_eq!(data.file_name_sanitized(), PathBuf::from("path/etc/passwd"));
    }

    #[test]
    #[allow(clippy::unusual_byte_groupings)]
    fn datetime_default() {
        use super::DateTime;
        let dt = DateTime::default();
        assert_eq!(dt.timepart(), 0);
        assert_eq!(dt.datepart(), 0b0000000_0001_00001);
    }

    #[test]
    #[allow(clippy::unusual_byte_groupings)]
    fn datetime_max() {
        use super::DateTime;
        let dt = DateTime::from_date_and_time(2107, 12, 31, 23, 59, 60).unwrap();
        assert_eq!(dt.timepart(), 0b10111_111011_11110);
        assert_eq!(dt.datepart(), 0b1111111_1100_11111);
    }

    #[test]
    fn datetime_bounds() {
        use super::DateTime;

        assert!(DateTime::from_date_and_time(2000, 1, 1, 23, 59, 60).is_ok());
        assert!(DateTime::from_date_and_time(2000, 1, 1, 24, 0, 0).is_err());
        assert!(DateTime::from_date_and_time(2000, 1, 1, 0, 60, 0).is_err());
        assert!(DateTime::from_date_and_time(2000, 1, 1, 0, 0, 61).is_err());

        assert!(DateTime::from_date_and_time(2107, 12, 31, 0, 0, 0).is_ok());
        assert!(DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).is_ok());
        assert!(DateTime::from_date_and_time(1979, 1, 1, 0, 0, 0).is_err());
        assert!(DateTime::from_date_and_time(1980, 0, 1, 0, 0, 0).is_err());
        assert!(DateTime::from_date_and_time(1980, 1, 0, 0, 0, 0).is_err());
        assert!(DateTime::from_date_and_time(2108, 12, 31, 0, 0, 0).is_err());
        assert!(DateTime::from_date_and_time(2107, 13, 31, 0, 0, 0).is_err());
        assert!(DateTime::from_date_and_time(2107, 12, 32, 0, 0, 0).is_err());
    }

    #[cfg(feature = "time")]
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};

    #[cfg(feature = "time")]
    #[test]
    fn datetime_try_from_bounds() {
        use std::convert::TryFrom;

        use super::DateTime;
        use time::macros::datetime;

        // 1979-12-31 23:59:59
        assert!(DateTime::try_from(datetime!(1979-12-31 23:59:59 UTC)).is_err());

        // 1980-01-01 00:00:00
        assert!(DateTime::try_from(datetime!(1980-01-01 00:00:00 UTC)).is_ok());

        // 2107-12-31 23:59:59
        assert!(DateTime::try_from(datetime!(2107-12-31 23:59:59 UTC)).is_ok());

        // 2108-01-01 00:00:00
        assert!(DateTime::try_from(datetime!(2108-01-01 00:00:00 UTC)).is_err());
    }

    #[test]
    fn time_conversion() {
        use super::DateTime;
        let dt = DateTime::from_msdos(0x4D71, 0x54CF);
        assert_eq!(dt.year(), 2018);
        assert_eq!(dt.month(), 11);
        assert_eq!(dt.day(), 17);
        assert_eq!(dt.hour(), 10);
        assert_eq!(dt.minute(), 38);
        assert_eq!(dt.second(), 30);

        #[cfg(feature = "time")]
        assert_eq!(
            dt.to_time().unwrap().format(&Rfc3339).unwrap(),
            "2018-11-17T10:38:30Z"
        );
    }

    #[test]
    fn time_out_of_bounds() {
        use super::DateTime;
        let dt = DateTime::from_msdos(0xFFFF, 0xFFFF);
        assert_eq!(dt.year(), 2107);
        assert_eq!(dt.month(), 15);
        assert_eq!(dt.day(), 31);
        assert_eq!(dt.hour(), 31);
        assert_eq!(dt.minute(), 63);
        assert_eq!(dt.second(), 62);

        #[cfg(feature = "time")]
        assert!(dt.to_time().is_err());

        let dt = DateTime::from_msdos(0x0000, 0x0000);
        assert_eq!(dt.year(), 1980);
        assert_eq!(dt.month(), 0);
        assert_eq!(dt.day(), 0);
        assert_eq!(dt.hour(), 0);
        assert_eq!(dt.minute(), 0);
        assert_eq!(dt.second(), 0);

        #[cfg(feature = "time")]
        assert!(dt.to_time().is_err());
    }

    #[cfg(feature = "time")]
    #[test]
    fn time_at_january() {
        use super::DateTime;
        use std::convert::TryFrom;

        // 2020-01-01 00:00:00
        let clock = OffsetDateTime::from_unix_timestamp(1_577_836_800).unwrap();

        assert!(DateTime::try_from(clock).is_ok());
    }
}
