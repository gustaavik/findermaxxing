//! Organization suggestions (ADR 0029): what could change in one folder —
//! group items into subfolders, rename them, tag them, or put clutter in
//! the Trash — and how the ones the user accepts become concrete steps.
//!
//! Nothing here changes files; [`resolve`] only works out where everything
//! would end up, so the preview and the applied result are the same.

use std::collections::{BTreeMap, HashSet};

use crate::clutter::ClutterReason;
use crate::entry::EntryKind;
use crate::error::PathError;
use crate::naming::numbered_name;
use crate::operation::Relocation;
use crate::path::{PathSegment, StoragePath};
use crate::tags::Tag;

/// Where a suggestion came from (shown, so people know what to trust).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionSource {
    /// Fixed rules: dates, kinds, names, clutter.
    Rules,
    /// The local language model (the content index's embeddings).
    LocalModel,
    /// The user's cloud model, for a folder they opted in.
    CloudModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Into a subfolder of the analysed folder (created if missing).
    MoveInto(PathSegment),
    Rename(PathSegment),
    AddTags(Vec<Tag>),
    Trash(ClutterReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SuggestionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub id: SuggestionId,
    /// An item directly in the analysed folder.
    pub item: StoragePath,
    pub kind: EntryKind,
    pub change: Change,
    /// One line saying why, shown next to the change.
    pub reason: String,
    pub source: SuggestionSource,
}

/// Everything suggested for one folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub folder: Option<StoragePath>,
    pub suggestions: Vec<Suggestion>,
}

impl Plan {
    /// Adds a suggestion with the next id.
    pub fn suggest(
        &mut self,
        item: StoragePath,
        kind: EntryKind,
        change: Change,
        reason: String,
        source: SuggestionSource,
    ) {
        let id = SuggestionId(u32::try_from(self.suggestions.len()).unwrap_or(u32::MAX));
        self.suggestions.push(Suggestion { id, item, kind, change, reason, source });
    }

    /// Whether `item` already has a suggestion of the same sort as `change`.
    #[must_use]
    pub fn has(&self, item: &StoragePath, change: &Change) -> bool {
        self.suggestions
            .iter()
            .any(|s| s.item == *item && std::mem::discriminant(&s.change) == std::mem::discriminant(change))
    }
}

/// One line of the before/after preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRow {
    pub before: StoragePath,
    /// `None` when the item goes to the Trash.
    pub after: Option<StoragePath>,
    pub tags: Vec<Tag>,
}

/// The accepted suggestions as steps, in the order they're applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    pub folders_to_create: Vec<StoragePath>,
    pub trash: Vec<StoragePath>,
    pub moves: Vec<Relocation>,
    /// Final locations and the tags to add there.
    pub tags: Vec<(StoragePath, Vec<Tag>)>,
    pub preview: Vec<PreviewRow>,
}

impl Resolution {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folders_to_create.is_empty() && self.trash.is_empty() && self.moves.is_empty() && self.tags.is_empty()
    }
}

/// What the accepted suggestions do to one item.
#[derive(Default)]
struct ItemPlan {
    kind: Option<EntryKind>,
    into: Option<PathSegment>,
    rename: Option<PathSegment>,
    tags: Vec<Tag>,
    trash: bool,
}

/// Works out the accepted suggestions of `plan`.
///
/// - The Trash wins: an item going there isn't also moved or tagged.
/// - A move and a rename of one item combine (`Invoices/2026-03 Electricity.pdf`).
/// - Names that would clash — with each other, or with `existing` (the
///   folder's current names) — are numbered (`report 2.pdf`), so the
///   preview shows exactly what applying does.
///
/// # Errors
/// Never in practice; names are built from valid segments.
pub fn resolve(
    plan: &Plan,
    accepted: &HashSet<SuggestionId>,
    existing: &HashSet<PathSegment>,
) -> Result<Resolution, PathError> {
    let Some(folder) = plan.folder.clone() else { return Ok(Resolution::default()) };
    let mut items: BTreeMap<StoragePath, ItemPlan> = BTreeMap::new();
    for suggestion in plan.suggestions.iter().filter(|s| accepted.contains(&s.id)) {
        let item = items.entry(suggestion.item.clone()).or_default();
        item.kind = Some(suggestion.kind);
        match &suggestion.change {
            Change::MoveInto(name) => item.into = Some(name.clone()),
            Change::Rename(name) => item.rename = Some(name.clone()),
            Change::AddTags(tags) => item.tags.extend(tags.iter().cloned()),
            Change::Trash(_) => item.trash = true,
        }
    }
    let mut resolution = Resolution::default();
    let moving_away: HashSet<PathSegment> = items
        .iter()
        .filter(|(_, p)| p.trash || p.into.is_some() || p.rename.is_some())
        .filter_map(|(i, _)| i.name().cloned())
        .collect();
    // Names still taken at the top level once everything has moved.
    let mut taken: HashSet<StoragePath> =
        existing.iter().filter(|n| !moving_away.contains(*n)).map(|n| folder.join(n.clone())).collect();
    let mut created: HashSet<PathSegment> = HashSet::new();
    for (item, planned) in &items {
        if planned.trash {
            resolution.trash.push(item.clone());
            resolution.preview.push(PreviewRow { before: item.clone(), after: None, tags: Vec::new() });
            continue;
        }
        let Some(name) = planned.rename.clone().or_else(|| item.name().cloned()) else { continue };
        let parent = match &planned.into {
            Some(group) => {
                if !existing.contains(group) && created.insert(group.clone()) {
                    resolution.folders_to_create.push(folder.join(group.clone()));
                }
                folder.join(group.clone())
            }
            None => folder.clone(),
        };
        let kind = planned.kind.unwrap_or(EntryKind::File);
        let target = free_target(&parent, &name, kind, &taken, item)?;
        taken.insert(target.clone());
        if target != *item {
            resolution.moves.push(Relocation { from: item.clone(), to: target.clone() });
        }
        if !planned.tags.is_empty() {
            resolution.tags.push((target.clone(), planned.tags.clone()));
        }
        resolution.preview.push(PreviewRow { before: item.clone(), after: Some(target), tags: planned.tags.clone() });
    }
    Ok(resolution)
}

