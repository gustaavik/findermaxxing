//! Innermost layer of FinderMaxxing. Everything here is pure data and rules;
//! nothing performs IO or knows which runtime, UI toolkit or provider is in use.

mod bulk_rename;
mod cache;
mod capability;
mod category;
mod change;
mod conflict;
mod cost;
mod drive;
mod entry;
mod error;
mod job;
mod naming;
mod operation;
mod path;
mod remote;
mod search;
mod server;
mod sorting;
mod units;

pub use bulk_rename::{
    AddPosition, BulkRename, CaseChange, CaseSensitivity, Matching, NamePart, PlannedRename, RenameOutcome,
    RenameProblem, RuleError,
};
pub use cache::{CachePolicy, CachedCopy, DEFAULT_CACHE_LIMIT, Retention};
pub use capability::{Capabilities, Capability};
pub use category::FileCategory;
pub use change::ChangeEvent;
pub use conflict::{Conflict, ConflictChoice, ConflictMemory, ConflictResolution, ConflictScope};
pub use cost::{CostDecision, CostPolicy, CostReason, CostWarning, DEFAULT_LARGE_TRANSFER, TransferEstimate};
pub use drive::{
    DROPBOX_SCHEME, DriveAccount, DriveLocation, DriveProvider, GOOGLE_DRIVE_SCHEME, ONEDRIVE_SCHEME, disambiguate,
};
pub use entry::{EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, SymlinkTarget, Visibility};
pub use error::{PathError, VfsError};
pub use job::{JobPhase, JobProgress};
pub use naming::{FIRST_DUPLICATE_NUMBER, first_free_name, numbered_name};
pub use operation::{Action, Operation, OperationKind, Relocation, TrashTicket};
pub use path::{MountId, PathSegment, Scheme, StoragePath};
pub use remote::{AccountId, Addressing, BucketName, S3_SCHEME, S3Account, S3Auth, S3Location};
pub use search::{HiddenItems, ModifiedWithin, SavedSearch, SavedSearchId, SearchMatcher, SearchQuery, SizeClass};
pub use server::{
    HostKey, HostTrust, SFTP_SCHEME, SMB_SCHEME, ServerAccount, ServerAuth, ServerLocation, ServerProtocol, Transport,
    WEBDAV_SCHEME,
};
pub use sorting::{FolderPlacement, SortKey, SortOrder, SortSpec, natural_cmp};
pub use units::{ByteSize, Timestamp, UnixMode};
