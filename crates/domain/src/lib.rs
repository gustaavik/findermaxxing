//! Innermost layer of FinderMaxxing. Everything here is pure data and rules;
//! nothing performs IO or knows which runtime, UI toolkit or provider is in use.

mod capability;
mod change;
mod entry;
mod error;
mod path;
mod sorting;
mod units;

pub use capability::{Capabilities, Capability};
pub use change::ChangeEvent;
pub use entry::{EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, SymlinkTarget, Visibility};
pub use error::{PathError, VfsError};
pub use path::{MountId, PathSegment, Scheme, StoragePath};
pub use sorting::{FolderPlacement, SortKey, SortOrder, SortSpec, natural_cmp};
pub use units::{ByteSize, Timestamp, UnixMode};
