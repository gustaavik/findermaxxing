use crate::entry::FileEntry;

/// An item being copied or moved whose name already exists at the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub incoming: FileEntry,
    pub existing: FileEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Move the existing item to the trash, then transfer (undoable).
    Replace,
    Skip,
    /// Transfer under a numbered name (`name 2.txt`).
    KeepBoth,
    /// Abandon the remaining items of the job.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictScope {
    ThisItem,
    AllRemaining,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictResolution {
    pub choice: ConflictChoice,
    pub scope: ConflictScope,
}

/// Remembers an "apply to all" answer for the rest of one job.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConflictMemory {
    remembered: Option<ConflictChoice>,
}

impl ConflictMemory {
    #[must_use]
    pub fn remembered(&self) -> Option<ConflictChoice> {
        self.remembered
    }

    pub fn record(&mut self, resolution: ConflictResolution) {
        if resolution.scope == ConflictScope::AllRemaining {
            self.remembered = Some(resolution.choice);
        }
    }
}
