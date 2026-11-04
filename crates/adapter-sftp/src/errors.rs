//! SFTP status codes → the shared `VfsError` contract.

use domain::VfsError;
use russh_sftp::client::error::Error;
use russh_sftp::protocol::StatusCode;

pub(crate) fn from_sftp(error: &Error) -> VfsError {
    match error {
        Error::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => VfsError::NotFound,
            StatusCode::PermissionDenied => VfsError::PermissionDenied,
            StatusCode::NoConnection | StatusCode::ConnectionLost => VfsError::Transient { retry_after: None },
            StatusCode::OpUnsupported => VfsError::Unsupported(domain::Capability::Write),
            _ => VfsError::Io { message: format!("{}: {}", status.status_code, status.error_message) },
        },
        Error::Timeout | Error::IO(_) => VfsError::Transient { retry_after: None },
        other => VfsError::Io { message: other.to_string() },
    }
}

pub(crate) fn from_io(error: &std::io::Error) -> VfsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound,
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied,
        _ => VfsError::Io { message: error.to_string() },
    }
}
