//! Translation between the search filter enums QML uses and domain queries.

use domain::{FileCategory, HiddenItems, ModifiedWithin, SearchQuery, SizeClass};

use crate::folder_view_controller::qobject::{SearchDate, SearchKind, SearchSize};

/// The filter controls' state.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SearchOptions {
    pub text: String,
    pub kind: SearchKind,
    pub date: SearchDate,
    pub size: SearchSize,
}

// The generated qenums have no Debug; show their raw values.
impl std::fmt::Debug for SearchOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchOptions")
            .field("text", &self.text)
            .field("kind", &self.kind.repr)
            .field("date", &self.date.repr)
            .field("size", &self.size.repr)
            .finish()
    }
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self { text: String::new(), kind: SearchKind::AnyKind, date: SearchDate::AnyDate, size: SearchSize::AnySize }
    }
}

const KINDS: &[(SearchKind, FileCategory)] = &[
    (SearchKind::Folders, FileCategory::Folder),
    (SearchKind::Images, FileCategory::Image),
    (SearchKind::Pdfs, FileCategory::Pdf),
    (SearchKind::Videos, FileCategory::Video),
    (SearchKind::Music, FileCategory::Audio),
    (SearchKind::Text, FileCategory::Text),
    (SearchKind::Archives, FileCategory::Archive),
    (SearchKind::Documents, FileCategory::Document),
];
const DATES: &[(SearchDate, ModifiedWithin)] = &[
    (SearchDate::AnyDate, ModifiedWithin::Any),
    (SearchDate::PastDay, ModifiedWithin::Day),
    (SearchDate::PastWeek, ModifiedWithin::Week),
    (SearchDate::PastMonth, ModifiedWithin::Month),
    (SearchDate::PastYear, ModifiedWithin::Year),
];
const SIZES: &[(SearchSize, SizeClass)] = &[
    (SearchSize::AnySize, SizeClass::Any),
    (SearchSize::Small, SizeClass::Small),
    (SearchSize::Medium, SizeClass::Medium),
    (SearchSize::Large, SizeClass::Large),
    (SearchSize::Huge, SizeClass::Huge),
];

pub(crate) fn to_query(options: &SearchOptions, show_hidden: bool) -> SearchQuery {
    SearchQuery {
        text: options.text.trim().to_owned(),
        category: KINDS.iter().find(|(kind, _)| *kind == options.kind).map(|(_, category)| *category),
        modified: DATES.iter().find(|(date, _)| *date == options.date).map_or(ModifiedWithin::Any, |(_, m)| *m),
        size: SIZES.iter().find(|(size, _)| *size == options.size).map_or(SizeClass::Any, |(_, s)| *s),
        hidden: if show_hidden { HiddenItems::Include } else { HiddenItems::Exclude },
    }
}

pub(crate) fn from_query(query: &SearchQuery) -> SearchOptions {
    SearchOptions {
        text: query.text.clone(),
        kind: KINDS.iter().find(|(_, c)| Some(*c) == query.category).map_or(SearchKind::AnyKind, |(k, _)| *k),
        date: DATES.iter().find(|(_, m)| *m == query.modified).map_or(SearchDate::AnyDate, |(d, _)| *d),
        size: SIZES.iter().find(|(_, s)| *s == query.size).map_or(SearchSize::AnySize, |(z, _)| *z),
    }
}

pub(crate) fn with_hidden(query: SearchQuery, show_hidden: bool) -> SearchQuery {
    SearchQuery { hidden: if show_hidden { HiddenItems::Include } else { HiddenItems::Exclude }, ..query }
}

pub(crate) fn status_text(folders_searched: u64, unreadable_folders: u64) -> String {
    let searched = if folders_searched == 1 {
        "1 folder searched".to_owned()
    } else {
        format!("{folders_searched} folders searched")
    };
    match unreadable_folders {
        0 => searched,
        1 => format!("{searched}, 1 couldn't be read"),
        n => format!("{searched}, {n} couldn't be read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_through_queries() {
        let options = SearchOptions {
            text: " report ".into(),
            kind: SearchKind::Pdfs,
            date: SearchDate::PastWeek,
            size: SearchSize::Huge,
        };
        let query = to_query(&options, false);
        assert_eq!(query.category, Some(FileCategory::Pdf));
        assert_eq!(from_query(&query), SearchOptions { text: "report".into(), ..options });
    }

    #[test]
    fn status_mentions_unreadable_folders_only_when_there_are_some() {
        assert_eq!(status_text(1, 0), "1 folder searched");
        assert_eq!(status_text(12, 2), "12 folders searched, 2 couldn't be read");
    }
}
