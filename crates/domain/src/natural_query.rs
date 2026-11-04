//! Natural-language search queries: "invoices from last spring", "photos of
//! the beach", "billeder fra sidste sommer". Date phrases and kind words
//! become filters; what remains is the meaning to search for.
//!
//! A deterministic, rule-based parser in English and Danish, resolved against
//! an injected clock, so it is testable and never needs a network or a model.

use crate::category::FileCategory;
use crate::units::Timestamp;

const MILLIS_PER_DAY: i64 = 86_400_000;
const MILLIS_PER_MINUTE: i64 = 60_000;
/// "recently": the last 30 days.
const RECENT_DAYS: i64 = 30;

/// When "now" is, in the user's time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryClock {
    pub now: Timestamp,
    pub utc_offset_minutes: i32,
}

/// A half-open span of modification times, `[from, until)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateRange {
    pub from: Timestamp,
    pub until: Timestamp,
}

impl DateRange {
    #[must_use]
    pub fn contains(&self, time: Timestamp) -> bool {
        self.from <= time && time < self.until
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    /// What to search for by meaning; may be empty ("photos from last week").
    pub text: String,
    pub modified: Option<DateRange>,
    pub category: Option<FileCategory>,
}

/// A calendar day in the user's time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Day {
    year: i64,
    month: i64,
    day: i64,
}

impl Day {
    fn from_days(days: i64) -> Self {
        let (year, month, day) = civil_from_days(days);
        Self { year, month, day }
    }

    fn days(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    fn first_of(year: i64, month: i64) -> Self {
        // Months past December roll into the next year.
        let (year, month) = (year + (month - 1).div_euclid(12), (month - 1).rem_euclid(12) + 1);
        Self { year, month, day: 1 }
    }
}

struct Calendar {
    clock: QueryClock,
    today: Day,
}

impl Calendar {
    fn new(clock: QueryClock) -> Self {
        let local = clock.now.unix_millis() + i64::from(clock.utc_offset_minutes) * MILLIS_PER_MINUTE;
        Self { clock, today: Day::from_days(local.div_euclid(MILLIS_PER_DAY)) }
    }

    /// Local midnight of `day`, as a timestamp.
    fn start(&self, day: Day) -> Timestamp {
        Timestamp::from_unix_millis(
            day.days() * MILLIS_PER_DAY - i64::from(self.clock.utc_offset_minutes) * MILLIS_PER_MINUTE,
        )
    }

    fn range(&self, from: Day, until: Day) -> DateRange {
        DateRange { from: self.start(from), until: self.start(until) }
    }

    fn days_back(&self, count: i64) -> DateRange {
        let tomorrow = Day::from_days(self.today.days() + 1);
        self.range(Day::from_days(self.today.days() - count + 1), tomorrow)
    }

    /// Monday-based weeks.
    fn week(&self, offset: i64) -> DateRange {
        let weekday = (self.today.days() + 3).rem_euclid(7); // 1970-01-01 was a Thursday.
        let monday = self.today.days() - weekday + offset * 7;
        self.range(Day::from_days(monday), Day::from_days(monday + 7))
    }

    fn month(&self, year: i64, month: i64) -> DateRange {
        self.range(Day::first_of(year, month), Day::first_of(year, month + 1))
    }

    fn year(&self, year: i64) -> DateRange {
        self.range(Day::first_of(year, 1), Day::first_of(year + 1, 1))
    }

    /// The most recent `month` that has begun (this year's, or last year's).
    fn latest_month(&self, month: i64) -> DateRange {
        let year = if month <= self.today.month { self.today.year } else { self.today.year - 1 };
        self.month(year, month)
    }

