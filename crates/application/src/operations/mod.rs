//! Shared machinery behind the file-operation use cases: job context,
//! progress, scanning, transfers (copy/move with conflicts), the prompt-free
//! action executor used by undo/redo, and the history itself.

mod context;
mod error;
pub(crate) mod executor;
mod history;
pub(crate) mod progress;
pub(crate) mod relocate;
pub(crate) mod resume;
pub(crate) mod scan;
pub(crate) mod transfer;

pub use context::{AcceptCosts, JobContext, NullObserver};
pub use error::{OperationError, OperationReport};
pub use history::{HISTORY_LIMIT, HistoryEntry, OperationHistory};
pub use resume::RESUMABLE_SIZE;
