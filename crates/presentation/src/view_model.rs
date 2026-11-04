//! Sorted directory state owned by one `FolderSession`, and the minimal list
//! edits that keep the Qt model in sync with it.

use std::cmp::Ordering;

use application::use_cases::ChangeBatch;
use domain::{ChangeEvent, EntryBatch, FileEntry, SortSpec, StoragePath};

use crate::row_format::to_row;
use crate::rows_bridge::EntryRow;

/// Above this many changes in one batch, a reset is cheaper for Qt than
/// individual row inserts/removes (each of which shifts the row vector).
const RESET_THRESHOLD: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelUpdate {
    Reset(Vec<EntryRow>),
    Insert { index: usize, row: EntryRow },
    Remove { index: usize },
    Replace { index: usize, row: EntryRow },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChangeOutcome {
    Updates(Vec<ModelUpdate>),
    RescanRequired,
}

#[derive(Debug)]
pub(crate) struct DirectoryViewModel {
    sort: SortSpec,
    entries: Vec<FileEntry>,
    sorted: bool,
}

impl DirectoryViewModel {
    pub(crate) fn new(sort: SortSpec) -> Self {
        Self { sort, entries: Vec::new(), sorted: true }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.sorted = true;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Appends without sorting; the stable sort in [`Self::snapshot`] merges
    /// the already-sorted prefix with new runs cheaply.
    pub(crate) fn add_batch(&mut self, batch: EntryBatch) {
        self.entries.extend(batch);
        self.sorted = false;
    }

    pub(crate) fn snapshot(&mut self) -> ModelUpdate {
        self.ensure_sorted();
        ModelUpdate::Reset(self.entries.iter().map(to_row).collect())
    }

    pub(crate) fn set_sort(&mut self, sort: SortSpec) -> ModelUpdate {
        self.sort = sort;
        self.sorted = false;
        self.snapshot()
    }

    pub(crate) fn apply(&mut self, changes: ChangeBatch) -> ChangeOutcome {
        if changes.iter().any(|c| matches!(c, ChangeEvent::Rescan)) {
            return ChangeOutcome::RescanRequired;
        }
        self.ensure_sorted();
        if changes.len() > RESET_THRESHOLD {
            changes.into_iter().for_each(|change| drop(self.apply_one(change)));
            return ChangeOutcome::Updates(vec![self.snapshot()]);
        }
        ChangeOutcome::Updates(changes.into_iter().flat_map(|change| self.apply_one(change)).collect())
    }

    fn apply_one(&mut self, change: ChangeEvent) -> Vec<ModelUpdate> {
        match change {
            ChangeEvent::Upserted(entry) => self.upsert(entry),
            ChangeEvent::Removed(path) => self.remove(&path).into_iter().collect(),
            ChangeEvent::Rescan => Vec::new(),
        }
    }

    fn upsert(&mut self, entry: FileEntry) -> Vec<ModelUpdate> {
        let previous = self.position_of(entry.path());
        if let Some(index) = previous {
            self.entries.remove(index);
        }
        let index = self.insertion_point(&entry);
        let row = to_row(&entry);
        self.entries.insert(index, entry);
        match previous {
            Some(old) if old == index => vec![ModelUpdate::Replace { index, row }],
            Some(old) => vec![ModelUpdate::Remove { index: old }, ModelUpdate::Insert { index, row }],
            None => vec![ModelUpdate::Insert { index, row }],
        }
    }

    fn remove(&mut self, path: &StoragePath) -> Option<ModelUpdate> {
        let index = self.position_of(path)?;
        self.entries.remove(index);
        Some(ModelUpdate::Remove { index })
    }

    fn position_of(&self, path: &StoragePath) -> Option<usize> {
        self.entries.iter().position(|e| e.path() == path)
    }

    fn insertion_point(&self, entry: &FileEntry) -> usize {
        self.entries.partition_point(|existing| self.sort.compare(existing, entry) == Ordering::Less)
    }

    fn ensure_sorted(&mut self) {
        if !self.sorted {
            let sort = self.sort;
            self.entries.sort_by(|a, b| sort.compare(a, b));
            self.sorted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::testing::FakeVfs;
    use domain::{SortKey, SortOrder};

    fn entries(fake: &FakeVfs, names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| {
                fake.add_file(&format!("/d/{n}"), n.len() as u64);
                match fake.upserted(&format!("/d/{n}")) {
                    ChangeEvent::Upserted(entry) => entry,
                    other => panic!("unexpected {other:?}"),
                }
            })
            .collect()
    }

    fn names(update: &ModelUpdate) -> Vec<String> {
        match update {
            ModelUpdate::Reset(rows) => rows.iter().map(|r| r.name.clone()).collect(),
            other => panic!("expected reset, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_is_sorted_across_batches() {
        let fake = FakeVfs::new();
        let mut vm = DirectoryViewModel::new(SortSpec::default());
        vm.add_batch(entries(&fake, &["c", "a10"]));
        vm.add_batch(entries(&fake, &["a2", "b"]));
        assert_eq!(names(&vm.snapshot()), ["a2", "a10", "b", "c"]);
    }

    #[test]
    fn upsert_new_entry_inserts_at_sorted_position() {
        let fake = FakeVfs::new();
        let mut vm = DirectoryViewModel::new(SortSpec::default());
        vm.add_batch(entries(&fake, &["a", "c"]));
        let new = entries(&fake, &["b"]).remove(0);
        let outcome = vm.apply(vec![ChangeEvent::Upserted(new.clone())]);
        assert_eq!(outcome, ChangeOutcome::Updates(vec![ModelUpdate::Insert { index: 1, row: to_row(&new) }]));
    }

    #[test]
    fn upsert_existing_entry_that_moves_is_remove_then_insert() {
        let fake = FakeVfs::new();
        let sort = SortSpec { key: SortKey::Size, order: SortOrder::Ascending, ..SortSpec::default() };
        let mut vm = DirectoryViewModel::new(sort);
        vm.add_batch(entries(&fake, &["a", "bb", "ccc"]));
        fake.add_file("/d/a", 10);
        let ChangeEvent::Upserted(grown) = fake.upserted("/d/a") else { panic!() };
        let outcome = vm.apply(vec![ChangeEvent::Upserted(grown.clone())]);
        assert_eq!(
            outcome,
            ChangeOutcome::Updates(vec![
                ModelUpdate::Remove { index: 0 },
                ModelUpdate::Insert { index: 2, row: to_row(&grown) }
            ])
        );
    }

    #[test]
    fn removal_of_known_and_unknown_paths() {
        let fake = FakeVfs::new();
        let mut vm = DirectoryViewModel::new(SortSpec::default());
        vm.add_batch(entries(&fake, &["a", "b"]));
        let outcome =
            vm.apply(vec![ChangeEvent::Removed(fake.path("/d/b")), ChangeEvent::Removed(fake.path("/d/zzz"))]);
        assert_eq!(outcome, ChangeOutcome::Updates(vec![ModelUpdate::Remove { index: 1 }]));
        assert_eq!(vm.len(), 1);
    }

    #[test]
    fn rescan_is_requested() {
        let mut vm = DirectoryViewModel::new(SortSpec::default());
        assert_eq!(vm.apply(vec![ChangeEvent::Rescan]), ChangeOutcome::RescanRequired);
    }

    #[test]
    fn large_bursts_become_a_reset() {
        let fake = FakeVfs::new();
        let mut vm = DirectoryViewModel::new(SortSpec::default());
        let burst: Vec<ChangeEvent> = (0..=RESET_THRESHOLD)
            .map(|i| {
                fake.add_file(&format!("/d/f{i}"), 1);
                fake.upserted(&format!("/d/f{i}"))
            })
            .collect();
        let ChangeOutcome::Updates(updates) = vm.apply(burst) else { panic!() };
        assert!(matches!(updates.as_slice(), [ModelUpdate::Reset(rows)] if rows.len() == RESET_THRESHOLD + 1));
    }
}
