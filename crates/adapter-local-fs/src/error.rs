use std::io;

use domain::VfsError;

/// Maps OS errors onto the provider-independent error contract.
pub(crate) fn from_io(error: &io::Error) -> VfsError {
    match error.kind() {
        io::ErrorKind::NotFound => VfsError::NotFound,
        io::ErrorKind::PermissionDenied => VfsError::PermissionDenied,
        io::ErrorKind::AlreadyExists => VfsError::AlreadyExists,
        io::ErrorKind::NotADirectory => VfsError::NotADirectory,
        io::ErrorKind::IsADirectory => VfsError::IsADirectory,
        io::ErrorKind::CrossesDevices => VfsError::CrossDevice,
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            VfsError::Transient { retry_after: None }
        }
        _ => VfsError::Io { message: error.to_string() },
    }
}
