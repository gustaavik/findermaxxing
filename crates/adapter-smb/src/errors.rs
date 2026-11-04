//! smb2 errors → the shared `VfsError` contract.

use domain::VfsError;
use smb2::ErrorKind;

pub(crate) fn from_smb(error: &smb2::Error) -> VfsError {
    match error.kind() {
        ErrorKind::AccessDenied | ErrorKind::AuthRequired | ErrorKind::GuestSessionRefused => {
            VfsError::PermissionDenied
        }
        ErrorKind::NotFound => VfsError::NotFound,
        ErrorKind::AlreadyExists => VfsError::AlreadyExists,
        ErrorKind::IsADirectory => VfsError::IsADirectory,
        ErrorKind::NotADirectory => VfsError::NotADirectory,
        ErrorKind::SharingViolation => VfsError::Conflict,
        ErrorKind::ConnectionLost | ErrorKind::TimedOut | ErrorKind::SessionExpired => {
            VfsError::Transient { retry_after: None }
        }
        ErrorKind::Cancelled => VfsError::Cancelled,
        _ => VfsError::Io { message: error.to_string() },
    }
}
