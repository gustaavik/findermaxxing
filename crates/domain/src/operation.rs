//! The undo journal. Every completed file operation is recorded as an
//! [`Operation`] describing what actually happened; undoing it means running
//! its [`Operation::inverse`] [`Action`], which yields a new `Operation` that is
//! in turn the redo entry. Undo and redo are therefore the same mechanism.

use crate::path::StoragePath;
use crate::tags::Tag;
use crate::units::UnixMode;

/// Where a trashed item went, so it can be put back exactly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrashTicket {
    pub original: StoragePath,
    pub trashed: StoragePath,
}

/// An item that now exists at `to` and came from (or was copied from) `from`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Relocation {
    pub from: StoragePath,
    pub to: StoragePath,
}

impl Relocation {
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self { from: self.to.clone(), to: self.from.clone() }
    }
}

/// One item's permissions before and after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeChange {
    pub before: UnixMode,
    pub after: UnixMode,
}

/// A completed change, as recorded in the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Copied(Vec<Relocation>),
    /// Also used for renames (a move within the same folder).
    Moved(Vec<Relocation>),
    Trashed(Vec<TrashTicket>),
    Restored(Vec<TrashTicket>),
    CreatedFolder(StoragePath),
    /// New items made from others (archives, extracted folders, converted images).
    Created(Vec<StoragePath>),
    PermissionsChanged(Vec<(StoragePath, ModeChange)>),
    /// Each item's tags before the change.
    TagsChanged(Vec<(StoragePath, Vec<Tag>)>),
    /// Permanently deleted while undoing on a location without a Trash.
    /// Irreversible: nothing to redo.
    Deleted(Vec<StoragePath>),
    /// Parts in the order they happened (e.g. trashing a replaced item, then copying).
    Sequence(Vec<Operation>),
}

/// The user action an undo-history entry stems from. Kept unchanged through
/// undo/redo cycles so menus can say "Redo Copy" even though the redo entry
/// itself records the trashing of the copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Copy,
    Move,
    Rename,
    Trash,
    CreateFolder,
    Compress,
    Extract,
    ConvertImages,
    CleanMetadata,
    ChangePermissions,
    Organize,
}

/// Something to execute that changes the filesystem without user prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Trash(Vec<StoragePath>),
    Restore(Vec<TrashTicket>),
    Move(Vec<Relocation>),
    SetPermissions(Vec<(StoragePath, UnixMode)>),
    /// Replaces each item's tags.
    SetTags(Vec<(StoragePath, Vec<Tag>)>),
    Sequence(Vec<Action>),
}

impl Operation {
    /// The action that reverts this operation.
    #[must_use]
    pub fn inverse(&self) -> Action {
        match self {
            Self::Copied(copies) => Action::Trash(copies.iter().map(|c| c.to.clone()).collect()),
            Self::Moved(moves) => Action::Move(moves.iter().rev().map(Relocation::reversed).collect()),
            Self::Trashed(tickets) => Action::Restore(tickets.clone()),
            Self::Restored(tickets) => Action::Trash(tickets.iter().map(|t| t.original.clone()).collect()),
            Self::CreatedFolder(path) => Action::Trash(vec![path.clone()]),
            Self::Created(paths) => Action::Trash(paths.clone()),
            Self::TagsChanged(before) => Action::SetTags(before.iter().rev().cloned().collect()),
            Self::PermissionsChanged(changes) => Action::SetPermissions(
                changes.iter().rev().map(|(path, change)| (path.clone(), change.before)).collect(),
            ),
            Self::Deleted(_) => Action::Sequence(Vec::new()),
            Self::Sequence(parts) => Action::Sequence(parts.iter().rev().map(Self::inverse).collect()),
        }
    }

