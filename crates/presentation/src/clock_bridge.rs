//! The current time in the user's time zone (C++ shim over `QDateTime`).

use std::time::{SystemTime, UNIX_EPOCH};

use domain::{QueryClock, Timestamp};

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("presentation/cpp/clock.h");

        #[cxx_name = "utcOffsetSeconds"]
        fn utc_offset_seconds() -> i32;
    }
}

const SECONDS_PER_MINUTE: i32 = 60;

pub(crate) fn query_clock() -> QueryClock {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    QueryClock {
        now: Timestamp::from_unix_millis(i64::try_from(millis).unwrap_or(i64::MAX)),
        utc_offset_minutes: ffi::utc_offset_seconds() / SECONDS_PER_MINUTE,
    }
}
