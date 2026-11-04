use std::sync::{Mutex, MutexGuard, PoisonError};

use domain::{Operation, OperationKind};

/// How many operations can be undone. Older entries are forgotten.
pub const HISTORY_LIMIT: usize = 100;

/// One undoable (or redoable) step and the user action it stems from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub kind: OperationKind,
    pub operation: Operation,
}

/// Undo/redo stacks shared by every file-operation use case of one window.
///
/// Shared mutable state is deliberate: jobs finish on worker tasks in any
/// order and each records itself here.
#[derive(Debug, Default)]
pub struct OperationHistory {
    stacks: Mutex<Stacks>,
}

#[derive(Debug, Default)]
struct Stacks {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
}

impl OperationHistory {
    /// A new user action: becomes undoable and invalidates redo.
    pub fn record(&self, kind: OperationKind, operation: Operation) {
        if operation.is_empty() {
            return;
        }
        let mut stacks = self.lock();
        push_bounded(&mut stacks.undo, HistoryEntry { kind, operation });
        stacks.redo.clear();
    }

    #[must_use]
    pub fn next_undo(&self) -> Option<HistoryEntry> {
        self.lock().undo.last().cloned()
    }

    #[must_use]
    pub fn next_redo(&self) -> Option<HistoryEntry> {
        self.lock().redo.last().cloned()
    }

    pub(crate) fn take_undo(&self) -> Option<HistoryEntry> {
        self.lock().undo.pop()
    }

    pub(crate) fn take_redo(&self) -> Option<HistoryEntry> {
        self.lock().redo.pop()
    }

    pub(crate) fn push_undo(&self, entry: HistoryEntry) {
        if !entry.operation.is_empty() {
            push_bounded(&mut self.lock().undo, entry);
        }
    }

    pub(crate) fn push_redo(&self, entry: HistoryEntry) {
        if !entry.operation.is_empty() {
            push_bounded(&mut self.lock().redo, entry);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Stacks> {
        self.stacks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn push_bounded(stack: &mut Vec<HistoryEntry>, entry: HistoryEntry) {
    if stack.len() == HISTORY_LIMIT {
        stack.remove(0);
    }
    stack.push(entry);
}