    /// Northern-hemisphere meteorological seasons; winter spans the new year.
    fn season(&self, season: Season, which: Which) -> DateRange {
        let first_month = season.first_month();
        // The season instance that contains (or last began before) today.
        let mut year = self.today.year;
        if Day::first_of(year, first_month) > self.today {
            year -= 1;
        }
        let current = (year, Day::first_of(year, first_month));
        let current_end = Day::first_of(current.0, first_month + 3);
        let year = match which {
            // "last spring": the latest one that has ended.
            Which::Last if current_end > self.today => current.0 - 1,
            Which::Last | Which::This => current.0,
        };
        self.range(Day::first_of(year, first_month), Day::first_of(year, first_month + 3))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    fn first_month(self) -> i64 {
        match self {
            Self::Spring => 3,
            Self::Summer => 6,
            Self::Autumn => 9,
            Self::Winter => 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    This,
    Last,
}

const SEASONS: &[(&str, Season)] = &[
    ("spring", Season::Spring),
    ("summer", Season::Summer),
    ("autumn", Season::Autumn),
    ("fall", Season::Autumn),
    ("winter", Season::Winter),
    ("forår", Season::Spring),
    ("foråret", Season::Spring),
    ("sommer", Season::Summer),
    ("sommeren", Season::Summer),
    ("efterår", Season::Autumn),
    ("efteråret", Season::Autumn),
    ("vinter", Season::Winter),
    ("vinteren", Season::Winter),
];

const MONTHS: &[(&str, i64)] = &[
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
    ("januar", 1),
    ("februar", 2),
    ("marts", 3),
    ("maj", 5),
    ("juni", 6),
    ("juli", 7),
    ("oktober", 10),
];

const THIS: &[&str] = &["this", "denne", "dette", "i"];
const LAST: &[&str] = &["last", "past", "previous", "sidste", "forrige", "seneste"];

/// Words that only glue a date or kind to the rest ("from", "of the").
const FILLERS: &[&str] = &[
    "from", "in", "during", "of", "the", "a", "an", "since", "taken", "made", "with", "about", "my", "all", "fra", "i",
    "under", "af", "med", "om", "mine", "min", "alle", "taget",
];

const CATEGORIES: &[(&str, FileCategory)] = &[
    ("photos", FileCategory::Image),
    ("photo", FileCategory::Image),
    ("pictures", FileCategory::Image),
    ("picture", FileCategory::Image),
    ("images", FileCategory::Image),
    ("pics", FileCategory::Image),
    ("screenshots", FileCategory::Image),
    ("billeder", FileCategory::Image),
    ("billede", FileCategory::Image),
    ("fotos", FileCategory::Image),
    ("foto", FileCategory::Image),
    ("videos", FileCategory::Video),
    ("video", FileCategory::Video),
    ("movies", FileCategory::Video),
    ("videoer", FileCategory::Video),
    ("film", FileCategory::Video),
    ("music", FileCategory::Audio),
    ("songs", FileCategory::Audio),
    ("audio", FileCategory::Audio),
    ("musik", FileCategory::Audio),
    ("sange", FileCategory::Audio),
    ("pdfs", FileCategory::Pdf),
    ("pdf", FileCategory::Pdf),
    ("documents", FileCategory::Document),
    ("docs", FileCategory::Document),
    ("dokumenter", FileCategory::Document),
    ("spreadsheets", FileCategory::Document),
    ("presentations", FileCategory::Document),
    ("archives", FileCategory::Archive),
    ("zips", FileCategory::Archive),
];

/// Parses `input` against `clock`. Unrecognised words are kept as the text.
#[must_use]
pub fn parse_query(input: &str, clock: QueryClock) -> ParsedQuery {
    let calendar = Calendar::new(clock);
    let words: Vec<String> =
        input.split_whitespace().map(|w| w.trim_matches(|c: char| ",.;:!?\"'".contains(c)).to_owned()).collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut keep = vec![true; words.len()];
    let mut modified = None;
    let mut category = None;
    let mut index = 0;
    while index < lower.len() {
        if let Some((range, used)) = date_at(&calendar, &lower[index..]) {
            modified.get_or_insert(range);
            keep[index..index + used].iter_mut().for_each(|k| *k = false);
            drop_fillers_before(&lower, &mut keep, index);
            index += used;
            continue;
        }
        if let Some(found) = CATEGORIES.iter().find(|(word, _)| *word == lower[index]).map(|(_, c)| *c) {
            category.get_or_insert(found);
            keep[index] = false;
        }
        index += 1;
    }
    let text = trimmed_text(&words, &lower, &keep);
    ParsedQuery { text, modified, category }
}

/// "from", "in", "fra" right before a date phrase belong to it.
fn drop_fillers_before(lower: &[String], keep: &mut [bool], mut index: usize) {
    while index > 0 && keep[index - 1] && FILLERS.contains(&lower[index - 1].as_str()) {
        keep[index - 1] = false;
        index -= 1;
    }
}

/// The kept words, without filler words at either end ("of the beach" → "beach").
fn trimmed_text(words: &[String], lower: &[String], keep: &[bool]) -> String {
    let kept: Vec<usize> = (0..words.len()).filter(|i| keep[*i] && !words[*i].is_empty()).collect();
    let is_filler = |i: &usize| FILLERS.contains(&lower[*i].as_str());
    let start = kept.iter().position(|i| !is_filler(i)).unwrap_or(kept.len());
    let end = kept.iter().rposition(|i| !is_filler(i)).map_or(start, |e| e + 1);
    kept[start..end.max(start)].iter().map(|i| words[*i].as_str()).collect::<Vec<_>>().join(" ")
}

/// A date phrase starting at `words[0]`, and how many words it used.
fn date_at(calendar: &Calendar, words: &[String]) -> Option<(DateRange, usize)> {
    let word = |i: usize| words.get(i).map(String::as_str);
    let today = calendar.today;
    match (word(0)?, word(1), word(2)) {
        ("today" | "idag", ..) => Some((calendar.days_back(1), 1)),
        ("i", Some("dag"), _) => Some((calendar.days_back(1), 2)),
        ("yesterday" | "igår", ..) => {
            let yesterday = Day::from_days(today.days() - 1);
            Some((calendar.range(yesterday, today), 1))
        }
        ("i", Some("går"), _) => {
            let yesterday = Day::from_days(today.days() - 1);
            Some((calendar.range(yesterday, today), 2))
        }
        ("recent" | "recently" | "nylig" | "nyligt", ..) => Some((calendar.days_back(RECENT_DAYS), 1)),
        ("for", Some("nylig"), _) => Some((calendar.days_back(RECENT_DAYS), 2)),
        ("i", Some("år"), _) => Some((calendar.year(today.year), 2)),
        (which, Some(unit), next) if THIS.contains(&which) || LAST.contains(&which) => {
            let which = if LAST.contains(&which) { Which::Last } else { Which::This };
            relative(calendar, which, unit, next)
        }
        (first, second, _) => absolute(calendar, first, second),
    }
}

/// "last week", "this month", "last 3 months", "sidste forår"…
fn relative(calendar: &Calendar, which: Which, unit: &str, next: Option<&str>) -> Option<(DateRange, usize)> {
    let today = calendar.today;
    let (offset, this) = match which {
        Which::This => (0, true),
        Which::Last => (-1, false),
    };
    if let Some(count) = unit.parse::<i64>().ok().filter(|n| (1..=1000).contains(n)) {
        let days = match next? {
            "days" | "dage" => count,
            "weeks" | "uger" => count * 7,
            "months" | "måneder" => count * 30,
            "years" | "år" => count * 365,
            _ => return None,
        };
        return Some((calendar.days_back(days), 3));
    }
    let range = match unit {
        "week" | "uge" => calendar.week(offset),
        "month" | "måned" => {
            let month = today.month + offset;
            calendar.month(today.year, month)
        }
        "year" | "år" => calendar.year(today.year + offset),
        _ => {
            if let Some((_, season)) = SEASONS.iter().find(|(name, _)| *name == unit) {
                calendar.season(*season, which)
            } else if let Some((_, month)) = MONTHS.iter().find(|(name, _)| *name == unit) {
                if this { calendar.month(today.year, *month) } else { calendar.latest_month(*month) }
            } else {
                return None;
            }
        }
    };
    Some((range, 2))
}

/// "2024", "march", "march 2024", "spring 2024", "foråret 2023".
fn absolute(calendar: &Calendar, first: &str, second: Option<&str>) -> Option<(DateRange, usize)> {
    let year_of = |text: Option<&str>| text?.parse::<i64>().ok().filter(|y| (1970..=2200).contains(y));
    if let Some(year) = year_of(Some(first)) {
        return Some((calendar.year(year), 1));
    }
    if let Some((_, month)) = MONTHS.iter().find(|(name, _)| *name == first) {
        return Some(match year_of(second) {
            Some(year) => (calendar.month(year, *month), 2),
            None => (calendar.latest_month(*month), 1),
        });
    }
    let (_, season) = SEASONS.iter().find(|(name, _)| *name == first)?;
    let year = year_of(second)?;
    let start = Day::first_of(year, season.first_month());
    Some((calendar.range(start, Day::first_of(year, season.first_month() + 3)), 2))
}

/// Days since 1970-01-01 (Howard Hinnant's civil-date algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 2026-11-04 12:00 in Copenhagen (UTC+1).
    const CLOCK: QueryClock =
        QueryClock { now: Timestamp::from_unix_millis(1_793_790_000_000), utc_offset_minutes: 60 };

    fn day(year: i64, month: i64, day: i64) -> Timestamp {
        Calendar::new(CLOCK).start(Day { year, month, day })
    }

    fn range(from: (i64, i64, i64), until: (i64, i64, i64)) -> Option<DateRange> {
        Some(DateRange { from: day(from.0, from.1, from.2), until: day(until.0, until.1, until.2) })
    }

    #[test]
    fn the_clock_is_where_the_tests_think() {
        assert_eq!(Calendar::new(CLOCK).today, Day { year: 2026, month: 11, day: 4 });
    }

    #[test]
    fn dates_become_filters_and_the_rest_stays_as_meaning() {
        let parsed = parse_query("invoices from last spring", CLOCK);
        assert_eq!(parsed.text, "invoices");
        assert_eq!(parsed.modified, range((2026, 3, 1), (2026, 6, 1)));
        assert_eq!(parsed.category, None);

        let parsed = parse_query("photos of the beach", CLOCK);
        assert_eq!((parsed.text.as_str(), parsed.category), ("beach", Some(FileCategory::Image)));

        let parsed = parse_query("billeder fra sidste sommer", CLOCK);
        assert_eq!((parsed.text.as_str(), parsed.category), ("", Some(FileCategory::Image)));
        assert_eq!(parsed.modified, range((2026, 6, 1), (2026, 9, 1)));
    }

    #[test]
    fn relative_and_absolute_phrases_resolve_against_the_clock() {
        let modified = |q: &str| parse_query(q, CLOCK).modified;
        assert_eq!(modified("notes from yesterday"), range((2026, 11, 3), (2026, 11, 4)));
        assert_eq!(modified("notes i går"), range((2026, 11, 3), (2026, 11, 4)));
        assert_eq!(modified("this week"), range((2026, 11, 2), (2026, 11, 9)), "Monday-based");
        assert_eq!(modified("last month"), range((2026, 10, 1), (2026, 11, 1)));
        assert_eq!(modified("sidste år"), range((2025, 1, 1), (2026, 1, 1)));
        assert_eq!(modified("taxes 2024"), range((2024, 1, 1), (2025, 1, 1)));
        assert_eq!(modified("receipts from december"), range((2025, 12, 1), (2026, 1, 1)), "the latest December");
        assert_eq!(modified("march 2023"), range((2023, 3, 1), (2023, 4, 1)));
        assert_eq!(modified("last winter"), range((2025, 12, 1), (2026, 3, 1)), "winter spans the new year");
        assert_eq!(modified("last 3 days"), range((2026, 11, 2), (2026, 11, 5)));
        assert_eq!(modified("forår 2025"), range((2025, 3, 1), (2025, 6, 1)));
        assert_eq!(modified("this autumn"), range((2026, 9, 1), (2026, 12, 1)));
    }

    #[test]
    fn last_season_skips_the_one_still_running() {
        // 2026-04-15: spring has begun but not ended.
        let april = QueryClock { now: Timestamp::from_unix_millis(1_776_247_200_000), utc_offset_minutes: 0 };
        let calendar = Calendar::new(april);
        assert_eq!(calendar.today, Day { year: 2026, month: 4, day: 15 });
        let parsed = parse_query("last spring", april);
        assert_eq!(parsed.modified, Some(calendar.range(Day::first_of(2025, 3), Day::first_of(2025, 6))));
    }

    #[test]
    fn plain_queries_pass_through() {
        let parsed = parse_query("quarterly report for the board", CLOCK);
        assert_eq!(
            parsed,
            ParsedQuery { text: "quarterly report for the board".into(), modified: None, category: None }
        );
    }
}
