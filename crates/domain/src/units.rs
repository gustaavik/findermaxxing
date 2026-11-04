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
