//! On-disk JSON form of a `SearchQuery`. Field and variant names here are a
//! storage format, deliberately decoupled from Rust names so refactoring the
//! domain never breaks existing databases.

use application::ports::StoreError;
use domain::{FileCategory, HiddenItems, ModifiedWithin, SearchQuery, SizeClass};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct QueryRecord {
    text: String,
    category: Option<String>,
    modified: String,
    size: String,
    include_hidden: bool,
}

pub(crate) fn encode(query: &SearchQuery) -> Result<String, StoreError> {
    let record = QueryRecord {
        text: query.text.clone(),
        category: query.category.map(|c| category_name(c).to_owned()),
        modified: modified_name(query.modified).to_owned(),
        size: size_name(query.size).to_owned(),
        include_hidden: query.hidden == HiddenItems::Include,
    };
    serde_json::to_string(&record).map_err(|e| StoreError::Corrupt(e.to_string()))
}

pub(crate) fn decode(json: &str) -> Result<SearchQuery, StoreError> {
    let corrupt = |what: &str| StoreError::Corrupt(format!("unknown {what} in saved search"));
    let record: QueryRecord = serde_json::from_str(json).map_err(|e| StoreError::Corrupt(e.to_string()))?;
    Ok(SearchQuery {
        text: record.text,
        category: record.category.map(|c| category_from(&c).ok_or_else(|| corrupt("kind"))).transpose()?,
        modified: modified_from(&record.modified).ok_or_else(|| corrupt("date range"))?,
        size: size_from(&record.size).ok_or_else(|| corrupt("size"))?,
        hidden: if record.include_hidden { HiddenItems::Include } else { HiddenItems::Exclude },
    })
}

const CATEGORIES: &[(FileCategory, &str)] = &[
    (FileCategory::Folder, "folder"),
    (FileCategory::Image, "image"),
    (FileCategory::Pdf, "pdf"),
    (FileCategory::Video, "video"),
    (FileCategory::Audio, "audio"),
    (FileCategory::Text, "text"),
    (FileCategory::Archive, "archive"),
    (FileCategory::Document, "document"),
    (FileCategory::Other, "other"),
];
const MODIFIED: &[(ModifiedWithin, &str)] = &[
    (ModifiedWithin::Any, "any"),
    (ModifiedWithin::Day, "day"),
    (ModifiedWithin::Week, "week"),
    (ModifiedWithin::Month, "month"),
    (ModifiedWithin::Year, "year"),
];
const SIZES: &[(SizeClass, &str)] = &[
    (SizeClass::Any, "any"),
    (SizeClass::Small, "small"),
    (SizeClass::Medium, "medium"),
    (SizeClass::Large, "large"),
    (SizeClass::Huge, "huge"),
];

fn name_of<T: PartialEq + Copy>(table: &'static [(T, &'static str)], value: T) -> &'static str {
    table.iter().find(|(v, _)| *v == value).map_or("", |(_, name)| name)
}

fn value_of<T: Copy>(table: &[(T, &str)], name: &str) -> Option<T> {
    table.iter().find(|(_, n)| *n == name).map(|(v, _)| *v)
}

fn category_name(category: FileCategory) -> &'static str {
    name_of(CATEGORIES, category)
}
fn category_from(name: &str) -> Option<FileCategory> {
    value_of(CATEGORIES, name)
}
fn modified_name(modified: ModifiedWithin) -> &'static str {
    name_of(MODIFIED, modified)
}
fn modified_from(name: &str) -> Option<ModifiedWithin> {
    value_of(MODIFIED, name)
}
fn size_name(size: SizeClass) -> &'static str {
    name_of(SIZES, size)
}
fn size_from(name: &str) -> Option<SizeClass> {
    value_of(SIZES, name)
}
