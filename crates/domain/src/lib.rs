//! Innermost layer of FinderMaxxing. Everything here is pure data and rules;
//! nothing performs IO or knows which runtime, UI toolkit or provider is in use.

mod archive;
mod bulk_rename;
mod cache;
mod capability;
mod category;
mod change;
mod checksum;
mod clutter;
mod conflict;
mod cost;
mod disk_usage;
mod drive;
mod duplicates;
mod entry;
mod error;
mod imaging;
mod indexing;
mod job;
mod licensing;
mod naming;
mod natural_query;
mod operation;
mod organize;
mod organize_rules;
mod path;
mod permissions;
mod remote;
mod search;
mod server;
mod sorting;
mod tags;
mod units;

pub use archive::{ArchiveFormat, CREATABLE_FORMATS, UnsafeEntry, archive_name, entry_path, extraction_folder_name};
pub use bulk_rename::{
    AddPosition, BulkRename, CaseChange, CaseSensitivity, Matching, NamePart, PlannedRename, RenameOutcome,
    RenameProblem, RuleError,
};
pub use cache::{CachePolicy, CachedCopy, DEFAULT_CACHE_LIMIT, Retention};
pub use capability::{Capabilities, Capability};
pub use category::FileCategory;
pub use change::ChangeEvent;
pub use checksum::{Digest, HashAlgorithm, Verification, digest_in_list, verify};
pub use clutter::{ClutterPolicy, ClutterReason, clutter_of, is_downloads_folder, is_installer, is_screenshot};
pub use conflict::{Conflict, ConflictChoice, ConflictMemory, ConflictResolution, ConflictScope};
pub use cost::{CostDecision, CostPolicy, CostReason, CostWarning, DEFAULT_LARGE_TRANSFER, TransferEstimate};
pub use disk_usage::{LARGEST_FILES_SHOWN, UsageKind, UsageNode};
pub use drive::{
    DROPBOX_SCHEME, DriveAccount, DriveLocation, DriveProvider, GOOGLE_DRIVE_SCHEME, ONEDRIVE_SCHEME, disambiguate,
};
pub use duplicates::{
    DEFAULT_MIN_DUPLICATE_SIZE, DuplicateGroup, DuplicateMatch, ImageFingerprint, PARTIAL_HASH_BYTES,
    SIMILAR_IMAGE_DISTANCE, keeper_first, looks_like_copy, same_size_groups, similar_groups, sort_groups, split_by,
};
pub use entry::{EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, SymlinkTarget, Visibility};
pub use error::{PathError, VfsError};
pub use imaging::{
    ConversionRequest, DEFAULT_JPEG_QUALITY, ImageFormat, MetadataField, Resize, Sensitivity, converted_name,
    sensitivity_of,
};
pub use indexing::{
    ContentKind, Fingerprint, IndexDecision, IndexScope, IndexStatus, MAX_PASSAGES, category_matches, index_decision,
    passages, relevance,
};
pub use job::{JobPhase, JobProgress};
pub use licensing::{Activation, LicenseStatus, ProFeature, RENEW_BEFORE_DAYS, license_status, should_renew};
pub use naming::{FIRST_DUPLICATE_NUMBER, first_free_name, numbered_name};
pub use natural_query::{DateRange, ParsedQuery, QueryClock, parse_query};
pub use operation::{Action, ModeChange, Operation, OperationKind, Relocation, TrashTicket};
pub use organize::{Change, Plan, PreviewRow, Resolution, Suggestion, SuggestionId, SuggestionSource, resolve};
pub use organize_rules::{
    MIN_GROUP_SIZE, MIN_PHOTOS_PER_MONTH, cluster, dated_name, group_label, is_camera_name, is_generic_name,
    parse_exif_time, photo_month_folder, titled_name,
};
pub use path::{MountId, PathSegment, Scheme, StoragePath};
pub use permissions::{
    Access, BitChange, PermissionClass, PermissionEdit, PermissionReach, common_bits, octal, parse_octal,
    permission_bit, symbolic,
};
pub use remote::{AccountId, Addressing, BucketName, S3_SCHEME, S3Account, S3Auth, S3Location};
pub use search::{HiddenItems, ModifiedWithin, SavedSearch, SavedSearchId, SearchMatcher, SearchQuery, SizeClass};
pub use server::{
    HostKey, HostTrust, SFTP_SCHEME, SMB_SCHEME, ServerAccount, ServerAuth, ServerLocation, ServerProtocol, Transport,
    WEBDAV_SCHEME,
};
pub use sorting::{FolderPlacement, SortKey, SortOrder, SortSpec, natural_cmp};
pub use tags::{Tag, TagError, merged_tags, tags_for_text};
pub use units::{ByteSize, CivilTime, Timestamp, UnixMode};