    /// True when nothing actually changed (every item was skipped).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Copied(items) | Self::Moved(items) => items.is_empty(),
            Self::Trashed(tickets) | Self::Restored(tickets) => tickets.is_empty(),
            Self::Deleted(paths) | Self::Created(paths) => paths.is_empty(),
            Self::PermissionsChanged(changes) => changes.is_empty(),
            Self::TagsChanged(changes) => changes.is_empty(),
            Self::CreatedFolder(_) => false,
            Self::Sequence(parts) => parts.iter().all(Self::is_empty),
        }
    }

    /// False when undoing this operation again is impossible (it deleted data for good).
    #[must_use]
    pub fn is_reversible(&self) -> bool {
        match self {
            Self::Deleted(_) => false,
            Self::Sequence(parts) => parts.iter().all(Self::is_reversible),
            _ => true,
        }
    }

    /// Flattens nested sequences and drops empty parts.
    #[must_use]
    pub fn sequence(parts: Vec<Self>) -> Self {
        let mut flat: Vec<Self> = Vec::new();
        for part in parts {
            match part {
                Self::Sequence(inner) => flat.extend(inner.into_iter().filter(|p| !p.is_empty())),
                other if !other.is_empty() => flat.push(other),
                _ => {}
            }
        }
        if flat.len() == 1 { flat.remove(0) } else { Self::Sequence(flat) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> StoragePath {
        StoragePath::parse(&format!("file://{path}")).unwrap()
    }

    fn rel(from: &str, to: &str) -> Relocation {
        Relocation { from: p(from), to: p(to) }
    }

    #[test]
    fn copy_is_undone_by_trashing_the_copies() {
        let op = Operation::Copied(vec![rel("/a/x", "/b/x")]);
        assert_eq!(op.inverse(), Action::Trash(vec![p("/b/x")]));
    }

    #[test]
    fn moves_are_reversed_in_reverse_order() {
        let op = Operation::Moved(vec![rel("/a", "/b"), rel("/c", "/d")]);
        assert_eq!(op.inverse(), Action::Move(vec![rel("/d", "/c"), rel("/b", "/a")]));
    }

    #[test]
    fn created_items_are_trashed_and_modes_restored() {
        assert_eq!(Operation::Created(vec![p("/a.zip")]).inverse(), Action::Trash(vec![p("/a.zip")]));
        let change = ModeChange { before: UnixMode::from_mode(0o644), after: UnixMode::from_mode(0o600) };
        assert_eq!(
            Operation::PermissionsChanged(vec![(p("/a"), change)]).inverse(),
            Action::SetPermissions(vec![(p("/a"), UnixMode::from_mode(0o644))])
        );
    }

    #[test]
    fn trash_and_restore_are_mutual_inverses() {
        let ticket = TrashTicket { original: p("/a"), trashed: p("/T/a") };
        assert_eq!(Operation::Trashed(vec![ticket.clone()]).inverse(), Action::Restore(vec![ticket.clone()]));
        assert_eq!(Operation::Restored(vec![ticket]).inverse(), Action::Trash(vec![p("/a")]));
    }

    #[test]
    fn sequences_invert_back_to_front() {
        let ticket = TrashTicket { original: p("/b/x"), trashed: p("/T/x") };
        let op = Operation::Sequence(vec![
            Operation::Trashed(vec![ticket.clone()]),
            Operation::Copied(vec![rel("/a/x", "/b/x")]),
        ]);
        assert_eq!(op.inverse(), Action::Sequence(vec![Action::Trash(vec![p("/b/x")]), Action::Restore(vec![ticket])]));
    }

    #[test]
    fn sequence_flattens_and_drops_empty_parts() {
        let copied = Operation::Copied(vec![rel("/a", "/b")]);
        let built = Operation::sequence(vec![
            Operation::Trashed(vec![]),
            Operation::Sequence(vec![copied.clone(), Operation::Moved(vec![])]),
        ]);
        assert_eq!(built, copied);
        assert!(Operation::sequence(vec![]).is_empty());
    }
}
