//! Code related to Date time in zip files

/// Representation of a moment in time.
///
/// Zip files use an old format from DOS to store timestamps,
/// with its own set of peculiarities.
/// For example, it has a resolution of 2 seconds!
///
/// A [`DateTime`] can be stored directly in a zipfile with [`FileOptions::last_modified_time`],
/// or read from one with [`ZipFile::last_modified`](crate::read::ZipFile::last_modified).
///
/// # Warning
///
/// Because there is no timezone associated with the [`DateTime`], they should ideally only
/// be used for user-facing descriptions.
///
/// Modern zip files store more precise timestamps; see [`crate::extra_fields::ExtendedTimestamp`]
/// for details.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DateTime {
    datepart: u16,
    timepart: u16,
}

impl Debug for DateTime {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if *self == Self::default() {
            return f.write_str("DateTime::default()");
        }
        f.write_fmt(format_args!(
            "DateTime::from_date_and_time({}, {}, {}, {}, {}, {})?",
            self.year(),
            self.month(),
            self.day(),
            self.hour(),
            self.minute(),
            self.second()
        ))
    }
}

impl DateTime {
    /// Constructs a default datetime of 1980-01-01 00:00:00.
    pub const DEFAULT: Self = DateTime {
        datepart: 0b0000_0000_0010_0001,
        timepart: 0,
    };

    /// Returns the current time if possible, otherwise the default of 1980-01-01.
    #[cfg(feature = "time")]
    #[must_use]
    pub fn default_for_write() -> Self {
        let now = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(now.date(), now.time())
            .try_into()
            .unwrap_or_else(|_| DateTime::default())
    }

    /// Returns the current time if possible, otherwise the default of 1980-01-01.
    #[cfg(not(feature = "time"))]
    #[must_use]
    pub fn default_for_write() -> Self {
        DateTime::default()
    }
}

#[cfg(feature = "_arbitrary")]
impl arbitrary::Arbitrary<'_> for DateTime {
    fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        // DOS time format stores seconds divided by 2 in a 5-bit field (0..=29),
        // so the maximum representable second value is 58.
        const MAX_DOS_SECONDS: u16 = 58;

        let year: u16 = u.int_in_range(1980..=2107)?;
        let month: u16 = u.int_in_range(1..=12)?;
        let day: u16 = u.int_in_range(1..=31)?;
        let datepart = day | (month << 5) | ((year - 1980) << 9);
        let hour: u16 = u.int_in_range(0..=23)?;
        let minute: u16 = u.int_in_range(0..=59)?;
        let second: u16 = u.int_in_range(0..=MAX_DOS_SECONDS)?;
        let timepart = (second >> 1) | (minute << 5) | (hour << 11);
        Ok(DateTime { datepart, timepart })
    }
}

#[cfg(feature = "chrono")]
impl TryFrom<chrono::NaiveDateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_from(value: chrono::NaiveDateTime) -> Result<Self, Self::Error> {
        use chrono::{Datelike, Timelike};

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
impl TryFrom<DateTime> for chrono::NaiveDateTime {
    type Error = DateTimeRangeError;

    fn try_from(value: DateTime) -> Result<Self, Self::Error> {
        let date = chrono::NaiveDate::from_ymd_opt(
            value.year().into(),
            value.month().into(),
            value.day().into(),
        )
        .ok_or(DateTimeRangeError)?;
        let time = chrono::NaiveTime::from_hms_opt(
            value.hour().into(),
            value.minute().into(),
            value.second().into(),
        )
        .ok_or(DateTimeRangeError)?;
        Ok(chrono::NaiveDateTime::new(date, time))
    }
}

#[cfg(feature = "jiff-02")]
impl TryFrom<jiff::civil::DateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_from(value: jiff::civil::DateTime) -> Result<Self, Self::Error> {
        Self::from_date_and_time(
            value.year().try_into()?,
            value.month() as u8,
            value.day() as u8,
            value.hour() as u8,
            value.minute() as u8,
            value.second() as u8,
        )
    }
}

#[cfg(feature = "jiff-02")]
impl TryFrom<DateTime> for jiff::civil::DateTime {
    type Error = jiff::Error;

    fn try_from(value: DateTime) -> Result<Self, Self::Error> {
        Self::new(
            value.year() as i16,
            value.month() as i8,
            value.day() as i8,
            value.hour() as i8,
            value.minute() as i8,
            value.second() as i8,
            0,
        )
    }
}

impl TryFrom<(u16, u16)> for DateTime {
    type Error = DateTimeRangeError;

    #[inline]
    fn try_from(values: (u16, u16)) -> Result<Self, Self::Error> {
        Self::try_from_msdos(values.0, values.1)
    }
}

impl From<DateTime> for (u16, u16) {
    #[inline]
    fn from(dt: DateTime) -> Self {
        (dt.datepart(), dt.timepart())
    }
}

impl Default for DateTime {
    /// Constructs an 'default' datetime of 1980-01-01 00:00:00
    fn default() -> DateTime {
        DateTime::DEFAULT
    }
}

impl fmt::Display for DateTime {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year(),
            self.month(),
            self.day(),
            self.hour(),
            self.minute(),
            self.second()
        )
    }
}

impl DateTime {
    /// Converts an msdos (u16, u16) pair to a `DateTime` object
    ///
    /// # Safety
    /// The caller must ensure the date and time are valid.
    #[must_use]
    pub const unsafe fn from_msdos_unchecked(datepart: u16, timepart: u16) -> DateTime {
        DateTime { datepart, timepart }
    }

