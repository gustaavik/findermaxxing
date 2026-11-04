//! RFC 3339 timestamps, as all three drive APIs send them.

use domain::Timestamp;

const MILLIS_PER_SECOND: i64 = 1000;
const SECONDS_PER_DAY: i64 = 86_400;

/// `2026-11-04T16:39:18Z`, `…18.123Z` or `…18+01:00`.
#[must_use]
pub fn parse_rfc3339(value: &str) -> Option<Timestamp> {
    let (date, time) = value.split_once(['T', 't'])?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, offset_seconds) = split_offset(time)?;
    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = whole.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    let seconds =
        days_from_civil(year, month, day) * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second - offset_seconds;
    Some(Timestamp::from_unix_millis(seconds * MILLIS_PER_SECOND + millis))
}

fn split_offset(time: &str) -> Option<(&str, i64)> {
    if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let index = time.rfind(['+', '-'])?;
    let (clock, offset) = time.split_at(index);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = offset[1..].split_once(':')?;
    Some((clock, sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)))
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_forms_providers_send() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(Timestamp::from_unix_millis(0)));
        assert_eq!(parse_rfc3339("1994-11-06T08:49:37.5Z"), Some(Timestamp::from_unix_millis(784_111_777_500)));
        assert_eq!(parse_rfc3339("1994-11-06T09:49:37+01:00"), Some(Timestamp::from_unix_millis(784_111_777_000)));
        assert_eq!(parse_rfc3339("yesterday"), None);
    }
}
