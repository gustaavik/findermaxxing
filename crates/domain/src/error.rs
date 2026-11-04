use std::time::Duration;

use crate::capability::Capability;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("location {0:?} has no scheme")]
    MissingScheme(String),
    #[error("{0:?} is not a valid scheme")]
    InvalidScheme(String),
    #[error("{0:?} is not a valid authority")]
    InvalidAuthority(String),
    #[error("{0:?} is not a valid path segment")]
    InvalidSegment(String),
    #[error("the mount root has no directory entry")]
    Root,
}

/// The error contract every `Vfs` role must honour. The shared contract test
/// suite asserts which variant each situation produces, so callers can rely on
/// matching variants regardless of provider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VfsError {
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("already exists")]
    AlreadyExists,
    #[error("not a directory")]
    NotADirectory,
    #[error("is a directory")]
    IsADirectory,
    #[error("operation cancelled")]
    Cancelled,
    #[error("provider does not support {0:?}")]
    Unsupported(Capability),
    #[error("no provider registered for scheme {0:?}")]
    UnknownScheme(String),
    #[error("conflicting concurrent modification")]
    Conflict,
    #[error("temporary failure")]
    Transient { retry_after: Option<Duration> },
    #[error("invalid path: {0}")]
    InvalidPath(#[from] PathError),
    #[error("io error: {message}")]
    Io { message: String },
}
