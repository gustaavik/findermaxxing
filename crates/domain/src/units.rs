/// File size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteSize(pub u64);

/// Milliseconds since the Unix epoch (negative for earlier instants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    unix_millis: i64,
}

impl Timestamp {
    #[must_use]
    pub const fn from_unix_millis(unix_millis: i64) -> Self {
        Self { unix_millis }
    }

    #[must_use]
    pub const fn unix_millis(self) -> i64 {
        self.unix_millis
    }

    /// The calendar date and time at a UTC offset.
    #[must_use]
    pub fn civil(self, utc_offset_minutes: i32) -> CivilTime {
        const MILLIS_PER_SECOND: i64 = 1000;
        const SECONDS_PER_DAY: i64 = 86_400;
        let seconds = self.unix_millis.div_euclid(MILLIS_PER_SECOND) + i64::from(utc_offset_minutes) * 60;
        let (days, second_of_day) = (seconds.div_euclid(SECONDS_PER_DAY), seconds.rem_euclid(SECONDS_PER_DAY));
        let (year, month, day) = crate::natural_query::civil_from_days(days);
        CivilTime {
            year,
            month,
            day,
            hour: second_of_day / 3600,
            minute: second_of_day / 60 % 60,
            second: second_of_day % 60,
        }
    }
}

/// A date and time of day as people read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
}

/// POSIX permission bits (`st_mode & 0o7777`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnixMode(u32);

impl UnixMode {
    const PERMISSION_MASK: u32 = 0o7777;

    #[must_use]
    pub const fn from_mode(mode: u32) -> Self {
        Self(mode & Self::PERMISSION_MASK)
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }
}