    pub(crate) fn is_leap_year(year: u16) -> bool {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    }

    /// Converts an msdos (u16, u16) pair to a `DateTime` object if it represents a valid date and
    /// time.
    pub fn try_from_msdos(datepart: u16, timepart: u16) -> Result<DateTime, DateTimeRangeError> {
        let seconds = (timepart & 0b0000_0000_0001_1111) << 1;
        let minutes = (timepart & 0b0000_0111_1110_0000) >> 5;
        let hours = (timepart & 0b1111_1000_0000_0000) >> 11;
        let days = datepart & 0b0000_0000_0001_1111;
        let months = (datepart & 0b0000_0001_1110_0000) >> 5;
        let years = (datepart & 0b1111_1110_0000_0000) >> 9;
        Self::from_date_and_time(
            years.checked_add(1980).ok_or(DateTimeRangeError)?,
            months.try_into()?,
            days.try_into()?,
            hours.try_into()?,
            minutes.try_into()?,
            seconds.try_into()?,
        )
    }

    /// Constructs a `DateTime` from a specific date and time
    ///
    /// The bounds are:
    /// * year: [1980, 2107]
    /// * month: [1, 12]
    /// * day: [1, 28..=31]
    /// * hour: [0, 23]
    /// * minute: [0, 59]
    /// * second: [0, 60] (rounded down to even and to [0, 58] due to ZIP format limitation)
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
            // DOS/ZIP timestamp stores seconds/2 in 5 bits and cannot represent 59 or 60 seconds (incl. leap seconds)
            let second = second.min(58);
            let max_day = match month {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 if Self::is_leap_year(year) => 29,
                2 => 28,
                _ => unreachable!(),
            };
            if day > max_day {
                return Err(DateTimeRangeError);
            }
            let datepart = u16::from(day) | (u16::from(month) << 5) | ((year - 1980) << 9);
            let timepart =
                (u16::from(second) >> 1) | (u16::from(minute) << 5) | (u16::from(hour) << 11);
            Ok(DateTime { datepart, timepart })
        } else {
            Err(DateTimeRangeError)
        }
    }

    /// Indicates whether this date and time can be written to a zip archive.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        Self::try_from_msdos(self.datepart, self.timepart).is_ok()
    }

    /// Gets the time portion of this datetime in the msdos representation
    #[must_use]
    pub const fn timepart(&self) -> u16 {
        self.timepart
    }

    /// Gets the date portion of this datetime in the msdos representation
    #[must_use]
    pub const fn datepart(&self) -> u16 {
        self.datepart
    }

    /// Get the year. There is no epoch, i.e. 2018 will be returned as 2018.
    #[must_use]
    pub const fn year(&self) -> u16 {
        (self.datepart >> 9) + 1980
    }

    /// Get the month, where 1 = january and 12 = december
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    #[must_use]
    pub const fn month(&self) -> u8 {
        ((self.datepart & 0b0000_0001_1110_0000) >> 5) as u8
    }

    /// Get the day
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    #[must_use]
    pub const fn day(&self) -> u8 {
        (self.datepart & 0b0000_0000_0001_1111) as u8
    }

    /// Get the hour
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    #[must_use]
    pub const fn hour(&self) -> u8 {
        (self.timepart >> 11) as u8
    }

    /// Get the minute
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    #[must_use]
    pub const fn minute(&self) -> u8 {
        ((self.timepart & 0b0000_0111_1110_0000) >> 5) as u8
    }

    /// Get the second
    ///
    /// # Warning
    ///
    /// When read from a zip file, this may not be a reasonable value
    #[must_use]
    pub const fn second(&self) -> u8 {
        ((self.timepart & 0b0000_0000_0001_1111) << 1) as u8
    }
}

#[cfg(all(feature = "time", feature = "deprecated-time"))]
impl TryFrom<time::OffsetDateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_from(dt: time::OffsetDateTime) -> Result<Self, Self::Error> {
        Self::try_from(time::PrimitiveDateTime::new(dt.date(), dt.time()))
    }
}

#[cfg(feature = "time")]
impl TryFrom<time::PrimitiveDateTime> for DateTime {
    type Error = DateTimeRangeError;

    fn try_from(dt: time::PrimitiveDateTime) -> Result<Self, Self::Error> {
        Self::from_date_and_time(
            dt.year().try_into()?,
            dt.month().into(),
            dt.day(),
            dt.hour(),
            dt.minute(),
            dt.second(),
        )
    }
}

#[cfg(all(feature = "time", feature = "deprecated-time"))]
impl TryFrom<DateTime> for time::OffsetDateTime {
    type Error = time::error::ComponentRange;

    fn try_from(dt: DateTime) -> Result<Self, Self::Error> {
        time::PrimitiveDateTime::try_from(dt).map(time::PrimitiveDateTime::assume_utc)
    }
}

#[cfg(feature = "time")]
impl TryFrom<DateTime> for time::PrimitiveDateTime {
    type Error = time::error::ComponentRange;

    fn try_from(dt: DateTime) -> Result<Self, Self::Error> {
        use time::{Date, Month, Time};
        let date =
            Date::from_calendar_date(i32::from(dt.year()), Month::try_from(dt.month())?, dt.day())?;
        let time = Time::from_hms(dt.hour(), dt.minute(), dt.second())?;
        Ok(time::PrimitiveDateTime::new(date, time))
    }
}