fn free_target(
    parent: &StoragePath,
    name: &PathSegment,
    kind: EntryKind,
    taken: &HashSet<StoragePath>,
    item: &StoragePath,
) -> Result<StoragePath, PathError> {
    let mut candidate = parent.join(name.clone());
    let mut number = crate::naming::FIRST_DUPLICATE_NUMBER;
    while taken.contains(&candidate) && candidate != *item {
        candidate = parent.join(numbered_name(name, kind, number)?);
        number = number.saturating_add(1);
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> StoragePath {
        StoragePath::parse(&format!("file://{path}")).unwrap()
    }

    fn seg(name: &str) -> PathSegment {
        PathSegment::new(name).unwrap()
    }

    fn plan(changes: Vec<(&str, Change)>) -> Plan {
        let mut plan = Plan { folder: Some(p("/d")), suggestions: Vec::new() };
        for (item, change) in changes {
            plan.suggest(p(&format!("/d/{item}")), EntryKind::File, change, String::new(), SuggestionSource::Rules);
        }
        plan
    }

    fn all(plan: &Plan) -> HashSet<SuggestionId> {
        plan.suggestions.iter().map(|s| s.id).collect()
    }

    fn names(existing: &[&str]) -> HashSet<PathSegment> {
        existing.iter().map(|n| seg(n)).collect()
    }

    #[test]
    fn moves_and_renames_combine_and_new_folders_are_created_once() {
        let plan = plan(vec![
            ("a.pdf", Change::MoveInto(seg("Invoices"))),
            ("a.pdf", Change::Rename(seg("2026-03 Power.pdf"))),
            ("b.pdf", Change::MoveInto(seg("Invoices"))),
        ]);
        let r = resolve(&plan, &all(&plan), &names(&["a.pdf", "b.pdf"])).unwrap();
        assert_eq!(r.folders_to_create, [p("/d/Invoices")]);
        assert_eq!(
            r.moves,
            [
                Relocation { from: p("/d/a.pdf"), to: p("/d/Invoices/2026-03 Power.pdf") },
                Relocation { from: p("/d/b.pdf"), to: p("/d/Invoices/b.pdf") },
            ]
        );
    }

    #[test]
    fn the_trash_wins_and_unaccepted_suggestions_do_nothing() {
        let plan = plan(vec![
            ("old.dmg", Change::Trash(ClutterReason::Installer { days: 30 })),
            ("old.dmg", Change::MoveInto(seg("Installers"))),
            ("c.txt", Change::Rename(seg("notes.txt"))),
        ]);
        let accepted: HashSet<SuggestionId> = [SuggestionId(0), SuggestionId(1)].into_iter().collect();
        let r = resolve(&plan, &accepted, &names(&["old.dmg", "c.txt"])).unwrap();
        assert_eq!(r.trash, [p("/d/old.dmg")]);
        assert!(r.moves.is_empty() && r.folders_to_create.is_empty());
        assert_eq!(r.preview[0].after, None);
    }

    #[test]
    fn clashing_names_are_numbered_but_freed_names_are_reused() {
        let plan = plan(vec![
            ("IMG_1.jpg", Change::Rename(seg("2026-03-14.jpg"))),
            ("IMG_2.jpg", Change::Rename(seg("2026-03-14.jpg"))),
            ("x.txt", Change::Rename(seg("notes.txt"))),
            ("notes.txt", Change::MoveInto(seg("Old"))),
        ]);
        let r = resolve(&plan, &all(&plan), &names(&["IMG_1.jpg", "IMG_2.jpg", "x.txt", "notes.txt", "Old"])).unwrap();
        let targets: Vec<String> = r.moves.iter().map(|m| m.to.path_string()).collect();
        assert!(targets.contains(&"/d/2026-03-14.jpg".to_owned()));
        assert!(targets.contains(&"/d/2026-03-14 2.jpg".to_owned()));
        assert!(targets.contains(&"/d/notes.txt".to_owned()), "notes.txt moves away, so its name is free");
        assert!(r.folders_to_create.is_empty(), "Old exists");
    }

    #[test]
    fn tags_land_on_the_final_location() {
        let tag = Tag::new("Invoice").unwrap();
        let plan = plan(vec![("a.pdf", Change::AddTags(vec![tag.clone()])), ("a.pdf", Change::MoveInto(seg("Bills")))]);
        let r = resolve(&plan, &all(&plan), &names(&["a.pdf"])).unwrap();
        assert_eq!(r.tags, [(p("/d/Bills/a.pdf"), vec![tag])]);
    }
}
