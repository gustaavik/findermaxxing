//! Bulk rename: rules that compute new names, and a planner that flags every
//! problem *before* anything is touched, so the preview is the truth.

use std::collections::{HashMap, HashSet};

use regex::{NoExpand, Regex, RegexBuilder};

use crate::entry::FileEntry;
use crate::path::{PathSegment, StoragePath};

const DEFAULT_COUNTER_DIGITS: usize = 1;
const MAX_COUNTER_DIGITS: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matching {
    Text,
    /// A regular expression; the replacement may use `$1`, `${name}`.
    Pattern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseSensitivity {
    Sensitive,
    Insensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddPosition {
    BeforeName,
    AfterName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseChange {
    Lower,
    Upper,
    /// First letter of every word upper case, the rest lower case.
    Title,
}

/// Which part of a name the rule edits. With `Stem`, file extensions are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamePart {
    Stem,
    WholeName,
}

#[derive(Debug, Clone)]
enum Rule {
    Replace { find: Option<Regex>, replacement: String, matching: Matching },
    Add { text: String, position: AddPosition },
    Format { base: String, start: u64, digits: usize },
    ChangeCase(CaseChange),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("invalid pattern: {0}")]
    InvalidPattern(String),
}

/// A configured bulk rename.
#[derive(Debug, Clone)]
pub struct BulkRename {
    rule: Rule,
    part: NamePart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameProblem {
    /// The current name is not valid UTF-8, so text rules can't apply.
    NotText,
    /// Empty, `.`/`..`, or contains `/`.
    InvalidName,
    /// Two items would get the same name.
    DuplicateName,
    /// Another item (not being renamed away) already has that name.
    NameTaken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameOutcome {
    Rename(PathSegment),
    Unchanged,
    Problem(RenameProblem),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRename {
    pub item: StoragePath,
    pub outcome: RenameOutcome,
}

impl BulkRename {
    /// Replace every occurrence of `find`. An empty `find` changes nothing.
    ///
    /// # Errors
    /// [`RuleError::InvalidPattern`] for a malformed regular expression.
    pub fn replace(
        find: &str,
        replacement: &str,
        matching: Matching,
        case: CaseSensitivity,
        part: NamePart,
    ) -> Result<Self, RuleError> {
        let find = if find.is_empty() {
            None
        } else {
            let source = match matching {
                Matching::Text => regex::escape(find),
                Matching::Pattern => find.to_owned(),
            };
            let regex = RegexBuilder::new(&source)
                .case_insensitive(case == CaseSensitivity::Insensitive)
                .build()
                .map_err(|error| RuleError::InvalidPattern(error.to_string()))?;
            Some(regex)
        };
        Ok(Self { rule: Rule::Replace { find, replacement: replacement.to_owned(), matching }, part })
    }

    #[must_use]
    pub fn add(text: &str, position: AddPosition) -> Self {
        Self { rule: Rule::Add { text: text.to_owned(), position }, part: NamePart::Stem }
    }

    /// `base 1`, `base 2`, … (zero-padded to `digits`), keeping extensions.
    #[must_use]
    pub fn format(base: &str, start: u64, digits: usize) -> Self {
        let digits = digits.clamp(DEFAULT_COUNTER_DIGITS, MAX_COUNTER_DIGITS);
        Self { rule: Rule::Format { base: base.to_owned(), start, digits }, part: NamePart::Stem }
    }

    #[must_use]
    pub fn change_case(change: CaseChange, part: NamePart) -> Self {
        Self { rule: Rule::ChangeCase(change), part }
    }

    /// Plans every item, in order. `is_taken` reports whether a path exists
    /// at all; the planner itself discounts items that are moving away.
    pub fn plan(&self, items: &[FileEntry], is_taken: impl Fn(&StoragePath) -> bool) -> Vec<PlannedRename> {
        let mut plan: Vec<PlannedRename> = items
            .iter()
            .enumerate()
            .map(|(index, entry)| PlannedRename { item: entry.path().clone(), outcome: self.propose(entry, index) })
            .collect();
        flag_duplicates(&mut plan);
        flag_taken(&mut plan, &is_taken);
        plan
    }

    fn propose(&self, entry: &FileEntry, index: usize) -> RenameOutcome {
        let Some(name) = entry.name().as_str() else { return RenameOutcome::Problem(RenameProblem::NotText) };
        let keep_extension = self.part == NamePart::Stem && !entry.kind().is_container();
        let (edited, extension) = match name.rsplit_once('.') {
            Some((stem, ext)) if keep_extension && !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
            _ => (name, None),
        };
        let mut new_name = self.apply(edited, index);
        if let Some(extension) = extension {
            new_name.push('.');
            new_name.push_str(extension);
        }
        match PathSegment::new(&new_name) {
            Ok(segment) if segment == *entry.name() => RenameOutcome::Unchanged,
            Ok(segment) => RenameOutcome::Rename(segment),
            Err(_) => RenameOutcome::Problem(RenameProblem::InvalidName),
        }
    }

    fn apply(&self, text: &str, index: usize) -> String {
        match &self.rule {
            Rule::Replace { find: None, .. } => text.to_owned(),
            Rule::Replace { find: Some(find), replacement, matching: Matching::Text } => {
                find.replace_all(text, NoExpand(replacement)).into_owned()
            }
            Rule::Replace { find: Some(find), replacement, matching: Matching::Pattern } => {
                find.replace_all(text, replacement.as_str()).into_owned()
            }
            Rule::Add { text: added, position: AddPosition::BeforeName } => format!("{added}{text}"),
            Rule::Add { text: added, position: AddPosition::AfterName } => format!("{text}{added}"),
            Rule::Format { base, start, digits } => {
                let number = start.saturating_add(index as u64);
                format!("{base} {number:0digits$}")
            }
            Rule::ChangeCase(change) => change_case(text, *change),
        }
    }
}

fn change_case(text: &str, change: CaseChange) -> String {
    match change {
        CaseChange::Lower => text.to_lowercase(),
        CaseChange::Upper => text.to_uppercase(),
        CaseChange::Title => {
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = true;
            for c in text.chars() {
                if at_word_start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                at_word_start = c.is_whitespace() || matches!(c, '-' | '_');
            }
            out
        }
    }
}

fn target(planned: &PlannedRename) -> Option<StoragePath> {
    match &planned.outcome {
        RenameOutcome::Rename(name) => planned.item.parent().map(|parent| parent.join(name.clone())),
        _ => None,
    }
}

fn flag_duplicates(plan: &mut [PlannedRename]) {
    let mut counts: HashMap<StoragePath, usize> = HashMap::new();
    for target in plan.iter().filter_map(target) {
        *counts.entry(target).or_default() += 1;
    }
    for planned in plan.iter_mut() {
        if target(planned).is_some_and(|t| counts.get(&t).copied().unwrap_or_default() > 1) {
            planned.outcome = RenameOutcome::Problem(RenameProblem::DuplicateName);
        }
    }
}

/// A target is free if nothing is there, or if the item there is itself
/// renamed away in this batch. Flagging one item can block another, so this
/// repeats until nothing changes.
fn flag_taken(plan: &mut [PlannedRename], is_taken: &impl Fn(&StoragePath) -> bool) {
    loop {
        let moving_away: HashSet<StoragePath> =
            plan.iter().filter(|p| matches!(p.outcome, RenameOutcome::Rename(_))).map(|p| p.item.clone()).collect();
        let mut changed = false;
        for planned in plan.iter_mut() {
            let Some(target) = target(planned) else { continue };
            if is_taken(&target) && !moving_away.contains(&target) {
                planned.outcome = RenameOutcome::Problem(RenameProblem::NameTaken);
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryKind, EntryMetadata};

    fn files(names: &[&str]) -> Vec<FileEntry> {
        let parent = StoragePath::parse("file:///d").unwrap();
        names
            .iter()
            .map(|n| FileEntry::new(&parent, PathSegment::new(n).unwrap(), EntryKind::File, EntryMetadata::default()))
            .collect()
    }

    fn outcomes(plan: &[PlannedRename]) -> Vec<String> {
        plan.iter()
            .map(|p| match &p.outcome {
                RenameOutcome::Rename(name) => name.to_string(),
                RenameOutcome::Unchanged => "=".to_owned(),
                RenameOutcome::Problem(problem) => format!("{problem:?}"),
            })
            .collect()
    }

    fn nothing_taken(_: &StoragePath) -> bool {
        false
    }

    #[test]
    fn replace_text_keeps_extensions_and_literal_dollars() {
        let rule = BulkRename::replace("IMG", "Trip $1", Matching::Text, CaseSensitivity::Insensitive, NamePart::Stem)
            .unwrap();
        assert_eq!(outcomes(&rule.plan(&files(&["img_01.jpg", "notes.txt"]), nothing_taken)), ["Trip $1_01.jpg", "="]);
    }

    #[test]
    fn regex_replacement_uses_groups() {
        let rule = BulkRename::replace(
            r"(\d{4})-(\d{2})-(\d{2})",
            "$3.$2.$1",
            Matching::Pattern,
            CaseSensitivity::Sensitive,
            NamePart::Stem,
        )
        .unwrap();
        assert_eq!(outcomes(&rule.plan(&files(&["scan 2024-03-01.pdf"]), nothing_taken)), ["scan 01.03.2024.pdf"]);
        assert!(BulkRename::replace("(", "", Matching::Pattern, CaseSensitivity::Sensitive, NamePart::Stem).is_err());
    }

    #[test]
    fn format_numbers_in_order_with_padding() {
        let rule = BulkRename::format("Holiday", 1, 3);
        assert_eq!(
            outcomes(&rule.plan(&files(&["b.jpg", "a.png"]), nothing_taken)),
            ["Holiday 001.jpg", "Holiday 002.png"]
        );
    }

    #[test]
    fn add_and_case_rules() {
        let added = BulkRename::add("draft-", AddPosition::BeforeName);
        assert_eq!(outcomes(&added.plan(&files(&["essay.md"]), nothing_taken)), ["draft-essay.md"]);
        let titled = BulkRename::change_case(CaseChange::Title, NamePart::Stem);
        assert_eq!(
            outcomes(&titled.plan(&files(&["my holiday-PHOTOS.JPG"]), nothing_taken)),
            ["My Holiday-Photos.JPG"]
        );
    }

    #[test]
    fn duplicates_and_invalid_names_are_flagged() {
        let rule =
            BulkRename::replace(r"\d", "", Matching::Pattern, CaseSensitivity::Sensitive, NamePart::Stem).unwrap();
        assert_eq!(
            outcomes(&rule.plan(&files(&["a1.txt", "a2.txt", "3"]), nothing_taken)),
            ["DuplicateName", "DuplicateName", "InvalidName"]
        );
    }

    #[test]
    fn existing_names_block_unless_they_move_away() {
        // Swap: a → b and b → a are both fine because both move.
        let swap =
            BulkRename::replace("^(a|b)$", "x", Matching::Pattern, CaseSensitivity::Sensitive, NamePart::WholeName)
                .unwrap();
        let taken = |p: &StoragePath| p.name().is_some_and(|n| n.as_str() == Some("x"));
        assert_eq!(outcomes(&swap.plan(&files(&["a"]), taken)), ["NameTaken"]);

        let shift = BulkRename::format("f", 3, 1);
        let existing = |p: &StoragePath| ["f 2", "f 3"].contains(&p.name().and_then(PathSegment::as_str).unwrap_or(""));
        // "f 2" → "f 3" is fine only because "f 3" itself moves to "f 4".
        assert_eq!(outcomes(&shift.plan(&files(&["f 2", "f 3"]), existing)), ["f 3", "f 4"]);
    }
}
