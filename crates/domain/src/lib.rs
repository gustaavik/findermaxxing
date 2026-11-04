//! Innermost layer of FinderMaxxing. Everything here is pure data and rules;
//! nothing performs IO or knows which runtime, UI toolkit or provider is in use.

mod bulk_rename;
mod capability;
mod category;
mod change;
mod conflict;
mod entry;
mod error;
mod job;
mod naming;
mod operation;
mod path;
mod search;
mod sorting;
mod units;

pub use bulk_rename::{
    AddPosition, BulkRename, CaseChange, CaseSensitivity, Matching, NamePart, PlannedRename, RenameOutcome,
    RenameProblem, RuleError,
};
pub use capability::{Capabilities, Capability};
pub use category::FileCategory;
pub use change::ChangeEvent;
pub use conflict::{Conflict, ConflictChoice, ConflictMemory, ConflictResolution, ConflictScope};
pub use entry::{EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, SymlinkTarget, Visibility};
pub use error::{PathError, VfsError};
pub use job::{JobPhase, JobProgress};
pub use naming::{FIRST_DUPLICATE_NUMBER, first_free_name, numbered_name};
pub use operation::{Action, Operation, OperationKind, Relocation, TrashTicket};
pub use path::{MountId, PathSegment, Scheme, StoragePath};
pub use search::{HiddenItems, ModifiedWithin, SavedSearch, SavedSearchId, SearchMatcher, SearchQuery, SizeClass};
pub use sorting::{FolderPlacement, SortKey, SortOrder, SortSpec, natural_cmp};
pub use units::{ByteSize, Timestamp, UnixMode};
